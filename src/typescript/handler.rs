use super::assertions::AssertionBindings;
use super::ast::push_named_children_reverse;
use super::node_text;
use super::registrations::{registration_callee, RegistrationCallee};
use crate::model::{stable_fingerprint, HandlerFingerprint};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

const MAX_ASSERTION_CHAIN_DEPTH: usize = 16;
const UNRESOLVED_ASSERTION: &str = "assert:unresolved";
const DEFERRED_UNRESOLVED_ASSERTION: &str = "deferred-assert:unresolved";

#[derive(Clone, Copy)]
pub(super) struct HandlerBinding<'tree> {
    declaration: Node<'tree>,
    scope: Node<'tree>,
    start_byte: usize,
    hoisted: bool,
}

pub(super) fn collect_handler_bindings<'tree>(
    node: Node<'tree>,
    source: &[u8],
    output: &mut BTreeMap<String, Vec<HandlerBinding<'tree>>>,
) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if matches!(
            node.kind(),
            "function_declaration" | "generator_function_declaration"
        ) {
            if let (Some(name), Some(scope)) = (
                node.child_by_field_name("name"),
                nearest_scope(node.parent()),
            ) {
                output
                    .entry(node_text(name, source).to_owned())
                    .or_default()
                    .push(HandlerBinding {
                        declaration: node,
                        scope,
                        start_byte: node.start_byte(),
                        hoisted: true,
                    });
            }
        } else if node.kind() == "variable_declarator" {
            if let (Some(name), Some(value), Some(scope)) = (
                node.child_by_field_name("name"),
                node.child_by_field_name("value"),
                nearest_scope(node.parent()),
            ) {
                if name.kind() == "identifier"
                    && matches!(
                        value.kind(),
                        "arrow_function" | "function_expression" | "generator_function"
                    )
                {
                    output
                        .entry(node_text(name, source).to_owned())
                        .or_default()
                        .push(HandlerBinding {
                            declaration: value,
                            scope,
                            start_byte: node.start_byte(),
                            hoisted: false,
                        });
                }
            }
        }

        push_named_children_reverse(node, &mut stack);
    }
}

fn nearest_scope(mut node: Option<Node<'_>>) -> Option<Node<'_>> {
    while let Some(current) = node {
        if matches!(current.kind(), "program" | "statement_block") {
            return Some(current);
        }
        node = current.parent();
    }
    None
}

pub(super) fn resolve_handler<'tree>(
    mut handler: Node<'tree>,
    source: &[u8],
    bindings: &BTreeMap<String, Vec<HandlerBinding<'tree>>>,
) -> Option<Node<'tree>> {
    while handler.kind() == "parenthesized_expression" {
        handler = handler.named_child(0)?;
    }
    if matches!(
        handler.kind(),
        "arrow_function" | "function_expression" | "generator_function"
    ) {
        return Some(handler);
    }
    if handler.kind() == "call_expression" {
        let function = handler.child_by_field_name("function")?;
        if function.kind() == "member_expression"
            && function
                .child_by_field_name("property")
                .is_some_and(|property| node_text(property, source) == "bind")
        {
            return resolve_handler(function.child_by_field_name("object")?, source, bindings);
        }
        return None;
    }
    if handler.kind() != "identifier" {
        return None;
    }

    let name = node_text(handler, source);
    let candidates = bindings.get(name)?;
    let mut scopes = Vec::new();
    let mut ancestor = handler.parent();
    while let Some(node) = ancestor {
        if matches!(node.kind(), "program" | "statement_block") {
            scopes.push(node.id());
        }
        ancestor = node.parent();
    }

    candidates
        .iter()
        .filter(|binding| binding.hoisted || binding.start_byte <= handler.start_byte())
        .filter_map(|binding| {
            scopes
                .iter()
                .position(|scope| *scope == binding.scope.id())
                .map(|scope_depth| (scope_depth, binding))
        })
        .min_by(|(left_depth, left), (right_depth, right)| {
            left_depth
                .cmp(right_depth)
                .then_with(|| right.start_byte.cmp(&left.start_byte))
        })
        .map(|(_, binding)| binding.declaration)
}

pub(super) fn bind_arguments<'tree>(handler: Node<'tree>, source: &[u8]) -> Option<Node<'tree>> {
    if handler.kind() != "call_expression" {
        return None;
    }
    let function = handler.child_by_field_name("function")?;
    if function.kind() != "member_expression"
        || function
            .child_by_field_name("property")
            .is_none_or(|property| node_text(property, source) != "bind")
    {
        return None;
    }
    handler.child_by_field_name("arguments")
}

pub(super) fn fingerprint_handler(
    handler: Node<'_>,
    bound_arguments: Option<Node<'_>>,
    source: &[u8],
    comparable: bool,
    assertions: &AssertionBindings,
) -> HandlerFingerprint {
    let declared = declared_identifiers(handler, source);
    fingerprint_node(
        handler,
        bound_arguments,
        source,
        comparable,
        &declared,
        assertions,
    )
}

/// Fingerprints a decorated class method without its name or decorator metadata.
///
/// Parameters and runtime modifiers remain part of the fingerprint: default initializers execute
/// at call time, and `async`/generator/static/accessor methods can behave differently even when
/// their bodies are textually identical.
pub(super) fn fingerprint_method_handler(
    method: Node<'_>,
    source: &[u8],
    assertions: &AssertionBindings,
) -> Option<HandlerFingerprint> {
    debug_assert_eq!(method.kind(), "method_definition");
    let name = method.child_by_field_name("name")?;
    let parameters = method.child_by_field_name("parameters")?;
    let body = method.child_by_field_name("body")?;
    let mut identifiers = Vec::new();
    collect_parameter_bindings(parameters, source, &mut identifiers);
    collect_declared_locals(body, source, &mut identifiers);
    let declared = identifier_map(identifiers);

    // Direct grammar tokens remain stable when comments separate a modifier from the name.
    // TypeScript accessibility and `override` nodes are deliberately ignored because they cannot
    // change execution behavior.
    let mut runtime_modifiers = Vec::new();
    for index in 0..method.child_count() {
        let child = method.child(index)?;
        if child.start_byte() >= name.start_byte() {
            break;
        }
        if !child.is_extra() && matches!(child.kind(), "static" | "async" | "*" | "get" | "set") {
            runtime_modifiers.push(child.kind());
        }
    }
    let modifier = runtime_modifiers.join(" ");
    let semantic_prefix = if modifier.is_empty() {
        "instance sync".to_owned()
    } else {
        modifier
    };

    let raw_parameters = node_text(parameters, source);
    let raw_body = node_text(body, source);
    let exact = format!("{semantic_prefix}\0{raw_parameters}\0{raw_body}");
    let normalized = method_representation(
        &semantic_prefix,
        parameters,
        body,
        source,
        &declared,
        AstMode::Normalized,
    );
    let alpha = method_representation(
        &semantic_prefix,
        parameters,
        body,
        source,
        &declared,
        AstMode::Alpha,
    );
    let structural = method_representation(
        &semantic_prefix,
        parameters,
        body,
        source,
        &declared,
        AstMode::Structural,
    );
    let local_constants = LocalConstants::collect(method, source, &declared);
    // Preserve invocation semantics for compatibility checks. Similarity removes this metadata
    // before measuring executed behavior so two one-assertion methods do not gain artificial
    // 50% overlap merely because both are methods.
    let mut signature = vec![format!("method:{semantic_prefix}")];
    signature.extend(behavior_signature(
        parameters,
        source,
        &declared,
        assertions,
        &local_constants,
    ));
    signature.extend(behavior_signature(
        body,
        source,
        &declared,
        assertions,
        &local_constants,
    ));
    let source_snippet = format!("{semantic_prefix} {raw_parameters} {raw_body}");
    let comparable = !signature.iter().any(|event| is_unresolved_assertion(event));

    Some(HandlerFingerprint {
        exact: stable_fingerprint(&exact),
        normalized: stable_fingerprint(&normalized),
        alpha_normalized: stable_fingerprint(&alpha),
        structural: stable_fingerprint(&structural),
        behavior_signature: signature,
        source_snippet: bounded_source_snippet(&source_snippet),
        comparable,
        // A default initializer executes before the body. Treating an empty method with one as a
        // stub would hide real behavior and could recreate pending-handler finding storms.
        trivial: !has_parameter_initializer(parameters) && is_trivial_handler(body, source),
    })
}

