use super::ast::{
    import_has_runtime_bindings, import_module, is_type_only_declaration, is_type_only_specifier,
    string_literal,
};
use super::node_text;
use super::registrations::RegistrationNames;
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

#[derive(Default)]
pub(super) struct AssertionBindings {
    identifiers: BTreeSet<String>,
    namespaces: BTreeSet<String>,
}

impl AssertionBindings {
    pub(super) fn discover(
        root: Node<'_>,
        source: &[u8],
        registrations: &RegistrationNames,
    ) -> Self {
        let mut bindings = Self::default();
        let mut shadowed = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "import_statement" if import_has_runtime_bindings(node) => {
                    let trusted = bindings.collect_import(node, source, registrations);
                    extend_untrusted_bindings(node, source, &trusted, &mut shadowed);
                }
                "variable_declarator" => {
                    let trusted = bindings.collect_require(node, source);
                    if let Some(name) = node.child_by_field_name("name") {
                        extend_untrusted_bindings(name, source, &trusted, &mut shadowed);
                    }
                }
                "function_declaration"
                | "generator_function_declaration"
                | "class_declaration"
                | "abstract_class_declaration"
                | "enum_declaration" => {
                    if let Some(name) = node.child_by_field_name("name") {
                        collect_binding_names(name, source, &mut shadowed);
                    }
                }
                "formal_parameters" => {
                    collect_binding_names(node, source, &mut shadowed);
                }
                "catch_clause" => {
                    if let Some(parameter) = node.child_by_field_name("parameter") {
                        collect_binding_names(parameter, source, &mut shadowed);
                    }
                }
                "assignment_expression" => {
                    if let Some(left) = node.child_by_field_name("left") {
                        if matches!(
                            left.kind(),
                            "identifier" | "object_pattern" | "array_pattern"
                        ) {
                            collect_binding_names(left, source, &mut shadowed);
                        }
                    }
                }
                _ => {}
            }
            super::ast::push_named_children_reverse(node, &mut stack);
        }
        // Runtime declarations override trusted imports and aliases conservatively for the whole
        // file, matching step-registration discovery. A false negative is safer than assigning
        // assertion semantics to an unrelated function and manufacturing similarity evidence.
        bindings
            .identifiers
            .retain(|identifier| !shadowed.contains(identifier));
        bindings
            .namespaces
            .retain(|namespace| !shadowed.contains(namespace));
        // With no runtime declaration, retain the conventional ambient Jest/Playwright global.
        if !shadowed.contains("expect") {
            bindings.identifiers.insert("expect".to_owned());
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

    fn collect_import(
        &mut self,
        import: Node<'_>,
        source: &[u8],
        registrations: &RegistrationNames,
    ) -> BTreeSet<String> {
        let mut trusted = BTreeSet::new();
        if is_type_only_declaration(import) {
            return trusted;
        }
        let Some(module) = import_module(import, source) else {
            return trusted;
        };
        let exact_assertion_module = ASSERTION_MODULES.contains(&module);
        // Custom framework fixtures commonly re-export `expect` beside Given/When/Then. Reuse
        // registration resolution as provenance instead of trusting every local module named by
        // an import. An unrelated package importing only `expect` remains untrusted.
        let framework_facade = import_has_resolved_registration(import, source, registrations);
        if !exact_assertion_module && !framework_facade {
            return trusted;
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
                        let local = node_text(local, source).to_owned();
                        self.identifiers.insert(local.clone());
                        trusted.insert(local);
                    }
                }
                "namespace_import" if exact_assertion_module || framework_facade => {
                    if let Some(identifier) = node.named_child(0) {
                        let local = node_text(identifier, source).to_owned();
                        self.namespaces.insert(local.clone());
                        trusted.insert(local);
                    }
                }
                // The standalone `expect` package also provides a callable default export.
                "import_clause" if module == "expect" => {
                    if let Some(identifier) = node
                        .named_child(0)
                        .filter(|child| child.kind() == "identifier")
                    {
                        let local = node_text(identifier, source).to_owned();
                        self.identifiers.insert(local.clone());
                        trusted.insert(local);
                    }
                }
                _ => {}
            }
            super::ast::push_named_children_reverse(node, &mut stack);
        }
        trusted
    }

    fn collect_require(&mut self, declaration: Node<'_>, source: &[u8]) -> BTreeSet<String> {
        let mut trusted = BTreeSet::new();
        let (Some(name), Some(value)) = (
            declaration.child_by_field_name("name"),
            declaration.child_by_field_name("value"),
        ) else {
            return trusted;
        };
        let Some(module) = required_module(value, source) else {
            return trusted;
        };
        if !ASSERTION_MODULES.contains(&module) {
            return trusted;
        }
        match name.kind() {
            "identifier" if module == "expect" => {
                let local = node_text(name, source).to_owned();
                self.identifiers.insert(local.clone());
                trusted.insert(local);
            }
            "identifier" => {
                let local = node_text(name, source).to_owned();
                self.namespaces.insert(local.clone());
                trusted.insert(local);
            }
            "object_pattern" => {
                collect_expect_pattern(name, source, &mut self.identifiers, &mut trusted)
            }
            _ => {}
        }
        trusted
    }
}

