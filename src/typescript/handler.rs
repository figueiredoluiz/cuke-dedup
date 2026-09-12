use super::assertions::AssertionBindings;
use super::ast::push_named_children_reverse;
use super::node_text;
use crate::model::{stable_fingerprint, HandlerFingerprint};
use std::collections::BTreeMap;
use tree_sitter::Node;

const MAX_ASSERTION_CHAIN_DEPTH: usize = 16;

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

    Some(HandlerFingerprint {
        exact: stable_fingerprint(&exact),
        normalized: stable_fingerprint(&normalized),
        alpha_normalized: stable_fingerprint(&alpha),
        structural: stable_fingerprint(&structural),
        behavior_signature: signature,
        source_snippet: bounded_source_snippet(&source_snippet),
        comparable: true,
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
    let mut normalized = serialize_ast(handler, source, declared, AstMode::Normalized);
    let mut alpha = serialize_ast(handler, source, declared, AstMode::Alpha);
    let mut structural = serialize_ast(handler, source, declared, AstMode::Structural);
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
}

#[derive(Default)]
struct LocalConstants {
    bindings: BTreeMap<String, Vec<LocalConstant>>,
}

impl LocalConstants {
    fn collect(handler: Node<'_>, source: &[u8], declared: &BTreeMap<String, String>) -> Self {
        let mut constants = Self::default();
        let mut pending = Vec::new();
        let mut stack = vec![handler];
        while let Some(node) = stack.pop() {
            if node.id() != handler.id() && is_function_like(node) {
                continue;
            }
            if node.kind() == "variable_declarator" {
                let declaration = node.parent();
                if declaration.is_some_and(is_const_declaration) {
                    if let (Some(name), Some(value), Some(scope)) = (
                        node.child_by_field_name("name")
                            .filter(|name| name.kind() == "identifier"),
                        node.child_by_field_name("value"),
                        local_constant_scope(node.parent(), handler),
                    ) {
                        pending.push((
                            node_text(name, source).to_owned(),
                            node.start_byte(),
                            scope.start_byte(),
                            scope.end_byte(),
                            value,
                        ));
                    }
                }
            }
            push_named_children_reverse(node, &mut stack);
        }
        // Declaration order makes earlier immutable constants available while serializing later
        // initializers. This preserves semantics through bounded alias chains without recursive
        // AST traversal; forward references remain unresolved because they are invalid at runtime.
        pending.sort_by_key(|(_, declaration_start, _, _, _)| *declaration_start);
        for (name, declaration_start, scope_start, scope_end, value) in pending {
            let alpha = stable_fingerprint(&serialize_assertion_value(
                value,
                source,
                declared,
                AstMode::Alpha,
                &constants,
            ));
            let structural = stable_fingerprint(&serialize_assertion_value(
                value,
                source,
                declared,
                AstMode::Structural,
                &constants,
            ));
            constants
                .bindings
                .entry(name)
                .or_default()
                .push(LocalConstant {
                    declaration_start,
                    scope_start,
                    scope_end,
                    alpha,
                    structural,
                });
        }
        constants
    }

    fn resolve(&self, name: &str, use_site: Node<'_>, mode: AstMode) -> Option<&str> {
        self.bindings
            .get(name)?
            .iter()
            .filter(|binding| {
                binding.declaration_start <= use_site.start_byte()
                    && binding.scope_start <= use_site.start_byte()
                    && binding.scope_end >= use_site.end_byte()
            })
            .min_by(|left, right| {
                let left_width = left.scope_end.saturating_sub(left.scope_start);
                let right_width = right.scope_end.saturating_sub(right.scope_start);
                left_width
                    .cmp(&right_width)
                    .then_with(|| right.declaration_start.cmp(&left.declaration_start))
            })
            .map(|binding| match mode {
                AstMode::Alpha => binding.alpha.as_str(),
                AstMode::Structural => binding.structural.as_str(),
                // Assertion values currently use only semantic modes. Returning the normalized
                // alpha form keeps this helper total if another caller is added later.
                AstMode::Normalized => binding.alpha.as_str(),
            })
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
    enum Event<'tree> {
        Visit(Node<'tree>, bool),
        Close,
    }
    let mut output = String::new();
    let mut stack = vec![Event::Visit(node, false)];
    while let Some(event) = stack.pop() {
        match event {
            Event::Close => output.push(')'),
            Event::Visit(node, prefixed) => {
                if node.kind() == "comment" {
                    continue;
                }
                if prefixed {
                    output.push(' ');
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

fn collect_behavior(
    node: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    assertions: &AssertionBindings,
    local_constants: &LocalConstants,
    output: &mut Vec<String>,
) {
    let root_id = node.id();
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        // Match constant collection: nested functions have their own bindings and execution.
        if node.id() != root_id && is_function_like(node) {
            continue;
        }
        match node.kind() {
            "call_expression" => {
                if let Some(event) =
                    assertion_behavior_event(node, source, declared, assertions, local_constants)
                {
                    output.push(event);
                    // Treat the complete assertion as one atomic behavior event. Its subject and
                    // expected values are already fingerprinted, so traversing its children would
                    // count subject calls such as `page.locator()` a second time and make overlap
                    // depend on incidental expression complexity.
                    continue;
                }
                if let Some(function) = node.child_by_field_name("function") {
                    output.push(call_behavior_event(function, source, declared));
                }
            }
            "if_statement" | "switch_statement" | "for_statement" | "for_in_statement"
            | "while_statement" | "do_statement" | "return_statement" | "throw_statement" => {
                output.push(node.kind().to_owned())
            }
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
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
    let function = call.child_by_field_name("function")?;
    if function.kind() != "member_expression" {
        return None;
    }
    let method = function.child_by_field_name("property")?;

    let mut modifiers = Vec::new();
    let invocation = find_expect_invocation(
        function.child_by_field_name("object")?,
        source,
        &mut modifiers,
        assertions,
    )?;
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
    // Alpha mode canonicalizes parameter and local names while retaining literal values and
    // member identities. The whole arguments node is included to preserve matcher arity.
    let expected =
        serialize_assertion_value(expected, source, declared, AstMode::Alpha, local_constants);
    let qualifier = if modifiers.is_empty() {
        "expect".to_owned()
    } else {
        format!("expect.{}", modifiers.join("."))
    };
    Some(format!(
        "assert:{qualifier}#{}:{}:{}",
        node_text(method, source),
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
    let mut serialized = serialize_ast(node, source, declared, mode);
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if matches!(
            current.kind(),
            "identifier" | "shorthand_property_identifier"
        ) {
            if let Some(initializer) =
                local_constants.resolve(node_text(current, source), current, mode)
            {
                serialized.push_str("\0const:");
                serialized.push_str(initializer);
            }
            continue;
        }
        push_named_children_reverse(current, &mut stack);
    }
    serialized
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
                let function = current.child_by_field_name("function")?;
                if assertions.is_factory(function, source) {
                    return Some(current);
                }
                if function.kind() == "member_expression" {
                    let object = function.child_by_field_name("object")?;
                    let property = function.child_by_field_name("property")?;
                    if assertions.is_factory(object, source)
                        && matches!(node_text(property, source), "soft" | "poll")
                    {
                        modifiers.push(node_text(property, source).to_owned());
                        return Some(current);
                    }
                }
                return None;
            }
            "member_expression" => {
                let property = current.child_by_field_name("property")?;
                modifiers.push(node_text(property, source).to_owned());
                current = current.child_by_field_name("object")?;
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
    if function.kind() == "member_expression" {
        if let (Some(object), Some(property)) = (
            function.child_by_field_name("object"),
            function.child_by_field_name("property"),
        ) {
            if let Some(receiver) = static_call_receiver(object, source, declared) {
                return format!("call:{receiver}#{}", node_text(property, source));
            }
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
        "member_expression" => {
            let object =
                static_call_receiver(node.child_by_field_name("object")?, source, declared)?;
            let property = node.child_by_field_name("property")?;
            Some(format!("{object}.{}", node_text(property, source)))
        }
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            if function.kind() == "member_expression" {
                static_call_receiver(function.child_by_field_name("object")?, source, declared)
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