fn method_representation(
    semantic_prefix: &str,
    parameters: Node<'_>,
    body: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    mode: AstMode,
) -> String {
    let parameters = match mode {
        AstMode::Normalized => serialize_ast(parameters, source, declared, mode),
        AstMode::Alpha | AstMode::Structural => {
            serialize_parameter_semantics(parameters, source, declared, mode)
        }
    };
    format!(
        "{semantic_prefix}\0{}\0{}",
        parameters,
        serialize_ast(body, source, declared, mode)
    )
}

fn serialize_parameter_semantics(
    parameters: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    mode: AstMode,
) -> String {
    enum Event<'tree> {
        Visit(Node<'tree>),
        Text(String),
        Close,
    }

    let mut output = String::new();
    let mut stack = vec![Event::Visit(parameters)];
    while let Some(event) = stack.pop() {
        match event {
            Event::Text(text) => output.push_str(&text),
            Event::Close => output.push(')'),
            Event::Visit(node) => match node.kind() {
                "type_annotation" => {}
                "required_parameter" | "optional_parameter" => {
                    let binding = node
                        .child_by_field_name("name")
                        .or_else(|| node.child_by_field_name("pattern"));
                    let initializer = node.child_by_field_name("value");
                    output.push('(');
                    output.push_str(node.kind());
                    stack.push(Event::Close);
                    if let Some(initializer) = initializer {
                        stack.push(Event::Text(serialize_ast(
                            initializer,
                            source,
                            declared,
                            mode,
                        )));
                        stack.push(Event::Text("=".to_owned()));
                    }
                    if let Some(binding) = binding {
                        stack.push(Event::Visit(binding));
                    }
                }
                "identifier" => output.push_str("binding"),
                "shorthand_property_identifier_pattern" => {
                    output.push_str("property:");
                    output.push_str(node_text(node, source));
                    output.push_str("(binding)");
                }
                "pair_pattern" => {
                    let (Some(key), Some(value)) = (
                        node.child_by_field_name("key"),
                        node.child_by_field_name("value"),
                    ) else {
                        continue;
                    };
                    output.push_str("property:");
                    output.push_str(node_text(key, source));
                    output.push('(');
                    stack.push(Event::Close);
                    stack.push(Event::Visit(value));
                }
                "assignment_pattern" | "object_assignment_pattern" => {
                    let left = node
                        .child_by_field_name("left")
                        .or_else(|| node.named_child(0));
                    let right = node
                        .child_by_field_name("right")
                        .or_else(|| node.child_by_field_name("value"))
                        .or_else(|| node.named_child(1));
                    let (Some(left), Some(right)) = (left, right) else {
                        continue;
                    };
                    output.push_str("default(");
                    stack.push(Event::Close);
                    stack.push(Event::Text(serialize_ast(right, source, declared, mode)));
                    stack.push(Event::Text("=".to_owned()));
                    stack.push(Event::Visit(left));
                }
                _ => {
                    let mut cursor = node.walk();
                    let children: Vec<_> = node
                        .named_children(&mut cursor)
                        .filter(|child| child.kind() != "type_annotation")
                        .collect();
                    output.push('(');
                    output.push_str(node.kind());
                    stack.push(Event::Close);
                    for child in children.into_iter().rev() {
                        stack.push(Event::Visit(child));
                    }
                }
            },
        }
    }
    output
}

fn has_parameter_initializer(parameters: Node<'_>) -> bool {
    let mut stack = vec![parameters];
    while let Some(node) = stack.pop() {
        if matches!(
            node.kind(),
            "assignment_pattern" | "object_assignment_pattern"
        ) || matches!(node.kind(), "required_parameter" | "optional_parameter")
            && node.child_by_field_name("value").is_some()
        {
            return true;
        }
        push_named_children_reverse(node, &mut stack);
    }
    false
}

fn collect_parameter_bindings(parameters: Node<'_>, source: &[u8], output: &mut Vec<String>) {
    let mut stack = vec![parameters];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "identifier" | "shorthand_property_identifier_pattern" => {
                push_identifier(output, node_text(node, source));
            }
            // Only the left side introduces a binding; traversing the initializer would alpha-
            // rename external calls such as `first()` and `second()` into a false match.
            "assignment_pattern" | "object_assignment_pattern" => {
                if let Some(left) = node
                    .child_by_field_name("left")
                    .or_else(|| node.named_child(0))
                {
                    stack.push(left);
                }
            }
            "required_parameter" | "optional_parameter" => {
                if let Some(binding) = node
                    .child_by_field_name("name")
                    .or_else(|| node.child_by_field_name("pattern"))
                {
                    stack.push(binding);
                }
            }
            // Object keys are property names. Only the value side binds a local identifier.
            "pair_pattern" => {
                if let Some(value) = node.child_by_field_name("value") {
                    stack.push(value);
                }
            }
            "type_annotation" => {}
            _ => push_named_children_reverse(node, &mut stack),
        }
    }
}

fn fingerprint_node(
    handler: Node<'_>,
    bound_arguments: Option<Node<'_>>,
    source: &[u8],
    comparable: bool,
    declared: &BTreeMap<String, String>,
    assertions: &AssertionBindings,
) -> HandlerFingerprint {
    let raw = node_text(handler, source);
    let mut exact = raw.to_owned();
    // A module-scoped constant is a fixed value every handler in the file reads, so substituting it
    // lets two handlers that spell the same value through differently named constants compare
    // equal. Handler-local names keep their alpha renaming instead: two handlers each declaring one
    // constant already align through `v0`, and substituting there would make the alpha fingerprint
    // value-sensitive and hide the parameterization signal it exists to expose.
    let module_constants = LocalConstants::collect_module_scope(handler, source, declared);
    // Substituting through the identifier map keeps literal handling untouched: passing the
    // constants to the serializer would also route every literal through the value-fingerprint
    // branch, which is a far wider change than reading a module constant.
    let alpha_declared = module_constants.substituted_names(declared, handler, AstMode::Alpha);
    let structural_declared =
        module_constants.substituted_names(declared, handler, AstMode::Structural);
    let mut normalized = serialize_ast(handler, source, declared, AstMode::Normalized);
    let mut alpha = serialize_ast(handler, source, &alpha_declared, AstMode::Alpha);
    let mut structural = serialize_ast(handler, source, &structural_declared, AstMode::Structural);
    let local_constants = LocalConstants::collect(handler, source, declared);
    let mut signature = behavior_signature(handler, source, declared, assertions, &local_constants);
    let mut source_snippet = raw.to_owned();
    if let Some(arguments) = bound_arguments {
        let no_declarations = BTreeMap::new();
        append_bound_context(&mut exact, node_text(arguments, source));
        append_bound_context(
            &mut normalized,
            &serialize_ast(arguments, source, &no_declarations, AstMode::Normalized),
        );
        append_bound_context(
            &mut alpha,
            &serialize_ast(arguments, source, &no_declarations, AstMode::Alpha),
        );
        append_bound_context(
            &mut structural,
            &serialize_ast(arguments, source, &no_declarations, AstMode::Structural),
        );
        signature.extend(behavior_signature(
            arguments,
            source,
            &no_declarations,
            assertions,
            &LocalConstants::default(),
        ));
        source_snippet.push_str(" bound with ");
        source_snippet.push_str(node_text(arguments, source));
    }
    let comparable = comparable && !signature.iter().any(|event| is_unresolved_assertion(event));
    HandlerFingerprint {
        exact: stable_fingerprint(&exact),
        normalized: stable_fingerprint(&normalized),
        alpha_normalized: stable_fingerprint(&alpha),
        structural: stable_fingerprint(&structural),
        behavior_signature: signature,
        source_snippet: bounded_source_snippet(&source_snippet),
        comparable,
        trivial: comparable && is_trivial_handler(handler, source),
    }
}

