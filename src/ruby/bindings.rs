//! Source-order local bindings for handler identity; names never establish framework trust.
use super::text;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

#[derive(Default)]
pub(super) struct Bindings {
    pub names: BTreeMap<usize, String>,
    pub captures: BTreeSet<String>,
    pub calls: BTreeSet<usize>,
    pub uncertain: bool,
}

impl Bindings {
    /// Tracks parser-order bindings while keeping unresolved reflection and implicit parameters uncertain.
    pub fn collect(block: Node<'_>, root: Node<'_>, source: &str) -> Self {
        let mut result = Self::default();
        let mut outer = BTreeSet::new();
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if node.start_byte() >= block.start_byte() {
                continue;
            }
            match node.kind() {
                "block" | "do_block" | "lambda" | "method" | "singleton_method" | "class"
                | "module" | "singleton_class" => continue,
                "for" | "rescue" | "in_clause" | "match_pattern" | "test_pattern" => {
                    result.uncertain = true
                }
                "binary"
                    if node
                        .child_by_field_name("operator")
                        .is_some_and(|n| text(n, source) == "=~") =>
                {
                    result.uncertain = true
                }
                "assignment" | "operator_assignment" => {
                    if let Some(left) = node.child_by_field_name("left") {
                        outer.extend(
                            binding_nodes(left)
                                .iter()
                                .map(|n| text(*n, source).to_owned()),
                        );
                    }
                }
                _ => {}
            }
            let mut cursor = node.walk();
            pending.extend(node.named_children(&mut cursor));
        }
        let mut scopes = vec![outer
            .into_iter()
            .map(|name| (name.clone(), format!("capture:{name}")))
            .collect::<BTreeMap<_, _>>()];
        let mut next = 0;
        let mut pending = vec![(block, false)];
        while let Some((node, closing)) = pending.pop() {
            let scope = matches!(node.kind(), "block" | "do_block" | "lambda");
            if closing {
                if scope {
                    scopes.pop();
                }
                continue;
            }
            if scope {
                scopes.push(BTreeMap::new());
                if let Some(parameters) = node.child_by_field_name("parameters") {
                    for name in binding_nodes(parameters) {
                        let key = format!("local:{next}");
                        next += 1;
                        scopes
                            .last_mut()
                            .unwrap()
                            .insert(text(name, source).to_owned(), key.clone());
                        result.names.insert(name.id(), key);
                    }
                }
            }
            if matches!(
                node.kind(),
                "for" | "in_clause" | "match_pattern" | "test_pattern"
            ) || (node.kind() == "rescue" && node.child_by_field_name("variable").is_some())
                || (node.kind() == "binary"
                    && node
                        .child_by_field_name("operator")
                        .is_some_and(|n| text(n, source) == "=~"))
                || (node.kind() == "pair" && node.child_by_field_name("value").is_none())
            {
                result.uncertain = true;
            }
            if matches!(node.kind(), "assignment" | "operator_assignment") {
                if let Some(left) = node.child_by_field_name("left") {
                    for name in binding_nodes(left) {
                        let spelling = text(name, source);
                        let key = scopes
                            .iter()
                            .rev()
                            .find_map(|s| s.get(spelling))
                            .cloned()
                            .unwrap_or_else(|| {
                                let key = format!("local:{next}");
                                next += 1;
                                scopes
                                    .last_mut()
                                    .unwrap()
                                    .insert(spelling.to_owned(), key.clone());
                                key
                            });
                        result.names.insert(name.id(), key);
                    }
                }
            }
            if node.kind() == "identifier" {
                let spelling = text(node, source);
                let method = node.parent().is_some_and(|p| {
                    p.kind() == "call" && p.child_by_field_name("method") == Some(node)
                });
                if !method {
                    if let Some(key) = scopes.iter().rev().find_map(|s| s.get(spelling)) {
                        result.names.insert(node.id(), key.clone());
                        if let Some(name) = key.strip_prefix("capture:") {
                            result.captures.insert(name.to_owned());
                        }
                    } else {
                        result.calls.insert(node.id());
                    }
                }
                if (method || result.calls.contains(&node.id()))
                    && matches!(
                        spelling,
                        "caller"
                            | "caller_locations"
                            | "source_location"
                            | "binding"
                            | "eval"
                            | "local_variables"
                            | "local_variable_get"
                            | "local_variable_set"
                            | "method"
                            | "public_method"
                            | "send"
                            | "public_send"
                            | "__send__"
                            | "instance_eval"
                    )
                {
                    result.uncertain = true;
                }
                if spelling
                    .strip_prefix('_')
                    .is_some_and(|n| n.parse::<u8>().is_ok_and(|n| (1..=9).contains(&n)))
                {
                    result.uncertain = true;
                }
                if spelling == "it" && result.calls.contains(&node.id()) {
                    result.uncertain = true;
                }
                if matches!(spelling, "__FILE__" | "__dir__") {
                    result.captures.insert("<source-file>".to_owned());
                }
            }
            if node.kind() == "file" {
                result.captures.insert("<source-file>".to_owned());
            }
            pending.push((node, true));
            let mut cursor = node.walk();
            let mut children: Vec<_> = node.named_children(&mut cursor).collect();
            // Ruby's lexical declarations follow source order, even for modifier expressions.
            children.sort_by_key(Node::start_byte);
            pending.extend(children.into_iter().rev().map(|n| (n, false)));
        }
        result
    }
}

/// Collects binding targets without renaming property receivers, keys, or method names.
fn binding_nodes(node: Node<'_>) -> Vec<Node<'_>> {
    let mut names = Vec::new();
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "identifier" => names.push(node),
            "left_assignment_list"
            | "destructured_left_assignment"
            | "rest_assignment"
            | "block_parameters"
            | "lambda_parameters"
            | "destructured_parameter" => {
                let mut cursor = node.walk();
                pending.extend(node.named_children(&mut cursor));
            }
            "block_parameter" | "splat_parameter" | "hash_splat_parameter" => {
                pending.extend(node.child_by_field_name("name"));
            }
            _ => {}
        }
    }
    names.sort_by_key(Node::start_byte);
    names
}
