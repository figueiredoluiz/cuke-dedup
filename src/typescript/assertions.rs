use super::ast::{import_module, is_type_only_declaration, is_type_only_specifier, string_literal};
use super::node_text;
use std::collections::BTreeSet;
use tree_sitter::Node;

// These modules expose the Jest-compatible `expect` API CukeDedup can identify without module
// resolution. Exact matching avoids treating unrelated package-name prefixes as trusted APIs.
const ASSERTION_MODULES: [&str; 4] = [
    "@playwright/test",
    "playwright/test",
    "@jest/globals",
    "expect",
];

pub(super) struct AssertionBindings {
    identifiers: BTreeSet<String>,
    namespaces: BTreeSet<String>,
}

impl Default for AssertionBindings {
    fn default() -> Self {
        Self {
            identifiers: BTreeSet::from(["expect".to_owned()]),
            namespaces: BTreeSet::new(),
        }
    }
}

impl AssertionBindings {
    pub(super) fn discover(root: Node<'_>, source: &[u8]) -> Self {
        let mut bindings = Self::default();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "import_statement" => bindings.collect_import(node, source),
                "variable_declarator" => bindings.collect_require(node, source),
                _ => {}
            }
            super::ast::push_named_children_reverse(node, &mut stack);
        }
        bindings
    }

    pub(super) fn is_factory(&self, node: Node<'_>, source: &[u8]) -> bool {
        if node.kind() == "identifier" {
            return self.identifiers.contains(node_text(node, source));
        }
        if node.kind() != "member_expression" {
            return false;
        }
        let (Some(object), Some(property)) = (
            node.child_by_field_name("object"),
            node.child_by_field_name("property"),
        ) else {
            return false;
        };
        object.kind() == "identifier"
            && self.namespaces.contains(node_text(object, source))
            && node_text(property, source) == "expect"
    }

    fn collect_import(&mut self, import: Node<'_>, source: &[u8]) {
        if is_type_only_declaration(import) {
            return;
        }
        let Some(module) = import_module(import, source) else {
            return;
        };
        if !ASSERTION_MODULES.contains(&module) {
            return;
        }

        let mut stack = vec![import];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "import_specifier" if !is_type_only_specifier(node) => {
                    let Some(name) = node.child_by_field_name("name") else {
                        continue;
                    };
                    if node_text(name, source) == "expect" {
                        let local = node.child_by_field_name("alias").unwrap_or(name);
                        self.identifiers.insert(node_text(local, source).to_owned());
                    }
                }
                "namespace_import" => {
                    if let Some(identifier) = node.named_child(0) {
                        self.namespaces
                            .insert(node_text(identifier, source).to_owned());
                    }
                }
                // The standalone `expect` package also provides a callable default export.
                "import_clause" if module == "expect" => {
                    if let Some(identifier) = node
                        .named_child(0)
                        .filter(|child| child.kind() == "identifier")
                    {
                        self.identifiers
                            .insert(node_text(identifier, source).to_owned());
                    }
                }
                _ => {}
            }
            super::ast::push_named_children_reverse(node, &mut stack);
        }
    }

    fn collect_require(&mut self, declaration: Node<'_>, source: &[u8]) {
        let (Some(name), Some(value)) = (
            declaration.child_by_field_name("name"),
            declaration.child_by_field_name("value"),
        ) else {
            return;
        };
        let Some(module) = required_module(value, source) else {
            return;
        };
        if !ASSERTION_MODULES.contains(&module) {
            return;
        }
        match name.kind() {
            "identifier" if module == "expect" => {
                self.identifiers.insert(node_text(name, source).to_owned());
            }
            "identifier" => {
                self.namespaces.insert(node_text(name, source).to_owned());
            }
            "object_pattern" => collect_expect_pattern(name, source, &mut self.identifiers),
            _ => {}
        }
    }
}

fn required_module<'a>(call: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    if call.kind() != "call_expression" {
        return None;
    }
    let function = call.child_by_field_name("function")?;
    if function.kind() != "identifier" || node_text(function, source) != "require" {
        return None;
    }
    let arguments = call.child_by_field_name("arguments")?;
    string_literal(arguments.named_child(0)?, source)
}

fn collect_expect_pattern(pattern: Node<'_>, source: &[u8], identifiers: &mut BTreeSet<String>) {
    let mut cursor = pattern.walk();
    for property in pattern.named_children(&mut cursor) {
        match property.kind() {
            "shorthand_property_identifier_pattern" if node_text(property, source) == "expect" => {
                identifiers.insert("expect".to_owned());
            }
            "pair_pattern" => {
                let (Some(key), Some(value)) = (
                    property.child_by_field_name("key"),
                    property.child_by_field_name("value"),
                ) else {
                    continue;
                };
                if node_text(key, source) == "expect" && value.kind() == "identifier" {
                    identifiers.insert(node_text(value, source).to_owned());
                }
            }
            _ => {}
        }
    }
}
