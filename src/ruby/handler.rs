//! Exact Ruby handler trees with explicit lexical capture identity.
use super::{descendants, text};
use crate::model::{BehaviorEvent, HandlerFingerprint, HandlerSemantics};
use std::borrow::Cow;
use tree_sitter::Node;

pub(super) fn fingerprint(
    block: Node<'_>,
    root: Node<'_>,
    source: &str,
    assertions: Option<&super::assertions::AssertionBindings>,
) -> HandlerFingerprint {
    let nodes = descendants(block);
    let bindings = super::bindings::Bindings::collect(block, root, source);
    let comparable = !bindings.uncertain
        && !nodes.iter().any(|node| {
            matches!(
                node.kind(),
                "method"
                    | "singleton_method"
                    | "class"
                    | "module"
                    | "singleton_class"
                    | "heredoc_body"
                    | "heredoc_beginning"
                    | "optional_parameter"
                    | "keyword_parameter"
                    | "ERROR"
            )
        });
    let (tokens, alpha) = syntax_tokens(block, source, &bindings);
    let structural = parameterized_calls(block, source, &bindings);
    let captures = &bindings.captures;
    let mut behavior_signature = if captures.is_empty() {
        vec![]
    } else {
        vec![BehaviorEvent::Method("ruby:lexical-file".to_owned())]
    };
    if let Some(assertions) = assertions {
        behavior_signature.extend(assertions.events(block, source, &bindings));
    }
    let alpha_normalized =
        serde_json::to_string(&(alpha, &captures)).expect("binding tokens serialize");
    let exact = if captures.is_empty() {
        serde_json::to_string(&tokens)
    } else {
        serde_json::to_string(&(tokens, captures))
    }
    .expect("string tokens serialize");
    let trivial = bindings.calls.is_empty()
        && !nodes
            .iter()
            .any(|node| matches!(node.kind(), "call" | "assignment" | "operator_assignment"));
    HandlerSemantics {
        exact: exact.clone(),
        normalized: exact.clone(),
        alpha: alpha_normalized,
        structural: structural.unwrap_or(exact),
        events: behavior_signature,
        source_snippet: text(block, source).chars().take(2000).collect(),
        comparable,
        trivial,
    }
    .finish(str::to_owned)
}

/// Abstract only whole straight-line handlers; any unmodeled effect preserves exact identity.
fn parameterized_calls(
    block: Node<'_>,
    source: &str,
    bindings: &super::bindings::Bindings,
) -> Option<String> {
    if block.child_by_field_name("parameters").is_some() {
        return None;
    }
    let body = block.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let mut calls = Vec::new();
    let mut literals = 0;
    for call in body.named_children(&mut cursor).filter(|n| !n.is_extra()) {
        if call.kind() != "call" || call.child_by_field_name("block").is_some() {
            return None;
        }
        let receiver = call.child_by_field_name("receiver")?;
        if !matches!(
            receiver.kind(),
            "identifier" | "self" | "instance_variable" | "global_variable" | "constant"
        ) {
            return None;
        }
        let mut arguments = Vec::new();
        if let Some(args) = call.child_by_field_name("arguments") {
            let mut cursor = args.walk();
            for arg in args.named_children(&mut cursor).filter(|n| !n.is_extra()) {
                match arg.kind() {
                    "string"
                        if !descendants(arg).iter().any(|n| n.kind() == "interpolation")
                            && super::literal_string(text(arg, source)).is_some() => {}
                    "integer" | "float" => {}
                    _ => return None,
                }
                arguments.push(arg.kind());
                literals += 1;
            }
        }
        calls.push((
            receiver.kind(),
            bindings
                .names
                .get(&receiver.id())
                .map_or(text(receiver, source), String::as_str),
            text(call.child_by_field_name("operator")?, source),
            text(call.child_by_field_name("method")?, source),
            arguments,
        ));
    }
    (literals > 0).then(|| {
        serde_json::to_string(&("ruby:literal-calls", calls, &bindings.captures))
            .expect("call structure serializes")
    })
}

// Preserve topology as well as leaves; Ruby whitespace can alter call nesting.
type SyntaxTokens<'tree, 'source> = Vec<(&'tree str, Cow<'source, str>)>;
pub(super) fn syntax_tokens<'tree, 'source>(
    root: Node<'tree>,
    source: &'source str,
    bindings: &super::bindings::Bindings,
) -> (SyntaxTokens<'tree, 'source>, SyntaxTokens<'tree, 'source>) {
    // Preserve tree structure: Ruby whitespace can change call nesting without changing leaves.
    let mut tokens = Vec::new();
    let mut alpha = Vec::new();
    let mut pending = vec![(root, false)];
    while let Some((node, closing)) = pending.pop() {
        if node.kind() == "comment"
            || (node
                .parent()
                .is_some_and(|parent| matches!(parent.kind(), "block" | "do_block"))
                && matches!(node.kind(), "{" | "}" | "do" | "end"))
        {
            continue;
        }
        let kind = match node.kind() {
            "block" | "do_block" => "block",
            "block_body" | "body_statement" => "body",
            other => other,
        };
        if closing {
            tokens.push((")", Cow::Borrowed("")));
            alpha.push((")", Cow::Borrowed("")));
            continue;
        }
        if node.kind() == "line"
            || (node.kind() == "identifier" && text(node, source) == "__LINE__")
        {
            tokens.push((
                node.kind(),
                Cow::Owned((node.start_position().row + 1).to_string()),
            ));
            alpha.push(tokens.last().unwrap().clone());
        } else if node.child_count() == 0 {
            tokens.push((kind, Cow::Borrowed(text(node, source))));
            alpha.push((
                kind,
                bindings.names.get(&node.id()).map_or_else(
                    || Cow::Borrowed(text(node, source)),
                    |name| Cow::Owned(name.clone()),
                ),
            ));
        } else {
            tokens.push((kind, Cow::Borrowed("(")));
            alpha.push((kind, Cow::Borrowed("(")));
            pending.push((node, true));
            let mut cursor = node.walk();
            let children: Vec<_> = node.children(&mut cursor).collect();
            pending.extend(children.into_iter().rev().map(|node| (node, false)));
        }
    }
    (tokens, alpha)
}