fn append_bound_context(target: &mut String, arguments: &str) {
    target.push_str("\0bind");
    target.push_str(arguments);
}

fn is_trivial_handler(handler: Node<'_>, source: &[u8]) -> bool {
    let body = if handler.kind() == "statement_block" {
        handler
    } else {
        if !matches!(
            handler.kind(),
            "arrow_function"
                | "function_expression"
                | "generator_function"
                | "function_declaration"
                | "generator_function_declaration"
        ) {
            return false;
        }
        let Some(body) = handler.child_by_field_name("body") else {
            return false;
        };
        body
    };
    if body.kind() != "statement_block" {
        return is_stub_expression(body, source);
    }
    let mut cursor = body.walk();
    let statements: Vec<_> = body
        .named_children(&mut cursor)
        .filter(|child| child.kind() != "comment")
        .collect();
    match statements.as_slice() {
        [] => true,
        [statement] if statement.kind() == "throw_statement" => statement
            .named_child(0)
            .is_some_and(|expression| contains_explicit_stub_marker(expression, source)),
        [statement] if statement.kind() == "return_statement" => statement
            .named_child(0)
            .is_none_or(|expression| is_stub_expression(expression, source)),
        [statement] if statement.kind() == "expression_statement" => statement
            .named_child(0)
            .is_some_and(|expression| is_stub_expression(expression, source)),
        _ => false,
    }
}

fn is_stub_expression(node: Node<'_>, source: &[u8]) -> bool {
    let mut node = node;
    while matches!(node.kind(), "await_expression" | "parenthesized_expression") {
        let Some(inner) = node.named_child(0) else {
            return false;
        };
        node = inner;
    }
    if matches!(node.kind(), "null" | "undefined") {
        return true;
    }
    if matches!(node.kind(), "string" | "template_string") {
        return is_stub_label(node_text(node, source));
    }
    if node.kind() == "unary_expression" {
        let compact = node_text(node, source)
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        return matches!(compact.as_str(), "void0" | "voidundefined");
    }
    if node.kind() != "call_expression" {
        return false;
    }
    let Some(function) = node.child_by_field_name("function") else {
        return false;
    };
    let name = callee_terminal_name(function, source)
        .unwrap_or_default()
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    if matches!(name.as_str(), "pending" | "notimplemented" | "todo") {
        return true;
    }
    if function.kind() == "member_expression"
        && node_text(function, source)
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
            == "Promise.resolve"
    {
        return node
            .child_by_field_name("arguments")
            .is_some_and(|arguments| arguments.named_child_count() == 0);
    }
    false
}

fn callee_terminal_name<'a>(function: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    match function.kind() {
        "identifier" => Some(node_text(function, source)),
        "member_expression" => function
            .child_by_field_name("property")
            .map(|property| node_text(property, source)),
        _ => None,
    }
}

fn is_stub_label(value: &str) -> bool {
    let value = value
        .trim()
        .trim_matches(|character| matches!(character, '\'' | '"' | '`'));
    let normalized = value
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    matches!(normalized.as_str(), "pending" | "notimplemented" | "todo")
}

fn contains_explicit_stub_marker(node: Node<'_>, source: &[u8]) -> bool {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "string" | "template_string")
            && is_stub_label(node_text(node, source))
        {
            return true;
        }
        if node.kind() == "identifier" && is_stub_label(node_text(node, source)) {
            return true;
        }
        push_named_children_reverse(node, &mut stack);
    }
    false
}

pub(super) const MAX_HANDLER_SNIPPET_CHARS: usize = 4_000;

pub(super) fn bounded_source_snippet(source: &str) -> String {
    let normalized = source.trim().replace("\r\n", "\n").replace('\r', "\n");
    if normalized.chars().count() <= MAX_HANDLER_SNIPPET_CHARS {
        return normalized;
    }
    let mut snippet: String = normalized
        .chars()
        .take(MAX_HANDLER_SNIPPET_CHARS - 1)
        .collect();
    snippet.push('…');
    snippet
}

/// Returns the module's own top-level declarations, without descending into sibling handlers.
fn module_declarations<'tree>(handler: Node<'tree>) -> Vec<Node<'tree>> {
    let mut root = handler;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    // The root of a parsed file is always `program`. Filtering the children below is what selects
    // declarations, so no separate guard on the root kind is needed.
    let mut cursor = root.walk();
    let children: Vec<Node<'tree>> = root.named_children(&mut cursor).collect();
    let mut declarations = Vec::new();
    for child in children {
        match child.kind() {
            "lexical_declaration" | "variable_declaration" => declarations.push(child),
            "export_statement" => {
                let mut inner = child.walk();
                let exported: Vec<Node<'tree>> = child.named_children(&mut inner).collect();
                declarations.extend(exported.into_iter().filter(|node| {
                    matches!(node.kind(), "lexical_declaration" | "variable_declaration")
                }));
            }
            _ => {}
        }
    }
    declarations
}

fn declared_identifiers(handler: Node<'_>, source: &[u8]) -> BTreeMap<String, String> {
    let mut identifiers = Vec::new();
    if matches!(
        handler.kind(),
        "function_declaration" | "generator_function_declaration"
    ) {
        if let Some(name) = handler.child_by_field_name("name") {
            collect_identifier_text(name, source, &mut identifiers);
        }
    }
    if let Some(parameters) = handler
        .child_by_field_name("parameters")
        .or_else(|| handler.child_by_field_name("parameter"))
    {
        // Initializers execute in the surrounding scope; only the binding side belongs in the
        // alpha map. Otherwise an external call such as `makePage()` is renamed differently when
        // the local parameter itself is renamed.
        collect_parameter_bindings(parameters, source, &mut identifiers);
    }
    collect_declared_locals(handler, source, &mut identifiers);
    identifier_map(identifiers)
}

fn identifier_map(identifiers: Vec<String>) -> BTreeMap<String, String> {
    identifiers
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name, format!("v{index}")))
        .collect()
}

