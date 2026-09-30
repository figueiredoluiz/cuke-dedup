//! Exact Ruby handler trees with explicit lexical capture identity.
use super::{descendants, text};
use crate::model::HandlerFingerprint;
use std::borrow::Cow;
use tree_sitter::Node;

pub(super) fn fingerprint(block: Node<'_>, root: Node<'_>, source: &str) -> HandlerFingerprint {
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
    // Preserve tree structure: Ruby whitespace can change call nesting without changing leaves.
    let mut tokens = Vec::new();
    let mut alpha = Vec::new();
    let mut pending = vec![(block, false)];
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
    let captures = bindings.captures;
    let behavior_signature = if captures.is_empty() {
        vec![]
    } else {
        vec!["method:ruby:lexical-file".to_owned()]
    };
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
    HandlerFingerprint {
        exact: exact.clone(),
        normalized: exact.clone(),
        alpha_normalized,
        structural: exact,
        behavior_signature,
        source_snippet: text(block, source).chars().take(2000).collect(),
        comparable,
        trivial,
    }
}
