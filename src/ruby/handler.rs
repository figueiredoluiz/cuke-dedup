//! Exact Ruby handler trees with explicit lexical capture identity.
use super::{descendants, text};
use crate::model::HandlerFingerprint;
use std::borrow::Cow;
use std::collections::BTreeSet;
use tree_sitter::Node;

pub(super) fn fingerprint(block: Node<'_>, root: Node<'_>, source: &str) -> HandlerFingerprint {
    let nodes = descendants(block);
    let captures = captures(block, root, source);
    let comparable = captures.is_some()
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
            ) || (node.kind() == "identifier"
                && matches!(
                    text(*node, source),
                    "binding" | "eval" | "local_variable_get" | "local_variable_set"
                ))
        });
    // Preserve tree structure: Ruby whitespace can change call nesting without changing leaves.
    let mut tokens = Vec::new();
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
            continue;
        }
        if node.kind() == "line"
            || (node.kind() == "identifier" && text(node, source) == "__LINE__")
        {
            tokens.push((
                node.kind(),
                Cow::Owned((node.start_position().row + 1).to_string()),
            ));
        } else if node.child_count() == 0 {
            tokens.push((kind, Cow::Borrowed(text(node, source))));
        } else {
            tokens.push((kind, Cow::Borrowed("(")));
            pending.push((node, true));
            let mut cursor = node.walk();
            let children: Vec<_> = node.children(&mut cursor).collect();
            pending.extend(children.into_iter().rev().map(|node| (node, false)));
        }
    }
    let captures = captures.unwrap_or_default();
    let behavior_signature = if captures.is_empty() {
        vec![]
    } else {
        vec!["method:ruby:lexical-file".to_owned()]
    };
    let exact = if captures.is_empty() {
        serde_json::to_string(&tokens)
    } else {
        serde_json::to_string(&(tokens, captures))
    }
    .expect("string tokens serialize");
    let trivial = !nodes
        .iter()
        .any(|node| node.kind() == "call" || node.kind() == "assignment");
    HandlerFingerprint {
        exact: exact.clone(),
        normalized: exact.clone(),
        alpha_normalized: exact.clone(),
        structural: exact,
        behavior_signature,
        source_snippet: text(block, source).chars().take(2000).collect(),
        comparable,
        trivial,
    }
}

fn captures(block: Node<'_>, root: Node<'_>, source: &str) -> Option<BTreeSet<String>> {
    let mut outer = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if node.start_byte() >= block.start_byte() {
            continue;
        }
        match node.kind() {
            "block" | "do_block" | "lambda" | "method" | "singleton_method" | "class"
            | "module" | "singleton_class" => continue,
            // These can introduce locals without ordinary assignment targets. Do not guess
            // that an unresolved identifier is a method call instead of a captured variable.
            "for" | "rescue" | "in" | "match_pattern" | "test_pattern" => return None,
            "binary"
                if node
                    .child_by_field_name("operator")
                    .is_some_and(|op| text(op, source) == "=~") =>
            {
                return None
            }
            "assignment" | "operator_assignment" => {
                if let Some(left) = node.child_by_field_name("left") {
                    binding_names(left, source, &mut outer);
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    let mut captured = BTreeSet::new();
    for node in descendants(block) {
        if node.kind() == "file"
            || (node.kind() == "identifier" && matches!(text(node, source), "__FILE__" | "__dir__"))
        {
            captured.insert("<source-file>".to_owned());
        }
        if node.kind() != "identifier" || !outer.contains(text(node, source)) {
            continue;
        }
        if node.parent().is_some_and(|parent| {
            parent.kind() == "call" && parent.child_by_field_name("method") == Some(node)
        }) {
            continue;
        }
        let mut shadowed = false;
        let mut ancestor = node.parent();
        while let Some(scope) = ancestor {
            if let Some(parameters) = scope.child_by_field_name("parameters") {
                let mut names = BTreeSet::new();
                binding_names(parameters, source, &mut names);
                shadowed |= names.contains(text(node, source));
            }
            if scope == block {
                break;
            }
            ancestor = scope.parent();
        }
        if !shadowed {
            captured.insert(text(node, source).to_owned());
        }
    }
    Some(captured)
}

fn binding_names(node: Node<'_>, source: &str, names: &mut BTreeSet<String>) {
    match node.kind() {
        "identifier" => {
            names.insert(text(node, source).to_owned());
        }
        "left_assignment_list"
        | "destructured_left_assignment"
        | "rest_assignment"
        | "block_parameters"
        | "lambda_parameters"
        | "destructured_parameter" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                binding_names(child, source, names);
            }
        }
        "block_parameter" | "splat_parameter" | "hash_splat_parameter" => {
            if let Some(name) = node.child_by_field_name("name") {
                binding_names(name, source, names);
            }
        }
        _ => {}
    }
}