fn collect_declared_locals(node: Node<'_>, source: &[u8], output: &mut Vec<String>) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if node.kind() == "variable_declarator" {
            if let Some(name) = node.child_by_field_name("name") {
                collect_identifier_text(name, source, output);
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_identifier_text(node: Node<'_>, source: &[u8], output: &mut Vec<String>) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if matches!(
            node.kind(),
            "identifier" | "shorthand_property_identifier_pattern"
        ) {
            push_identifier(output, node_text(node, source));
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn push_identifier(output: &mut Vec<String>, identifier: &str) {
    if !output.iter().any(|existing| existing == identifier) {
        output.push(identifier.to_owned());
    }
}

#[derive(Clone, Copy)]
enum AstMode {
    Normalized,
    Alpha,
    Structural,
}

struct LocalConstant {
    declaration_start: usize,
    scope_start: usize,
    scope_end: usize,
    alpha: String,
    structural: String,
    safe: bool,
    parameter: bool,
    lexical: bool,
}

#[derive(Default)]
struct LocalConstants {
    bindings: BTreeMap<String, Vec<LocalConstant>>,
}

impl LocalConstants {
    /// Collects only the module's own constants, for fingerprints that must not substitute a
    /// handler-local value. The declarations are walked with the handler as the scope anchor so
    /// registration and containment behave exactly as in `collect`.
    fn collect_module_scope(
        handler: Node<'_>,
        source: &[u8],
        declared: &BTreeMap<String, String>,
    ) -> Self {
        Self::collect_from(module_declarations(handler), handler, source, declared)
    }

    /// Collects the constants a handler body can resolve, including the module's own declarations.
    ///
    /// `behavior_signature` reads these bindings to recognise assertions and to resolve their
    /// expected values, so a proven module constant has to be visible here too. `duplicate-handler`
    /// compares `alpha_normalized` *and* `behavior_signature`, and seeding only one of them leaves
    /// two handlers that read the same value through differently named constants unequal.
    ///
    /// Do not remove the module seeding to fix an assertion-recognition regression: that was tried,
    /// and the cause was passing the constants to the serializer, which also routes every literal
    /// through the value-fingerprint branch. `collect_module_scope` is the narrower path that
    /// substitutes module values into a fingerprint without touching literal handling.
    fn collect(handler: Node<'_>, source: &[u8], declared: &BTreeMap<String, String>) -> Self {
        let mut seeds = vec![handler];
        seeds.extend(module_declarations(handler));
        Self::collect_from(seeds, handler, source, declared)
    }

    fn collect_from(
        seeds: Vec<Node<'_>>,
        handler: Node<'_>,
        source: &[u8],
        declared: &BTreeMap<String, String>,
    ) -> Self {
        let mut constants = Self::default();
        let mut ancestor = handler.parent();
        while let Some(node) = ancestor {
            if matches!(
                node.kind(),
                "class" | "class_declaration" | "abstract_class_declaration"
            ) {
                if let Some(name) = node.child_by_field_name("name") {
                    constants.insert_unresolved_shadow(
                        name,
                        source,
                        node.start_byte(),
                        node,
                        false,
                    );
                }
            }
            // A handler can close over a binding in an enclosing scope. That name is neither
            // handler-local nor module-scoped, so without recording it a module constant of the
            // same name would substitute over the captured value and fuse two handlers that read
            // different values. The value itself is not resolved here — only the fact that a nearer
            // binding exists, which is enough to decline substitution.
            //
            // Every binding form an enclosing scope can introduce is recorded, not just its
            // declarations: a parameter, a catch binding and a loop head all capture the same way,
            // and covering one form at a time leaves the next as a latent false duplicate.
            let captured = if is_function_like(node) {
                node.child_by_field_name("parameters")
                    .or_else(|| node.child_by_field_name("parameter"))
            } else {
                match node.kind() {
                    "catch_clause" => node.child_by_field_name("parameter"),
                    "for_in_statement" => node
                        .child_by_field_name("kind")
                        .and_then(|_| node.child_by_field_name("left")),
                    _ => None,
                }
            };
            if let Some(captured) = captured {
                constants.insert_unresolved_shadow(
                    captured,
                    source,
                    node.start_byte(),
                    node,
                    false,
                );
            }
            let mut cursor = node.walk();
            let children: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
            for child in children {
                let declaration = match child.kind() {
                    "lexical_declaration" | "variable_declaration" => Some(child),
                    "export_statement" => {
                        let mut inner = child.walk();
                        let exported: Vec<Node<'_>> = child.named_children(&mut inner).collect();
                        exported.into_iter().find(|node| {
                            matches!(node.kind(), "lexical_declaration" | "variable_declaration")
                        })
                    }
                    _ => None,
                };
                let Some(declaration) = declaration else {
                    continue;
                };
                let mut declarators = declaration.walk();
                let names: Vec<Node<'_>> = declaration
                    .named_children(&mut declarators)
                    .filter(|declarator| declarator.kind() == "variable_declarator")
                    .filter_map(|declarator| declarator.child_by_field_name("name"))
                    .collect();
                for name in names {
                    constants.insert_unresolved_shadow(
                        name,
                        source,
                        declaration.start_byte(),
                        node,
                        true,
                    );
                }
            }
            ancestor = node.parent();
        }
        let mut pending = Vec::new();
        let mut stack = seeds;
        while let Some(node) = stack.pop() {
            if is_function_like(node) {
                if let Some(parameters) = node
                    .child_by_field_name("parameters")
                    .or_else(|| node.child_by_field_name("parameter"))
                {
                    let mut names = Vec::new();
                    collect_parameter_bindings(parameters, source, &mut names);
                    for name in names {
                        constants
                            .bindings
                            .entry(name)
                            .or_default()
                            .push(LocalConstant {
                                declaration_start: node.start_byte(),
                                scope_start: node.start_byte(),
                                scope_end: node.end_byte(),
                                alpha: String::new(),
                                structural: String::new(),
                                safe: true,
                                parameter: true,
                                lexical: false,
                            });
                    }
                }
            }
            let declaration_shadow = match node.kind() {
                "class_declaration" | "abstract_class_declaration" => node
                    .child_by_field_name("name")
                    .zip(local_constant_scope(node.parent(), handler))
                    .map(|(name, scope)| (name, scope, node.start_byte(), true)),
                "function_declaration" | "generator_function_declaration" | "enum_declaration" => {
                    node.child_by_field_name("name")
                        .zip(local_constant_scope(node.parent(), handler))
                        .map(|(name, scope)| (name, scope, scope.start_byte(), false))
                }
                "function_expression" | "generator_function" | "class" => node
                    .child_by_field_name("name")
                    .map(|name| (name, node, node.start_byte(), false)),
                "catch_clause" => node
                    .child_by_field_name("parameter")
                    .map(|parameter| (parameter, node, node.start_byte(), false)),
                // `for (const x of xs)` binds `x` for the loop, but the grammar attaches that
                // binding to the loop itself rather than to a `variable_declarator`, so the
                // declarator branch below never sees it and an outer constant of the same name
                // stayed visible inside the body. Without a declaration keyword the left side is
                // an assignment target and introduces no binding at all.
                "for_in_statement" => node
                    .child_by_field_name("kind")
                    .zip(node.child_by_field_name("left"))
                    .and_then(|(keyword, left)| {
                        let lexical = matches!(node_text(keyword, source), "let" | "const");
                        // `var` is function-scoped and hoists past the loop; `let`/`const` belong
                        // to the loop alone.
                        let scope = if lexical {
                            Some(node)
                        } else {
                            local_constant_scope(node.parent(), handler)
                        };
                        scope.map(|scope| (left, scope, node.start_byte(), lexical))
                    }),
                _ => None,
            };
            if let Some((name, scope, declaration_start, lexical)) = declaration_shadow {
                constants.insert_unresolved_shadow(name, source, declaration_start, scope, lexical);
            }
            if node.kind() == "variable_declarator" {
                let declaration = node.parent();
                if let (Some(name), Some(scope)) = (
                    node.child_by_field_name("name"),
                    local_constant_scope(node.parent(), handler),
                ) {
                    // Register shadows before evaluating any initializer: a later lexical
                    // declaration must never expose an outer constant through its TDZ.
                    let mut names = Vec::new();
                    collect_parameter_bindings(name, source, &mut names);
                    for binding_name in names {
                        let lexical =
                            declaration.is_none_or(|d| d.kind() != "variable_declaration");
                        if !lexical
                            && node.child_by_field_name("value").is_none()
                            && constants
                                .bindings
                                .get(&binding_name)
                                .is_some_and(|entries| {
                                    entries.iter().any(|b| {
                                        b.scope_start == scope.start_byte()
                                            && b.scope_end == scope.end_byte()
                                    })
                                })
                        {
                            continue;
                        }
                        constants
                            .bindings
                            .entry(binding_name)
                            .or_default()
                            .push(LocalConstant {
                                declaration_start: node.start_byte(),
                                scope_start: scope.start_byte(),
                                scope_end: scope.end_byte(),
                                alpha: String::new(),
                                structural: String::new(),
                                safe: false,
                                parameter: false,
                                lexical,
                            });
                    }
                    // Destructuring is a shadow too, but is not a proven primitive constant.
                    if let Some(value) = node
                        .child_by_field_name("value")
                        .filter(|_| name.kind() == "identifier")
                    {
                        pending.push((
                            node_text(name, source).to_owned(),
                            node.start_byte(),
                            scope.start_byte(),
                            scope.end_byte(),
                            value,
                            declaration.is_some_and(is_const_declaration),
                        ));
                    }
                }
            }
            push_named_children_reverse(node, &mut stack);
        }
        // Declaration order makes earlier immutable constants available while serializing later
        // initializers. This preserves semantics through bounded alias chains without recursive
        // AST traversal; forward references remain unresolved because they are invalid at runtime.
        pending.sort_by_key(|(_, declaration_start, _, _, _, _)| *declaration_start);
        for (name, declaration_start, scope_start, scope_end, value, immutable) in pending {
            let safe = immutable
                && match value.kind() {
                    "string" | "number" | "true" | "false" | "null" | "undefined" => true,
                    "identifier" => constants
                        .binding(node_text(value, source), value)
                        .is_some_and(|binding| binding.safe),
                    _ => false,
                };
            let fingerprint = |mode| {
                // Reuse the value digest across aliases instead of hashing the alias spelling.
                if safe && value.kind() == "identifier" {
                    if let Some(resolved) = constants.resolve(node_text(value, source), value, mode)
                    {
                        return resolved.to_owned();
                    }
                }
                let serialized = if safe {
                    serialize_ast(value, source, declared, mode)
                } else {
                    serialize_assertion_value(value, source, declared, mode, &constants)
                };
                stable_fingerprint(&serialized)
            };
            let alpha = fingerprint(AstMode::Alpha);
            let structural = fingerprint(AstMode::Structural);
            constants.bindings.entry(name).and_modify(|entries| {
                if let Some(binding) = entries
                    .iter_mut()
                    .find(|binding| binding.declaration_start == declaration_start)
                {
                    *binding = LocalConstant {
                        declaration_start,
                        scope_start,
                        scope_end,
                        alpha,
                        structural,
                        safe,
                        parameter: false,
                        lexical: binding.lexical,
                    };
                }
            });
        }
        constants
    }

    fn insert_unresolved_shadow(
        &mut self,
        pattern: Node<'_>,
        source: &[u8],
        declaration_start: usize,
        scope: Node<'_>,
        lexical: bool,
    ) {
        let mut names = Vec::new();
        if pattern.kind() == "type_identifier" {
            push_identifier(&mut names, node_text(pattern, source));
        } else {
            collect_parameter_bindings(pattern, source, &mut names);
        }
        for name in names {
            self.bindings.entry(name).or_default().push(LocalConstant {
                declaration_start,
                scope_start: scope.start_byte(),
                scope_end: scope.end_byte(),
                alpha: String::new(),
                structural: String::new(),
                safe: false,
                parameter: false,
                lexical,
            });
        }
    }

    /// Returns `declared` extended with the values of every proven module constant.
    ///
    /// Only a binding with a proven value contributes, so a mutable or unresolved declaration keeps
    /// its identifier and stays distinguishing. Two handlers reading the same value through
    /// differently named constants therefore serialize identically.
    fn substituted_names(
        &self,
        declared: &BTreeMap<String, String>,
        handler: Node<'_>,
        mode: AstMode,
    ) -> BTreeMap<String, String> {
        let mut names = declared.clone();
        for name in self.bindings.keys() {
            // A handler-local binding of the same name shadows the module constant.
            if declared.contains_key(name) {
                continue;
            }
            // Take the nearest binding that spans the handler rather than any safe one. A
            // constant inside another declaration's initializer is not in scope at all, and a
            // binding captured from an enclosing scope is nearer than the module's, so only a
            // module constant that nothing shadows may substitute.
            if let Some(binding) = self.binding(name, handler).filter(|binding| binding.safe) {
                let value = match mode {
                    AstMode::Alpha | AstMode::Normalized => &binding.alpha,
                    AstMode::Structural => &binding.structural,
                };
                if !value.is_empty() {
                    names.insert(name.clone(), format!("const:{value}"));
                }
            }
        }
        names
    }

    fn resolve(&self, name: &str, use_site: Node<'_>, mode: AstMode) -> Option<&str> {
        let binding = self
            .binding(name, use_site)
            .filter(|binding| !binding.parameter)?;
        let fingerprint = match mode {
            AstMode::Alpha | AstMode::Normalized => binding.alpha.as_str(),
            AstMode::Structural => binding.structural.as_str(),
        };
        // Shadow placeholders have no resolved value; retain their identifier in the AST.
        (!fingerprint.is_empty()).then_some(fingerprint)
    }

    fn binding(&self, name: &str, use_site: Node<'_>) -> Option<&LocalConstant> {
        self.bindings
            .get(name)?
            .iter()
            .filter(|binding| {
                binding.scope_start <= use_site.start_byte()
                    && binding.scope_end >= use_site.end_byte()
            })
            .min_by(|left, right| {
                let left_width = left.scope_end.saturating_sub(left.scope_start);
                let right_width = right.scope_end.saturating_sub(right.scope_start);
                left_width.cmp(&right_width).then_with(|| {
                    let rank = |b: &LocalConstant| {
                        (
                            b.lexical || b.declaration_start <= use_site.start_byte(),
                            b.declaration_start,
                        )
                    };
                    rank(right).cmp(&rank(left))
                })
            })
            .filter(|binding| binding.declaration_start <= use_site.start_byte())
    }
}

fn is_function_like(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
    )
}

fn is_unresolved_assertion(event: &str) -> bool {
    matches!(event, UNRESOLVED_ASSERTION | DEFERRED_UNRESOLVED_ASSERTION)
}

fn has_function_parameters(node: Node<'_>) -> bool {
    node.child_by_field_name("parameters")
        .is_some_and(|parameters| parameters.named_child_count() > 0)
        || node.child_by_field_name("parameter").is_some()
}

fn is_const_declaration(declaration: Node<'_>) -> bool {
    declaration.kind() == "lexical_declaration"
        && (0..declaration.child_count()).any(|index| {
            declaration
                .child(index)
                .is_some_and(|child| !child.is_named() && child.kind() == "const")
        })
}

fn local_constant_scope<'tree>(
    mut node: Option<Node<'tree>>,
    handler: Node<'tree>,
) -> Option<Node<'tree>> {
    if node.is_some_and(|declaration| declaration.kind() == "variable_declaration") {
        return super::assertions::nearest_function_scope(node);
    }
    while let Some(candidate) = node {
        if candidate.id() == handler.id() {
            return Some(candidate);
        }
        if matches!(
            candidate.kind(),
            "statement_block"
                | "for_statement"
                | "for_in_statement"
                | "switch_body"
                | "catch_clause"
                | "class_static_block"
                | "internal_module"
                | "module"
                // Without this the walk runs past `program`, yields no scope, and a module-scoped
                // declarator is skipped entirely.
                | "program"
        ) {
            return Some(candidate);
        }
        if is_function_like(candidate) {
            return None;
        }
        node = candidate.parent();
    }
    None
}

