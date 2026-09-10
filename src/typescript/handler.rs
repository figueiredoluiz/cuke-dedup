use super::node_text;
use crate::model::{stable_fingerprint, HandlerFingerprint};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

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
) -> HandlerFingerprint {
    let raw = node_text(handler, source);
    let declared = declared_identifiers(handler, source);
    let mut exact = raw.to_owned();
    let mut normalized = serialize_ast(handler, source, &declared, AstMode::Normalized);
    let mut alpha = serialize_ast(handler, source, &declared, AstMode::Alpha);
    let mut structural = serialize_ast(handler, source, &declared, AstMode::Structural);
    let mut signature = behavior_signature(handler, source, &declared);
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
        signature.extend(behavior_signature(arguments, source, &no_declarations));
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
    let mut identifiers = BTreeSet::new();
    if matches!(
        handler.kind(),
        "function_declaration" | "generator_function_declaration"
    ) {
        if let Some(name) = handler.child_by_field_name("name") {
            collect_identifier_text(name, source, &mut identifiers);
        }
    }
    if let Some(parameters) = handler.child_by_field_name("parameters") {
        collect_identifier_text(parameters, source, &mut identifiers);
    }
    collect_declared_locals(handler, source, &mut identifiers);
    identifiers
        .into_iter()
        .enumerate()
        .map(|(index, name)| (name, format!("v{index}")))
        .collect()
}

fn collect_declared_locals(node: Node<'_>, source: &[u8], output: &mut BTreeSet<String>) {
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

fn collect_identifier_text(node: Node<'_>, source: &[u8], output: &mut BTreeSet<String>) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if matches!(
            node.kind(),
            "identifier" | "shorthand_property_identifier_pattern"
        ) {
            output.insert(node_text(node, source).to_owned());
        }
        push_named_children_reverse(node, &mut stack);
    }
}

#[derive(Clone, Copy)]
enum AstMode {
    Normalized,
    Alpha,
    Structural,
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
) -> Vec<String> {
    let mut signature = Vec::new();
    collect_behavior(handler, source, declared, &mut signature);
    signature
}

fn collect_behavior(
    node: Node<'_>,
    source: &[u8],
    declared: &BTreeMap<String, String>,
    output: &mut Vec<String>,
) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "call_expression" => {
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
            if let Some(receiver) = static_call_receiver(object, source) {
                return format!("call:{receiver}#{}", node_text(property, source));
            }
        }
    }
    format!(
        "call:{}",
        serialize_ast(function, source, declared, AstMode::Structural)
    )
}

fn static_call_receiver(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" | "this" | "super" => Some(node_text(node, source).to_owned()),
        "parenthesized_expression" | "await_expression" => {
            static_call_receiver(node.named_child(0)?, source)
        }
        "member_expression" => {
            let object = static_call_receiver(node.child_by_field_name("object")?, source)?;
            let property = node.child_by_field_name("property")?;
            Some(format!("{object}.{}", node_text(property, source)))
        }
        "call_expression" => {
            let function = node.child_by_field_name("function")?;
            if function.kind() == "member_expression" {
                static_call_receiver(function.child_by_field_name("object")?, source)
            } else if function.kind() == "identifier" {
                Some(node_text(function, source).to_owned())
            } else {
                None
            }
        }
        _ => None,
    }
}

fn push_named_children_reverse<'tree>(node: Node<'tree>, stack: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    stack.extend(children.into_iter().rev());
}
