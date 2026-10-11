//! Closed, immutable local references to the implicit registration method table.
use super::{descendants, registration, static_method_name, text};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

#[derive(Default)]
pub(super) struct RegistrationAliases {
    captures: BTreeSet<usize>,
    calls: BTreeMap<usize, String>,
}

impl RegistrationAliases {
    pub(super) fn collect(root: Node<'_>, source: &str) -> Self {
        let mut result = Self::default();
        if root.has_error() {
            return result;
        }
        let nodes = descendants(root);
        let references = identifier_references(&nodes, source);
        for assignment in nodes
            .iter()
            .copied()
            .filter(|node| node.kind() == "assignment" && node.parent() == Some(root))
        {
            let Some(binding) = assignment.child_by_field_name("left") else {
                continue;
            };
            let Some(capture) = assignment.child_by_field_name("right") else {
                continue;
            };
            if binding.kind() != "identifier"
                || capture.kind() != "call"
                || capture.child_by_field_name("receiver").is_some()
                || capture.child_by_field_name("block").is_some()
                || !capture
                    .child_by_field_name("method")
                    .is_some_and(|name| text(name, source) == "method")
            {
                continue;
            }
            let Some(args) = capture.child_by_field_name("arguments") else {
                continue;
            };
            if args.named_child_count() != 1 {
                continue;
            }
            let Some(name) = args
                .named_child(0)
                .and_then(|arg| static_method_name(arg, source))
                .filter(|name| registration(name))
            else {
                continue;
            };
            let mut calls = Vec::new();
            // Any other use may copy, mutate, escape or shadow this value. Refuse the
            // whole capture rather than resolve execution order across arbitrary scopes.
            let closed = references[text(binding, source)]
                .iter()
                .copied()
                .filter(|node| *node != binding)
                .all(|reference| {
                    let Some(call) = closed_call(reference, assignment, source) else {
                        return false;
                    };
                    calls.push(call.id());
                    true
                });
            if closed {
                result.captures.insert(capture.id());
                result
                    .calls
                    .extend(calls.into_iter().map(|call| (call, name.clone())));
            }
        }
        result
    }

    pub(super) fn extend_provider(&mut self, root: Node<'_>, proof: &super::providers::Proof) {
        for node in descendants(root)
            .into_iter()
            .filter(|node| node.kind() == "call")
        {
            if proof.captures.contains(&node.start_byte()) {
                self.captures.insert(node.id());
            }
            if let Some(name) = proof.calls.get(&node.start_byte()) {
                self.calls.insert(node.id(), name.clone());
            }
        }
    }

    pub(super) fn capture(&self, node: Node<'_>) -> bool {
        self.captures.contains(&node.id())
    }

    pub(super) fn registration(&self, node: Node<'_>) -> Option<&str> {
        self.calls.get(&node.id()).map(String::as_str)
    }
}

/// Indexes every same-spelled reference, including assignments and nested scopes.
pub(super) fn identifier_references<'a>(
    nodes: &[Node<'a>],
    source: &'a str,
) -> BTreeMap<&'a str, Vec<Node<'a>>> {
    let mut references = BTreeMap::<_, Vec<_>>::new();
    for node in nodes.iter().copied().filter(|n| {
        n.kind() == "identifier"
            && !n.parent().is_some_and(|p| {
                (p.kind() == "call" && p.child_by_field_name("method") == Some(*n))
                    || (matches!(p.kind(), "method" | "singleton_method")
                        && p.child_by_field_name("name") == Some(*n))
            })
    }) {
        references.entry(text(node, source)).or_default().push(node);
    }
    references
}

/// A registration alias may only be called after assignment in the same root scope.
pub(super) fn closed_call<'a>(
    reference: Node<'a>,
    assignment: Node<'a>,
    source: &str,
) -> Option<Node<'a>> {
    let call = reference.parent()?;
    (call.kind() == "call"
        && call.parent() == assignment.parent()
        && call.start_byte() >= assignment.end_byte()
        && call.child_by_field_name("receiver") == Some(reference)
        && call
            .child_by_field_name("method")
            .is_some_and(|n| text(n, source) == "call"))
    .then_some(call)
}
