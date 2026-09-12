use super::ast::{
    import_has_runtime_bindings, import_module, is_type_only_declaration, is_type_only_specifier,
    string_literal,
};
use super::node_text;
use super::registrations::RegistrationNames;
use std::collections::{BTreeMap, BTreeSet};
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
    shadow_ranges: BTreeMap<String, Vec<(usize, usize)>>,
}

impl AssertionBindings {
    pub(super) fn discover(
        root: Node<'_>,
        source: &[u8],
        registrations: &RegistrationNames,
    ) -> Self {
        let facade_modules = resolved_facade_modules(root, source, registrations);
        let shadow_ranges = collect_scoped_bindings(root, source);
        let require_shadowed =
            module_runtime_binding_exists(root, source, "require", &shadow_ranges);
        let mut bindings = Self::default();
        let mut shadowed = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "import_statement" if import_has_runtime_bindings(node) => {
                    let trusted = bindings.collect_import(node, source, &facade_modules);
                    extend_untrusted_bindings(node, source, &trusted, &mut shadowed);
                }
                "variable_declarator" if is_top_level_variable(node) => {
                    // Binding names are file-scoped in this conservative model. Trust only
                    // module-scope CommonJS declarations so a nested helper cannot leak an
                    // assertion alias into unrelated step handlers.
                    let trusted = if !require_shadowed {
                        bindings.collect_require(node, source)
                    } else {
                        BTreeSet::new()
                    };
                    if let Some(name) = node.child_by_field_name("name") {
                        extend_untrusted_bindings(name, source, &trusted, &mut shadowed);
                    }
                }
                "variable_declarator" if is_module_var(node) => {
                    if let Some(name) = node.child_by_field_name("name") {
                        collect_binding_names(name, source, &mut shadowed);
                    }
                }
                "for_in_statement"
                    if node.child_by_field_name("left").is_some_and(|left| {
                        loop_binding_keyword(node, left) == Some("var")
                            && nearest_function_scope(node.parent()).is_none()
                    }) =>
                {
                    if let Some(left) = node.child_by_field_name("left") {
                        collect_binding_names(left, source, &mut shadowed);
                    }
                }
                "function_declaration"
                | "generator_function_declaration"
                | "class_declaration"
                | "abstract_class_declaration"
                | "enum_declaration"
                    if is_module_declaration(node) =>
                {
                    if let Some(name) = node.child_by_field_name("name") {
                        collect_binding_names(name, source, &mut shadowed);
                    }
                }
                "internal_module" | "module"
                    if !is_erased_declaration(node) && is_module_declaration(node) =>
                {
                    if let Some(name) = node.child_by_field_name("name") {
                        collect_binding_names(name, source, &mut shadowed);
                    }
                }
                "assignment_expression" => {
                    if let Some(left) = node.child_by_field_name("left") {
                        if matches!(
                            left.kind(),
                            "identifier" | "object_pattern" | "array_pattern"
                        ) {
                            let mut assigned = BTreeSet::new();
                            collect_assignment_targets(left, source, &mut assigned);
                            shadowed.extend(
                                assigned.into_iter().filter(|name| {
                                    !position_is_shadowed(&shadow_ranges, name, node)
                                }),
                            );
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
        // A trusted namespace called `expect` still occupies the runtime name and is not itself
        // callable. Do not restore the ambient callable merely because the namespace provenance
        // is trusted; `expect.expect(...)` remains recognized through `namespaces`.
        if !shadowed.contains("expect") && !bindings.namespaces.contains("expect") {
            bindings.identifiers.insert("expect".to_owned());
        }
        bindings.shadow_ranges = shadow_ranges;
        bindings
    }

    pub(super) fn is_factory(&self, node: Node<'_>, source: &[u8]) -> bool {
        if node.kind() == "identifier" {
            let name = node_text(node, source);
            return self.identifiers.contains(name) && !self.is_locally_shadowed(node, name);
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
        let namespace = node_text(object, source);
        object.kind() == "identifier"
            && self.namespaces.contains(namespace)
            && !self.is_locally_shadowed(object, namespace)
            && node_text(property, source) == "expect"
    }

    fn is_locally_shadowed(&self, node: Node<'_>, expected: &str) -> bool {
        position_is_shadowed(&self.shadow_ranges, expected, node)
    }

    fn collect_import(
        &mut self,
        import: Node<'_>,
        source: &[u8],
        facade_modules: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut trusted = BTreeSet::new();
        if is_type_only_declaration(import) {
            return trusted;
        }
        let Some(module) = assertion_import_module(import, source) else {
            return trusted;
        };
        let exact_assertion_module = ASSERTION_MODULES.contains(&module);
        // Registration provenance is collected by module before this pass, so split imports from
        // one custom fixture retain the same trust without trusting unrelated local modules.
        let framework_facade = facade_modules.contains(module);
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
                "namespace_import" if exact_assertion_module => {
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
                "import_require_clause" => {
                    if let Some(local) = node
                        .named_child(0)
                        .filter(|child| child.kind() == "identifier")
                    {
                        let local = node_text(local, source).to_owned();
                        if module == "expect" {
                            self.identifiers.insert(local.clone());
                        } else if exact_assertion_module {
                            self.namespaces.insert(local.clone());
                        } else {
                            continue;
                        }
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

fn assertion_import_module<'a>(import: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    if let Some(module) = import_module(import, source) {
        return Some(module);
    }
    let mut cursor = import.walk();
    let module = import
        .named_children(&mut cursor)
        .find(|child| child.kind() == "import_require_clause")
        .and_then(|clause| clause.child_by_field_name("source"))
        .and_then(|literal| string_literal(literal, source));
    module
}

fn resolved_facade_modules(
    root: Node<'_>,
    source: &[u8],
    registrations: &RegistrationNames,
) -> BTreeSet<String> {
    let mut modules = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_statement"
            && import_has_runtime_bindings(node)
            && import_has_resolved_registration(node, source, registrations)
        {
            if let Some(module) = import_module(node, source) {
                modules.insert(module.to_owned());
            }
        }
        super::ast::push_named_children_reverse(node, &mut stack);
    }
    modules
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
            "identifier"
            | "type_identifier"
            | "shorthand_property_identifier_pattern"
            | "namespace_import" => {
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

fn collect_assignment_targets(root: Node<'_>, source: &[u8], output: &mut BTreeSet<String>) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "identifier" | "shorthand_property_identifier_pattern" => {
                output.insert(node_text(node, source).to_owned());
            }
            // Property writes mutate an object; they do not rebind the receiver identifier.
            "member_expression" | "subscript_expression" => continue,
            "pair" | "pair_pattern" => {
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

fn module_runtime_binding_exists(
    root: Node<'_>,
    source: &[u8],
    expected: &str,
    shadow_ranges: &BTreeMap<String, Vec<(usize, usize)>>,
) -> bool {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let pattern = match node.kind() {
            "import_statement" if import_has_runtime_bindings(node) => Some(node),
            "variable_declarator" if is_top_level_variable(node) => {
                node.child_by_field_name("name")
            }
            "variable_declarator" if is_module_var(node) => node.child_by_field_name("name"),
            "for_in_statement"
                if node.child_by_field_name("left").is_some_and(|left| {
                    loop_binding_keyword(node, left) == Some("var")
                        && nearest_function_scope(node.parent()).is_none()
                }) =>
            {
                node.child_by_field_name("left")
            }
            "function_declaration"
            | "generator_function_declaration"
            | "class_declaration"
            | "abstract_class_declaration"
            | "enum_declaration"
                if is_module_declaration(node) =>
            {
                node.child_by_field_name("name")
            }
            "internal_module" | "module"
                if !is_erased_declaration(node) && is_module_declaration(node) =>
            {
                node.child_by_field_name("name")
            }
            "assignment_expression" => node.child_by_field_name("left").filter(|left| {
                matches!(
                    left.kind(),
                    "identifier" | "object_pattern" | "array_pattern"
                ) && !position_is_shadowed(shadow_ranges, expected, node)
            }),
            _ => None,
        };
        if let Some(pattern) = pattern {
            let mut names = BTreeSet::new();
            if node.kind() == "assignment_expression" {
                collect_assignment_targets(pattern, source, &mut names);
            } else {
                collect_binding_names(pattern, source, &mut names);
            }
            if names.contains(expected) {
                return true;
            }
        }
        super::ast::push_named_children_reverse(node, &mut stack);
    }
    false
}

fn collect_scoped_bindings(root: Node<'_>, source: &[u8]) -> BTreeMap<String, Vec<(usize, usize)>> {
    let mut scopes = BTreeMap::<usize, BTreeSet<String>>::new();
    let mut scope_ranges = BTreeMap::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if is_local_scope(node) {
            scope_ranges.insert(node.id(), (node.start_byte(), node.end_byte()));
        }
        match node.kind() {
            "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "generator_function"
            | "arrow_function"
            | "method_definition" => {
                if let Some(name) = node.child_by_field_name("name") {
                    // A method name is a class property, not a lexical binding visible in its
                    // body. Function and generator names, by contrast, support self-reference.
                    if node.kind() != "method_definition" {
                        add_scope_binding(&mut scopes, node, name, source);
                    }
                    if matches!(
                        node.kind(),
                        "function_declaration" | "generator_function_declaration"
                    ) {
                        if let Some(scope) = nearest_lexical_scope(node.parent()) {
                            add_scope_binding(&mut scopes, scope, name, source);
                        }
                    }
                }
                if let Some(parameters) = node
                    .child_by_field_name("parameters")
                    .or_else(|| node.child_by_field_name("parameter"))
                {
                    add_scope_binding(&mut scopes, node, parameters, source);
                }
            }
            "class_declaration" | "abstract_class_declaration" | "class" => {
                if let Some(name) = node.child_by_field_name("name") {
                    add_scope_binding(&mut scopes, node, name, source);
                    if node.kind() != "class" {
                        if let Some(scope) = nearest_lexical_scope(node.parent()) {
                            add_scope_binding(&mut scopes, scope, name, source);
                        }
                    }
                }
            }
            "catch_clause" => {
                if let Some(parameter) = node.child_by_field_name("parameter") {
                    add_scope_binding(&mut scopes, node, parameter, source);
                }
            }
            "for_statement" | "for_in_statement" => {
                if let Some(binding) = node
                    .child_by_field_name("initializer")
                    .or_else(|| node.child_by_field_name("left"))
                {
                    let keyword = match binding.kind() {
                        "variable_declaration" => Some("var"),
                        "lexical_declaration" => Some("let"),
                        _ => loop_binding_keyword(node, binding),
                    };
                    let target = if keyword == Some("var") {
                        nearest_function_scope(node.parent())
                    } else if matches!(keyword, Some("let" | "const" | "using")) {
                        Some(node)
                    } else {
                        None
                    };
                    if let Some(target) = target {
                        if matches!(
                            binding.kind(),
                            "lexical_declaration" | "variable_declaration"
                        ) {
                            collect_declaration_bindings(&mut scopes, target, binding, source);
                        } else {
                            add_scope_binding(&mut scopes, target, binding, source);
                        }
                    }
                }
            }
            "variable_declarator" if !is_top_level_variable(node) => {
                if let (Some(declaration), Some(name)) =
                    (node.parent(), node.child_by_field_name("name"))
                {
                    let scope = if declaration.kind() == "variable_declaration" {
                        nearest_function_scope(declaration.parent())
                    } else {
                        nearest_lexical_scope(declaration.parent())
                    };
                    if let Some(scope) = scope {
                        add_scope_binding(&mut scopes, scope, name, source);
                    }
                }
            }
            "enum_declaration" if !is_module_declaration(node) => {
                if let (Some(scope), Some(name)) = (
                    nearest_lexical_scope(node.parent()),
                    node.child_by_field_name("name"),
                ) {
                    add_scope_binding(&mut scopes, scope, name, source);
                }
            }
            "internal_module" | "module"
                if !is_erased_declaration(node) && !is_module_declaration(node) =>
            {
                if let (Some(scope), Some(name)) = (
                    nearest_lexical_scope(node.parent()),
                    node.child_by_field_name("name"),
                ) {
                    add_scope_binding(&mut scopes, scope, name, source);
                }
            }
            _ => {}
        }
        super::ast::push_named_children_reverse(node, &mut stack);
    }
    let mut ranges = BTreeMap::<String, Vec<(usize, usize)>>::new();
    for (scope, names) in scopes {
        let Some(range) = scope_ranges.get(&scope).copied() else {
            continue;
        };
        for name in names {
            ranges.entry(name).or_default().push(range);
        }
    }
    for entries in ranges.values_mut() {
        entries.sort_unstable();
        let mut merged = Vec::<(usize, usize)>::with_capacity(entries.len());
        for &(start, end) in entries.iter() {
            if let Some(previous) = merged.last_mut().filter(|previous| start <= previous.1) {
                previous.1 = previous.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        *entries = merged;
    }
    ranges
}

fn position_is_shadowed(
    shadow_ranges: &BTreeMap<String, Vec<(usize, usize)>>,
    expected: &str,
    node: Node<'_>,
) -> bool {
    let Some(ranges) = shadow_ranges.get(expected) else {
        return false;
    };
    let index = ranges.partition_point(|(start, _)| *start <= node.start_byte());
    index > 0 && ranges[index - 1].1 >= node.end_byte()
}

fn add_scope_binding(
    scopes: &mut BTreeMap<usize, BTreeSet<String>>,
    scope: Node<'_>,
    pattern: Node<'_>,
    source: &[u8],
) {
    collect_binding_names(pattern, source, scopes.entry(scope.id()).or_default());
}

fn collect_declaration_bindings(
    scopes: &mut BTreeMap<usize, BTreeSet<String>>,
    scope: Node<'_>,
    declaration: Node<'_>,
    source: &[u8],
) {
    let mut cursor = declaration.walk();
    for declarator in declaration.named_children(&mut cursor) {
        if declarator.kind() == "variable_declarator" {
            if let Some(name) = declarator.child_by_field_name("name") {
                add_scope_binding(scopes, scope, name, source);
            }
        } else if matches!(
            declarator.kind(),
            "identifier" | "object_pattern" | "array_pattern"
        ) {
            add_scope_binding(scopes, scope, declarator, source);
        }
    }
}

fn loop_binding_keyword(loop_node: Node<'_>, binding: Node<'_>) -> Option<&'static str> {
    (0..loop_node.child_count()).find_map(|index| {
        let child = loop_node.child(index)?;
        (child.end_byte() <= binding.start_byte()).then(|| match child.kind() {
            "const" => Some("const"),
            "let" => Some("let"),
            "using" => Some("using"),
            "var" => Some("var"),
            _ => None,
        })?
    })
}

fn is_module_var(declarator: Node<'_>) -> bool {
    declarator
        .parent()
        .is_some_and(|declaration| declaration.kind() == "variable_declaration")
        && !is_erased_declaration(declarator)
        && nearest_function_scope(declarator.parent()).is_none()
}

fn is_local_scope(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "function_declaration"
            | "generator_function_declaration"
            | "function_expression"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
            | "class_declaration"
            | "abstract_class_declaration"
            | "class"
            | "class_static_block"
            | "statement_block"
            | "for_statement"
            | "for_in_statement"
            | "switch_body"
            | "catch_clause"
    )
}

fn nearest_function_scope(mut node: Option<Node<'_>>) -> Option<Node<'_>> {
    while let Some(candidate) = node {
        if matches!(
            candidate.kind(),
            "function_declaration"
                | "generator_function_declaration"
                | "function_expression"
                | "generator_function"
                | "arrow_function"
                | "method_definition"
                | "class_static_block"
        ) {
            return Some(candidate);
        }
        if candidate.kind() == "program" {
            return None;
        }
        node = candidate.parent();
    }
    None
}

fn nearest_lexical_scope(mut node: Option<Node<'_>>) -> Option<Node<'_>> {
    while let Some(candidate) = node {
        if matches!(
            candidate.kind(),
            "statement_block"
                | "for_statement"
                | "for_in_statement"
                | "switch_body"
                | "catch_clause"
                | "class_static_block"
        ) {
            return Some(candidate);
        }
        if candidate.kind() == "program" {
            return None;
        }
        node = candidate.parent();
    }
    None
}

fn is_top_level_variable(declarator: Node<'_>) -> bool {
    let Some(declaration) = declarator.parent() else {
        return false;
    };
    match declaration.parent() {
        Some(parent) if parent.kind() == "program" => true,
        Some(parent) if parent.kind() == "export_statement" => parent
            .parent()
            .is_some_and(|ancestor| ancestor.kind() == "program"),
        _ => false,
    }
}

fn is_module_declaration(node: Node<'_>) -> bool {
    let mut parent = node.parent();
    while let Some(candidate) = parent {
        match candidate.kind() {
            "program" => return true,
            // These grammar wrappers do not introduce a lexical scope for the declaration.
            "expression_statement" | "export_statement" => parent = candidate.parent(),
            _ => return false,
        }
    }
    false
}

fn is_erased_declaration(mut node: Node<'_>) -> bool {
    loop {
        if node.kind() == "ambient_declaration" {
            return true;
        }
        if node.kind() == "program" {
            return false;
        }
        let Some(parent) = node.parent() else {
            return false;
        };
        node = parent;
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