fn serialize_ast(
    node: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    mode: AstMode,
) -> String {
    serialize_ast_with_constants(node, source, declared, mode, None, None)
}

/// Whether a decoded property name can also be written with dot access.
///
/// Only then do `obj['name']` and `obj.name` denote the same expression; any other name has no dot
/// spelling, so folding it into one would invent an equivalence.
fn is_identifier_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_alphabetic() || first == '_' || first == '$')
        && characters
            .all(|character| character.is_alphanumeric() || character == '_' || character == '$')
}

fn serialize_ast_with_constants(
    node: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    mode: AstMode,
    constants: Option<&LocalConstants>,
    assertions: Option<&AssertionBindings>,
) -> String {
    enum Event<'tree> {
        Visit(Node<'tree>, bool),
        Text(String),
        Close,
    }
    let mut output = String::new();
    let mut stack = vec![Event::Visit(node, false)];
    while let Some(event) = stack.pop() {
        match event {
            Event::Close => output.push(')'),
            Event::Text(text) => output.push_str(&text),
            Event::Visit(node, prefixed) => {
                if node.kind() == "comment" {
                    continue;
                }
                if prefixed {
                    output.push(' ');
                }
                if let Some((negated, matcher)) =
                    assertions.and_then(|bindings| bindings.asymmetric_matcher(node, source))
                {
                    output.push_str("(asymmetric_matcher:");
                    if negated {
                        output.push_str("not.");
                    }
                    output.push_str(&matcher);
                    if let Some(arguments) = node.child_by_field_name("arguments") {
                        stack.push(Event::Close);
                        stack.push(Event::Visit(arguments, true));
                    } else {
                        output.push(')');
                    }
                    continue;
                }
                if let Some(constants) = constants {
                    // Substitute at the use site so inline and local values retain identical shape.
                    let shorthand = node.kind() == "shorthand_property_identifier";
                    let resolved = (node.kind() == "identifier" || shorthand)
                        .then(|| constants.resolve(node_text(node, source), node, mode))
                        .flatten();
                    let literal = matches!(
                        node.kind(),
                        "string" | "number" | "true" | "false" | "null" | "undefined"
                    );
                    if resolved.is_some() || literal {
                        let fingerprint = resolved.map(str::to_owned).unwrap_or_else(|| {
                            stable_fingerprint(&serialize_ast(node, source, declared, mode))
                        });
                        if shorthand {
                            // A shorthand carries both a property key and a resolved value.
                            output.push_str(&format!("<key:{}>", node_text(node, source)));
                        }
                        output.push_str(&format!("<value:{fingerprint}>"));
                        continue;
                    }
                }
                if matches!(mode, AstMode::Structural)
                    && matches!(
                        node.kind(),
                        "string"
                            | "string_fragment"
                            | "number"
                            | "regex"
                            | "true"
                            | "false"
                            | "null"
                    )
                {
                    output.push('<');
                    output.push_str(node.kind());
                    output.push('>');
                    continue;
                }
                // `obj['name']` and `obj.name` read the same property, so a readable computed
                // form is serialized as the dot form and the two share a fingerprint. Only a
                // static string literal decoding to a valid identifier qualifies: that is the one
                // case where a dot spelling exists. A key such as `'not.resolves'` names a single
                // property and must not be folded into the chain `.not.resolves`.
                if node.kind() == "subscript_expression" {
                    if let Some((object, name)) = node
                        .child_by_field_name("object")
                        .zip(node.child_by_field_name("index"))
                        .and_then(|(object, index)| {
                            super::matcher::static_string_key(index, source)
                                .filter(|name| is_identifier_name(name))
                                .map(|name| (object, name))
                        })
                    {
                        // `a?.['b']` short-circuits where `a['b']` throws, so the optional link is
                        // part of the access and must survive canonicalisation. Emitting it in the
                        // shape the dotted form uses keeps `a?.['b']` equal to `a?.b` and distinct
                        // from `a.b`.
                        let mut cursor = node.walk();
                        let optional = node
                            .children(&mut cursor)
                            .any(|child| child.kind() == "optional_chain");
                        let link = if optional {
                            " (optional_chain ?.:?.)"
                        } else {
                            " .:."
                        };
                        output.push_str("(member_expression");
                        stack.push(Event::Close);
                        stack.push(Event::Text(format!("{link} property_identifier:{name}")));
                        stack.push(Event::Visit(object, true));
                        continue;
                    }
                }
                let mut cursor = node.walk();
                let children: Vec<_> = node
                    .children(&mut cursor)
                    .filter(|child| !child.is_extra() && child.kind() != "comment")
                    .collect();
                if children.is_empty() {
                    let text = node_text(node, source);
                    let value = match mode {
                        AstMode::Normalized => text,
                        AstMode::Alpha | AstMode::Structural
                            if matches!(
                                node.kind(),
                                "identifier" | "shorthand_property_identifier_pattern"
                            ) =>
                        {
                            declared.get(text).map_or(text, String::as_str)
                        }
                        AstMode::Alpha | AstMode::Structural => text,
                    };
                    output.push_str(node.kind());
                    output.push(':');
                    output.push_str(value);
                } else {
                    output.push('(');
                    output.push_str(node.kind());
                    stack.push(Event::Close);
                    for child in children.into_iter().rev() {
                        stack.push(Event::Visit(child, true));
                    }
                }
            }
        }
    }
    output
}