fn import_has_resolved_registration(
    import: Node<'_>,
    source: &[u8],
    registrations: &RegistrationNames,
) -> bool {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_specifier" if !is_type_only_specifier(node) => {
                if node
                    .child_by_field_name("alias")
                    .or_else(|| node.child_by_field_name("name"))
                    .is_some_and(|local| registrations.recognizes_alias(node_text(local, source)))
                {
                    return true;
                }
                continue;
            }
            "namespace_import"
                if node.named_child(0).is_some_and(|local| {
                    registrations.recognizes_namespace(node_text(local, source))
                }) =>
            {
                return true
            }
            _ => {}
        }
        super::ast::push_named_children_reverse(node, &mut stack);
    }
    false
}

fn extend_untrusted_bindings(
    root: Node<'_>,
    source: &[u8],
    trusted: &BTreeSet<String>,
    shadowed: &mut BTreeSet<String>,
) {
    let mut locals = BTreeSet::new();
    collect_binding_names(root, source, &mut locals);
    shadowed.extend(locals.into_iter().filter(|local| !trusted.contains(local)));
}

fn collect_binding_names(root: Node<'_>, source: &[u8], output: &mut BTreeSet<String>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "identifier" | "shorthand_property_identifier_pattern" | "namespace_import" => {
                output.insert(
                    node_text(node, source)
                        .trim_start_matches("* as ")
                        .to_owned(),
                );
            }
            "import_specifier" => {
                if !is_type_only_specifier(node) {
                    if let Some(local) = node
                        .child_by_field_name("alias")
                        .or_else(|| node.child_by_field_name("name"))
                    {
                        output.insert(node_text(local, source).to_owned());
                    }
                }
                continue;
            }
            // Object keys are property names; only the value introduces a local binding.
            "pair_pattern" => {
                if let Some(value) = node.child_by_field_name("value") {
                    stack.push(value);
                }
                continue;
            }
            "assignment_pattern" | "object_assignment_pattern" => {
                if let Some(left) = node
                    .child_by_field_name("left")
                    .or_else(|| node.named_child(0))
                {
                    stack.push(left);
                }
                continue;
            }
            "type_annotation" => continue,
            _ => {}
        }
        super::ast::push_named_children_reverse(node, &mut stack);
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

fn collect_expect_pattern(
    pattern: Node<'_>,
    source: &[u8],
    identifiers: &mut BTreeSet<String>,
    trusted: &mut BTreeSet<String>,
) {
    let mut cursor = pattern.walk();
    for property in pattern.named_children(&mut cursor) {
        match property.kind() {
            "shorthand_property_identifier_pattern" if node_text(property, source) == "expect" => {
                identifiers.insert("expect".to_owned());
                trusted.insert("expect".to_owned());
            }
            "pair_pattern" => {
                let (Some(key), Some(value)) = (
                    property.child_by_field_name("key"),
                    property.child_by_field_name("value"),
                ) else {
                    continue;
                };
                if node_text(key, source) == "expect" && value.kind() == "identifier" {
                    let local = node_text(value, source).to_owned();
                    identifiers.insert(local.clone());
                    trusted.insert(local);
                }
            }
            _ => {}
        }
    }
}
