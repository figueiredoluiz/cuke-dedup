//! Exact Ruby handler trees with explicit lexical capture identity.
use super::{descendants, text};
use crate::model::{BehaviorEvent, HandlerFingerprint, HandlerSemantics};
use std::borrow::Cow;
use std::collections::BTreeSet;
use tree_sitter::Node;

/// Builds handler identity from bound syntax, captures, and assertion effects.
pub(super) fn fingerprint(
    block: Node<'_>,
    root: Node<'_>,
    source: &str,
    assertions: Option<&super::assertions::AssertionBindings>,
) -> HandlerFingerprint {
    let nodes = descendants(block);
    let bindings = assertions.map_or_else(
        || super::bindings::Bindings::collect(block, root, source),
        |assertions| assertions.bindings(block, root, source),
    );
    let comparable = !bindings.uncertain
        && !assertions.is_some_and(|assertions| assertions.known_ineligible(block, root, source))
        && !nodes.iter().any(|node| {
            (block.kind() == "method"
                && matches!(
                    node.kind(),
                    "self" | "instance_variable" | "class_variable" | "global_variable"
                )
                && !assertions.is_some_and(|assertions| assertions.known_field(*node)))
                || (node != &block
                    && matches!(
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
                    ))
        });
    let (tokens, alpha) = syntax_tokens(block, source, &bindings);
    let captures = &bindings.captures;
    let exact = if captures.is_empty() {
        serde_json::to_string(&tokens)
    } else {
        serde_json::to_string(&(tokens, captures))
    }
    .expect("string tokens serialize");
    // Literal abstraction needs the complete operation stream that configured assertion
    // evidence computes; ordinary handlers without it keep exact identity.
    let structural = parameterized_calls(block, source, &bindings)
        .or_else(|| {
            assertions
                .and_then(|assertions| assertions.ordinary_structural(block, source, &bindings))
        })
        .or_else(|| assertions.map(|_| structural_tokens(&alpha, captures)))
        .unwrap_or_else(|| exact.clone());
    let mut behavior_signature = if bindings.lexical {
        vec![BehaviorEvent::Method("ruby:lexical-file".to_owned())]
    } else {
        vec![]
    };
    if let Some(assertions) = assertions {
        let events = assertions.events(block, source, &bindings);
        let assertion_evidence = events
            .iter()
            .any(|event| matches!(event, BehaviorEvent::Assertion { .. }));
        behavior_signature.extend(events);
        if assertion_evidence {
            behavior_signature.push(BehaviorEvent::Method("ruby:complete-events".to_owned()));
        }
    }
    let trivial = bindings.calls.is_empty()
        && !nodes
            .iter()
            .any(|node| matches!(node.kind(), "call" | "assignment" | "operator_assignment"));
    let alpha_normalized =
        serde_json::to_string(&(alpha, &captures)).expect("binding tokens serialize");
    HandlerSemantics {
        exact: exact.clone(),
        normalized: exact.clone(),
        alpha: alpha_normalized,
        structural,
        events: behavior_signature,
        source_snippet: text(block, source).chars().take(2000).collect(),
        comparable,
        trivial,
    }
    .finish(str::to_owned)
}

/// Abstract only whole straight-line handlers; any unmodeled effect preserves exact identity.
pub(super) fn parameterized_calls(
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

/// Abstracts literal values out of alpha tokens; behaviour events keep the values.
fn structural_tokens(alpha: &SyntaxTokens<'_, '_>, captures: &BTreeSet<String>) -> String {
    let abstracted: Vec<(&str, &str)> = alpha
        .iter()
        .map(|(kind, value)| match *kind {
            "integer" | "float" | "string_content" | "escape_sequence" => (*kind, "<literal>"),
            _ => value
                .strip_prefix("const:")
                .and_then(|rest| rest.split_once(':'))
                .map_or((*kind, value.as_ref()), |(kind, _)| ("constant", kind)),
        })
        .collect();
    serde_json::to_string(&("ruby:structural", abstracted, captures))
        .expect("structural tokens serialize")
}

// Preserve topology as well as leaves; Ruby whitespace can alter call nesting.
type SyntaxTokens<'tree, 'source> = Vec<(&'tree str, Cow<'source, str>)>;
pub(super) fn syntax_tokens<'tree, 'source>(
    root: Node<'tree>,
    source: &'source str,
    bindings: &super::bindings::Bindings,
) -> (SyntaxTokens<'tree, 'source>, SyntaxTokens<'tree, 'source>) {
    tokens_with_elision(root, source, bindings, false)
}

/// Alpha tokens for behaviour events: declared callable bodies are syntax, not behaviour.
pub(super) fn event_tokens<'tree, 'source>(
    root: Node<'tree>,
    source: &'source str,
    bindings: &super::bindings::Bindings,
) -> SyntaxTokens<'tree, 'source> {
    tokens_with_elision(root, source, bindings, true).1
}

fn tokens_with_elision<'tree, 'source>(
    root: Node<'tree>,
    source: &'source str,
    bindings: &super::bindings::Bindings,
    elide: bool,
) -> (SyntaxTokens<'tree, 'source>, SyntaxTokens<'tree, 'source>) {
    // Preserve tree structure: Ruby whitespace can change call nesting without changing leaves.
    let mut tokens = Vec::new();
    let mut alpha = Vec::new();
    let mut pending = vec![(root, false)];
    while let Some((node, closing)) = pending.pop() {
        if elide && node != root && bindings.elided.contains(&node.id()) {
            tokens.push((node.kind(), Cow::Borrowed("<declared>")));
            alpha.push((node.kind(), Cow::Borrowed("<declared>")));
            continue;
        }
        if root.kind() == "method"
            && node.parent() == Some(root)
            && (root.child_by_field_name("name") == Some(node)
                || matches!(node.kind(), "def" | "end"))
        {
            continue;
        }
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