/// Maps functions declared inside the handler to their declaration, by name.
///
/// Only a plain `function` declaration is collected. A function reached through a variable can be
/// reassigned before the call, so treating it as the callee would assert behaviour the source does
/// not prove.
///
/// A name declared more than once maps to `None`. The lookup is by name alone, so a shadowed name
/// cannot be resolved to the right body here, and expanding the wrong one would fabricate executed
/// behaviour the handler never runs. Declining to expand only loses the expansion; inventing an
/// assertion changes the finding.
fn local_function_declarations<'tree>(
    handler: Node<'tree>,
    source: &[u8],
) -> BTreeMap<String, Option<Node<'tree>>> {
    let mut declarations: BTreeMap<String, Option<Node<'tree>>> = BTreeMap::new();
    let mut shadowed = BTreeSet::new();
    let mut stack = vec![handler];
    while let Some(node) = stack.pop() {
        // A `function_declaration` always carries a name, so the lookup is folded into the
        // condition rather than nested: an inner `if let` would leave a branch no input reaches.
        let declared = (node.id() != handler.id() && node.kind() == "function_declaration")
            .then(|| node.child_by_field_name("name"))
            .flatten();
        if let Some(name) = declared {
            declarations
                .entry(node_text(name, source).to_owned())
                .and_modify(|existing| *existing = None)
                .or_insert(Some(node));
        }
        // Any other construct that introduces the same name shadows the declaration at some
        // position, and a name-keyed lookup cannot tell where. This is deliberately inverted: rather
        // than enumerate the shadowing forms, which leaves the next syntax form to be discovered as
        // a defect, every named binding disqualifies the name and only a sole `function` declaration
        // stays expandable. Over-declining loses an expansion; expanding the wrong body invents
        // assertions the handler never runs.
        let bound = match node.kind() {
            "variable_declarator" => node.child_by_field_name("name"),
            "catch_clause" => node.child_by_field_name("parameter"),
            // A function declaration is a mutable binding: `check = () => ...` replaces the body
            // before the call reaches it, so the declaration no longer describes what runs.
            "assignment_expression" | "augmented_assignment_expression" => {
                node.child_by_field_name("left")
            }
            _ if is_function_like(node) => node
                .child_by_field_name("parameters")
                .or_else(|| node.child_by_field_name("parameter")),
            // `class check {}`, `enum check {}`, `namespace check {}` and any future declaration
            // form all bind their name at runtime.
            kind if kind.ends_with("_declaration") || kind.ends_with("_statement") => {
                node.child_by_field_name("name")
            }
            _ => node.child_by_field_name("name"),
        };
        if let Some(bound) = bound {
            // The declaration currently being recorded is not a shadow of itself.
            if node.kind() != "function_declaration" {
                let mut names = Vec::new();
                match bound.kind() {
                    // A class or enum name is a `type_identifier`, which the pattern collector does
                    // not treat as a binding, so name nodes are read directly.
                    "identifier" | "type_identifier" | "property_identifier" => {
                        names.push(node_text(bound, source).to_owned());
                    }
                    _ => collect_parameter_bindings(bound, source, &mut names),
                }
                shadowed.extend(names);
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
    for name in shadowed {
        if let Some(entry) = declarations.get_mut(&name) {
            *entry = None;
        }
    }
    declarations
}

fn behavior_signature(
    handler: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    assertions: &AssertionBindings,
    local_constants: &LocalConstants,
) -> Vec<String> {
    let mut signature = Vec::new();
    collect_behavior(
        handler,
        source,
        declared,
        assertions,
        local_constants,
        &mut signature,
    );
    signature
}

fn collect_behavior<'tree>(
    node: Node<'tree>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    assertions: &AssertionBindings,
    local_constants: &LocalConstants,
    output: &mut Vec<String>,
) {
    let root_id = node.id();
    let local_functions = local_function_declarations(node, source);
    let mut stack = vec![(node, false, false, false)];
    let push_children = |node: Node<'tree>,
                         deferred,
                         unresolved_callback_parameters,
                         expanded,
                         stack: &mut Vec<_>| {
        let start = stack.len();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push((child, deferred, unresolved_callback_parameters, expanded));
        }
        stack[start..].reverse();
    };
    while let Some((node, deferred, unresolved_callback_parameters, expanded)) = stack.pop() {
        let nested_function = node.id() != root_id && is_function_like(node);
        if nested_function {
            let mut parent = node.parent();
            // Argument values can contain callbacks inside objects, arrays, or expressions.
            // Do not cross into an enclosing function's arguments or unrelated local bodies.
            while let Some(ancestor) = parent {
                if ancestor.kind() == "arguments" {
                    break;
                }
                if ancestor.id() == root_id
                    || is_function_like(ancestor)
                    || matches!(ancestor.kind(), "statement_block" | "class_body")
                {
                    parent = None;
                    break;
                }
                parent = ancestor.parent();
            }
            if parent.is_none() {
                continue;
            }
        }
        // Retain syntactic call evidence in callbacks without promoting their assertions
        // to executed behavior. Dropping the body makes unrelated callbacks look identical.
        let deferred = deferred || nested_function;
        let parameterized_callback = nested_function && has_function_parameters(node);
        if parameterized_callback {
            // Callback arguments are not evaluated, so any runtime parameter can change the
            // callback's behavior even when its body contains no recognized assertion.
            output.push(DEFERRED_UNRESOLVED_ASSERTION.to_owned());
        }
        let unresolved_callback_parameters =
            unresolved_callback_parameters || parameterized_callback;
        match node.kind() {
            "call_expression" => {
                if let Some(event) =
                    assertion_behavior_event(node, source, declared, assertions, local_constants)
                {
                    if unresolved_callback_parameters {
                        // The callback-level marker already makes this handler non-comparable.
                    } else if deferred {
                        output.push(if event == UNRESOLVED_ASSERTION {
                            DEFERRED_UNRESOLVED_ASSERTION.to_owned()
                        } else {
                            format!("deferred-{event}")
                        });
                    } else {
                        output.push(event);
                    }
                    // Treat the complete assertion as one atomic behavior event. Its subject and
                    // expected values are already fingerprinted, so traversing its children would
                    // count subject calls such as `page.locator()` a second time and make overlap
                    // depend on incidental expression complexity.
                    continue;
                }
                if let Some(function) = node.child_by_field_name("function") {
                    let invoked = super::registrations::unwrap_registration_callee(function)
                        .unwrap_or(function);
                    if invoked.kind() == "generator_function" {
                        // Arguments run now; the body stays suspended. Parameter initialization
                        // is unresolved, as for other parameterized inline functions.
                        if has_function_parameters(invoked) {
                            output.push(if deferred {
                                DEFERRED_UNRESOLVED_ASSERTION.to_owned()
                            } else {
                                UNRESOLVED_ASSERTION.to_owned()
                            });
                        }
                        if let Some(evaluated) = node.child_by_field_name("arguments") {
                            push_children(
                                evaluated,
                                deferred,
                                unresolved_callback_parameters,
                                expanded,
                                &mut stack,
                            );
                        }
                        continue;
                    }
                    if matches!(invoked.kind(), "arrow_function" | "function_expression") {
                        // Parameter substitution is not evaluated; do not invent equal values.
                        if has_function_parameters(invoked) {
                            output.push(if deferred {
                                DEFERRED_UNRESOLVED_ASSERTION.to_owned()
                            } else {
                                UNRESOLVED_ASSERTION.to_owned()
                            });
                            if let Some(arguments) = node.child_by_field_name("arguments") {
                                push_children(
                                    arguments,
                                    deferred,
                                    unresolved_callback_parameters,
                                    expanded,
                                    &mut stack,
                                );
                            }
                            continue;
                        }
                        // Record the executed body, not an extra generic wrapper-call event.
                        push_children(
                            invoked,
                            deferred,
                            unresolved_callback_parameters,
                            expanded,
                            &mut stack,
                        );
                        if let Some(arguments) = node.child_by_field_name("arguments") {
                            push_children(
                                arguments,
                                deferred,
                                unresolved_callback_parameters,
                                expanded,
                                &mut stack,
                            );
                        }
                        continue;
                    }
                    // A function declared in the handler and then called does execute, so its
                    // assertions are the handler's behaviour. Expanding only one level keeps this
                    // bounded and terminates on recursion; a deeper call still records the generic
                    // call event, as before.
                    // `invoked` has already been unwrapped through parentheses, `as`, and the
                    // non-null assertion, so `(check)()` resolves the same declaration `check()`
                    // does. Reading the original callee here would silently skip those spellings.
                    let expansion = (!expanded && invoked.kind() == "identifier")
                        .then(|| local_functions.get(node_text(invoked, source)))
                        .flatten()
                        .and_then(|declaration| *declaration)
                        // The declaration must be able to reach this call: a `function` nested in
                        // another function or a block is not visible outside it, so expanding it
                        // here would attribute behaviour the call never reaches.
                        .filter(|declaration| {
                            declaration.parent().is_some_and(|scope| {
                                scope.start_byte() <= node.start_byte()
                                    && scope.end_byte() >= node.end_byte()
                            })
                        });
                    if let Some(target) = expansion {
                        if has_function_parameters(target) {
                            // Arguments are not substituted, so the body's values are unproven.
                            output.push(if deferred {
                                DEFERRED_UNRESOLVED_ASSERTION.to_owned()
                            } else {
                                UNRESOLVED_ASSERTION.to_owned()
                            });
                        } else {
                            push_children(
                                target,
                                deferred,
                                unresolved_callback_parameters,
                                true,
                                &mut stack,
                            );
                        }
                        if let Some(arguments) = node.child_by_field_name("arguments") {
                            push_children(
                                arguments,
                                deferred,
                                unresolved_callback_parameters,
                                expanded,
                                &mut stack,
                            );
                        }
                        continue;
                    }
                    output.push(call_behavior_event(function, source, declared));
                }
            }
            "if_statement" | "switch_statement" | "for_statement" | "for_in_statement"
            | "while_statement" | "do_statement" | "return_statement" | "throw_statement" => {
                output.push(node.kind().to_owned())
            }
            _ => {}
        }
        push_children(
            node,
            deferred,
            unresolved_callback_parameters,
            expanded,
            &mut stack,
        );
    }
}

