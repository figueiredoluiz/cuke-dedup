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
        let mut references = BTreeMap::<_, Vec<_>>::new();
        for node in nodes
            .iter()
            .copied()
            .filter(|node| node.kind() == "identifier")
        {
            references.entry(text(node, source)).or_default().push(node);
        }
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
                    let Some(call) = reference.parent() else {
                        return false;
                    };
                    if call.kind() != "call"
                        || call.parent() != Some(root)
                        || call.start_byte() < assignment.end_byte()
                        || call.child_by_field_name("receiver") != Some(reference)
                        || !call
                            .child_by_field_name("method")
                            .is_some_and(|method| text(method, source) == "call")
                    {
                        return false;
                    }
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

    pub(super) fn capture(&self, node: Node<'_>) -> bool {
        self.captures.contains(&node.id())
    }

    pub(super) fn registration(&self, node: Node<'_>) -> Option<&str> {
        self.calls.get(&node.id()).map(String::as_str)
    }
}