/// Describes an assertion by its terminal matcher, modifiers, and asserted subject.
///
/// A generic call shape such as `expect#toHaveValue` is not enough to establish shared
/// behavior: assertions against two different controls are distinct, and `.not` reverses the
/// meaning of an otherwise identical assertion. Expected matcher arguments remain semantic so
/// assertions for conflicting values do not collapse into the same near-duplicate behavior.
/// Literal normalization remains confined to the separate structural fingerprint.
fn assertion_behavior_event(
    call: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    assertions: &AssertionBindings,
    local_constants: &LocalConstants,
) -> Option<String> {
    let (receiver, method) = assertion_property(call.child_by_field_name("function")?, source)?;

    let mut modifiers = Vec::new();
    let invocation = find_expect_invocation(receiver, source, &mut modifiers, assertions)?;
    modifiers.reverse();
    let arguments = invocation.child_by_field_name("arguments")?;
    let subject = arguments.named_child(0)?;
    // Structural mode retains member and call identities while normalizing local bindings and
    // literal values. This distinguishes `form.title` from `form.date` without hiding the
    // existing parameterization signal for `page.locator("#one")` versus `"#two"`.
    let subject = serialize_assertion_value(
        subject,
        source,
        declared,
        AstMode::Structural,
        local_constants,
    );
    let expected = call.child_by_field_name("arguments")?;
    // An external name is not proof of an equal value across files. Preserve extraction and
    // matcher checks, but decline handler equivalence rather than invent module resolution.
    let mut pending = vec![expected];
    let mut cursor = arguments.walk();
    let configuration: Vec<_> = arguments.named_children(&mut cursor).skip(1).collect();
    pending.extend(configuration.iter().copied());
    while let Some(value) = pending.pop() {
        // Only known builders on a trusted assertion factory are syntax, not external
        // values. Their arguments still need the same conservative value validation.
        if assertions.is_asymmetric_matcher(value, source) {
            if let Some(arguments) = value.child_by_field_name("arguments") {
                pending.push(arguments);
            }
            continue;
        }
        if matches!(value.kind(), "identifier" | "shorthand_property_identifier")
            && !local_constants
                .binding(node_text(value, source), value)
                .is_some_and(|binding| binding.safe)
        {
            return Some(UNRESOLVED_ASSERTION.to_owned());
        }
        push_named_children_reverse(value, &mut pending);
    }
    // Alpha mode canonicalizes parameter and local names while retaining literal values and
    // member identities. The whole arguments node is included to preserve matcher arity.
    let mut expected = serialize_assertion_expected(
        expected,
        source,
        declared,
        AstMode::Alpha,
        local_constants,
        assertions,
    );
    // Factory options affect execution, unlike structural literal normalization.
    for option in configuration {
        expected.push_str("\0factory-option:");
        expected.push_str(&serialize_assertion_value(
            option,
            source,
            declared,
            AstMode::Alpha,
            local_constants,
        ));
    }
    let qualifier = if modifiers.is_empty() {
        "expect".to_owned()
    } else {
        format!("expect.{}", modifiers.join("."))
    };
    Some(format!(
        "assert:{qualifier}#{}:{}:{}",
        encode_event_component(method),
        stable_fingerprint(&subject),
        stable_fingerprint(&expected)
    ))
}

fn serialize_assertion_value(
    node: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    mode: AstMode,
    local_constants: &LocalConstants,
) -> String {
    serialize_ast_with_constants(node, source, declared, mode, Some(local_constants), None)
}

fn serialize_assertion_expected(
    node: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    mode: AstMode,
    local_constants: &LocalConstants,
    assertions: &AssertionBindings,
) -> String {
    serialize_ast_with_constants(
        node,
        source,
        declared,
        mode,
        Some(local_constants),
        Some(assertions),
    )
}

/// Resolves a property access spelled either `object.name` or `object["name"]` to the same
/// receiver and name, so a statically computed key means what the dotted key means at every
/// position of an assertion chain. This is the resolver the factory and asymmetric-builder
/// positions already use; routing the chain walk through it keeps the supported spellings in one
/// place. A key that is not a static string yields `None`, so a runtime-selected property keeps
/// its conservative treatment instead of gaining trust it cannot prove.
/// Encodes a property name as a component of an assertion event.
///
/// Event components are joined with `.`, `#` and `:`, so a decoded string key containing one of
/// them would be indistinguishable from the join itself: `expect(x)['not.resolves']` would
/// otherwise produce the event that `expect(x).not.resolves` produces.
///
/// Only a decoded key is escaped. An identifier cannot contain those delimiters, and escaping it
/// too would make a name written with a unicode escape collide with a key that genuinely contains
/// a backslash — `x.foo\u0062ar` reads `foobar`, while `x['foo\\u0062ar']` reads a different
/// property entirely.
fn encode_event_component(name: Cow<'_, str>) -> Cow<'_, str> {
    const DELIMITERS: [char; 4] = ['\\', '.', '#', ':'];
    let Cow::Owned(decoded) = name else {
        return name;
    };
    if !decoded.contains(DELIMITERS) {
        return Cow::Owned(decoded);
    }
    let mut escaped = String::with_capacity(decoded.len() + 8);
    for character in decoded.chars() {
        if DELIMITERS.contains(&character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    Cow::Owned(escaped)
}

fn assertion_property<'tree, 'source>(
    node: Node<'tree>,
    source: &'source [u8],
) -> Option<(Node<'tree>, Cow<'source, str>)> {
    match registration_callee(node, source)? {
        RegistrationCallee::Property { object, name } => Some((object, name)),
        RegistrationCallee::Identifier(_) => None,
    }
}

fn find_expect_invocation<'tree>(
    node: Node<'tree>,
    source: &[u8],
    modifiers: &mut Vec<String>,
    assertions: &AssertionBindings,
) -> Option<Node<'tree>> {
    let mut current = node;
    for _ in 0..MAX_ASSERTION_CHAIN_DEPTH {
        match current.kind() {
            "call_expression" => {
                let function = super::registrations::unwrap_registration_callee(
                    current.child_by_field_name("function")?,
                )?;
                if assertions.is_factory(function, source) {
                    return Some(current);
                }
                if let Some((object, option)) = assertion_property(function, source) {
                    if assertions.is_factory(object, source)
                        && matches!(option.as_ref(), "soft" | "poll")
                    {
                        modifiers.push(encode_event_component(option).into_owned());
                        return Some(current);
                    }
                }
                return None;
            }
            "member_expression" | "subscript_expression" => {
                let (object, modifier) = assertion_property(current, source)?;
                modifiers.push(encode_event_component(modifier).into_owned());
                current = object;
            }
            "parenthesized_expression"
            | "await_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression" => {
                current = current.named_child(0)?;
            }
            "type_assertion" => {
                let last = u32::try_from(current.named_child_count().checked_sub(1)?).ok()?;
                current = current.named_child(last)?;
            }
            _ => return None,
        }
    }
    None
}

fn call_behavior_event(
    function: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
) -> String {
    if function.kind() == "identifier" {
        let name = node_text(function, source);
        return format!("call:{}", declared.get(name).map_or(name, String::as_str));
    }
    // Both spellings of one property name the same method, so `page['locator']()` records the
    // event `page.locator()` records. `assertion_property` resolves only a static string literal,
    // so a runtime key still falls through to the structural form below.
    if let Some((object, property)) = assertion_property(function, source) {
        if let Some(receiver) = static_call_receiver(object, source, declared) {
            return format!(
                "call:{receiver}#{}",
                encode_event_component(property.clone())
            );
        }
    }
    format!(
        "call:{}",
        serialize_ast(function, source, declared, AstMode::Structural)
    )
}

fn static_call_receiver(
    node: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
) -> Option<String> {
    match node.kind() {
        "identifier" => {
            let name = node_text(node, source);
            Some(declared.get(name).map_or(name, String::as_str).to_owned())
        }
        "this" | "super" => Some(node_text(node, source).to_owned()),
        "parenthesized_expression" | "await_expression" => {
            static_call_receiver(node.named_child(0)?, source, declared)
        }
        "member_expression" | "subscript_expression" => {
            let (object, property) = assertion_property(node, source)?;
            let object = static_call_receiver(object, source, declared)?;
            Some(format!("{object}.{}", encode_event_component(property)))
        }
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            if let Some((object, _)) = assertion_property(function, source) {
                static_call_receiver(object, source, declared)
            } else if function.kind() == "identifier" {
                let name = node_text(function, source);
                Some(declared.get(name).map_or(name, String::as_str).to_owned())
            } else {
                None
            }
        }
        _ => None,
    }
}
