use super::ast::{
    import_has_runtime_bindings, import_module, is_top_level_variable, is_type_only_declaration,
    is_type_only_specifier, string_literal,
};
use super::node_text;
use super::registrations::{registration_callee, RegistrationCallee, RegistrationNames};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

// These modules expose an `expect` factory CukeDedup can identify without module resolution. Exact
// matching avoids treating unrelated package-name prefixes as trusted APIs.
//
// Chain recognition itself is shape-based rather than matcher-name-based: an assertion is a call on
// a property chain that traces back to a trusted `expect(...)`, so Chai's `.to.equal` is recognized
// exactly as Jest's `.toBe` is. Only the provenance of the factory is listed here, which is why a
// runner is added by name rather than by dialect. Anything this list cannot name is declared
// through the `assertionModules` configuration key.
/// Property depth a write must reach past a copied slot before it crosses to the copied object.
///
/// A copy shares its source's property *values*, so replacing a slot on the copy changes nothing
/// the source can observe, while a write that reaches past the copy's own slots does reach it. Two
/// levels is the whole of that distinction: at this depth or beyond, a copied edge is always
/// crossed, so propagation depth has no effect on the result once it reaches here. The propagation
/// loop saturates depth at this ceiling, which both keeps the comparison meaningful and bounds the
/// work — an alias graph with a positive-delta cycle would otherwise raise the depth without limit
/// and never terminate on inputs, such as minified bundles, that close such a cycle.
const COPY_CROSS_DEPTH: usize = 2;

const ASSERTION_MODULES: [&str; 7] = [
    "@playwright/test",
    "playwright/test",
    "@jest/globals",
    "expect",
    "vitest",
    "chai",
    "bun:test",
];

#[derive(Default)]
pub(super) struct AssertionBindings {
    identifiers: BTreeSet<String>,
    namespaces: BTreeSet<String>,
    shadow_ranges: BTreeMap<String, Vec<(usize, usize)>>,
    mutated_matcher_factories: BTreeSet<String>,
}

impl AssertionBindings {
    pub(super) fn discover(
        root: Node<'_>,
        source: &[u8],
        registrations: &RegistrationNames,
        configured_modules: &BTreeSet<String>,
    ) -> Self {
        let mut facade_modules = resolved_facade_modules(root, source, registrations);
        // A project-declared module is trusted exactly like a recognized package: the analyzer
        // cannot see through a local re-export, so the declaration is the evidence.
        facade_modules.extend(configured_modules.iter().cloned());
        let (scopes, scope_ranges, scope_parents) = collect_binding_scopes(root, source);
        let shadow_ranges = collect_scoped_bindings(scopes.clone(), scope_ranges);
        let require_shadowed =
            module_runtime_binding_exists(root, source, "require", &shadow_ranges);
        let alias_writes = namespace_alias_writes(
            root,
            source,
            &scopes,
            &scope_parents,
            false,
            require_shadowed,
        );
        let mutated_matcher_factories = namespace_alias_writes(
            root,
            source,
            &scopes,
            &scope_parents,
            true,
            require_shadowed,
        );
        let mut bindings = Self::default();
        let mut shadowed = BTreeSet::new();
        let mut factory_writes = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "import_statement" if import_has_runtime_bindings(node) => {
                    let trusted =
                        bindings.collect_import(node, source, &facade_modules, configured_modules);
                    extend_untrusted_bindings(node, source, &trusted, &mut shadowed);
                }
                "variable_declarator" if is_top_level_variable(node) => {
                    // Binding names are file-scoped in this conservative model. Trust only
                    // module-scope CommonJS declarations so a nested helper cannot leak an
                    // assertion alias into unrelated step handlers.
                    let trusted = if !require_shadowed {
                        bindings.collect_require(node, source, configured_modules)
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
                _ => {
                    if let Some(left) = assignment_target(node) {
                        let mut assigned = BTreeSet::new();
                        let mut writes = BTreeMap::new();
                        collect_assignment_targets(left, source, &mut assigned, &mut writes, false);
                        factory_writes.extend(
                            writes
                                .into_keys()
                                .filter(|name| !position_is_shadowed(&shadow_ranges, name, node)),
                        );
                        shadowed.extend(
                            assigned
                                .into_iter()
                                .filter(|name| !position_is_shadowed(&shadow_ranges, name, node)),
                        );
                    }
                }
            }
            super::ast::push_named_children_reverse(node, &mut stack);
        }
        // Runtime declarations override trusted imports and aliases conservatively for the whole
        // file, matching step-registration discovery. A false negative is safer than assigning
        // assertion semantics to an unrelated function and manufacturing similarity evidence.
        shadowed.extend(factory_writes.intersection(&bindings.namespaces).cloned());
        shadowed.extend(alias_writes.intersection(&bindings.namespaces).cloned());
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
        bindings.mutated_matcher_factories = mutated_matcher_factories;
        bindings
    }

    pub(super) fn is_factory(&self, node: Node<'_>, source: &[u8]) -> bool {
        let Some(node) = super::registrations::unwrap_registration_callee(node) else {
            return false;
        };
        if node.kind() == "identifier" {
            let name = node_text(node, source);
            return self.identifiers.contains(name) && !self.is_locally_shadowed(node, name);
        }
        let Some(RegistrationCallee::Property { object, name }) = registration_callee(node, source)
        else {
            return false;
        };
        let namespace = node_text(object, source);
        object.kind() == "identifier"
            && self.namespaces.contains(namespace)
            && !self.is_locally_shadowed(object, namespace)
            && name == "expect"
    }

    pub(super) fn is_asymmetric_matcher(&self, node: Node<'_>, source: &[u8]) -> bool {
        self.asymmetric_matcher(node, source).is_some()
    }

    pub(super) fn asymmetric_matcher<'source>(
        &self,
        node: Node<'_>,
        source: &'source [u8],
    ) -> Option<(bool, Cow<'source, str>)> {
        if node.kind() != "call_expression" {
            return None;
        }
        let function = node
            .child_by_field_name("function")
            .and_then(super::registrations::unwrap_registration_callee)?;
        let RegistrationCallee::Property {
            mut object,
            name: matcher,
        } = registration_callee(function, source)?
        else {
            return None;
        };
        if !matches!(
            matcher.as_ref(),
            "objectContaining"
                | "arrayContaining"
                | "stringContaining"
                | "stringMatching"
                | "anything"
                | "any"
                | "closeTo"
        ) {
            return None;
        }
        let mut negated = false;
        if let Some(RegistrationCallee::Property { object: base, name }) =
            registration_callee(object, source)
        {
            if name == "not" {
                object = base;
                negated = true;
            }
        }
        let mut receiver = super::registrations::unwrap_registration_callee(object)?;
        while matches!(
            receiver.kind(),
            "member_expression" | "subscript_expression"
        ) {
            receiver = super::registrations::unwrap_registration_callee(
                receiver.child_by_field_name("object")?,
            )?;
        }
        (self.is_factory(object, source)
            && !self
                .mutated_matcher_factories
                .contains(node_text(receiver, source)))
        .then_some((negated, matcher))
    }

    fn is_locally_shadowed(&self, node: Node<'_>, expected: &str) -> bool {
        position_is_shadowed(&self.shadow_ranges, expected, node)
    }

    fn collect_import(
        &mut self,
        import: Node<'_>,
        source: &[u8],
        facade_modules: &BTreeSet<String>,
        configured_modules: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut trusted = BTreeSet::new();
        if is_type_only_declaration(import) {
            return trusted;
        }
        let Some(module) = assertion_import_module(import, source) else {
            return trusted;
        };
        let exact_assertion_module = ASSERTION_MODULES.contains(&module);
        // A declared module carries the same provenance a recognized package does, so every import
        // form it supports is trusted. A facade inferred from registration re-exports keeps the
        // narrower named-import rule: the declaration is explicit evidence, the inference is not.
        let declared_module = configured_modules.contains(module);
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
                    if node_text(name, source) == "expect"
                        || (module == "expect" && node_text(name, source) == "default")
                    {
                        let local = node.child_by_field_name("alias").unwrap_or(name);
                        let local = node_text(local, source).to_owned();
                        self.identifiers.insert(local.clone());
                        trusted.insert(local);
                    }
                }
                "namespace_import" if exact_assertion_module || declared_module => {
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
                        } else if exact_assertion_module || declared_module {
                            // `import fixtures = require('./fixtures')` binds the module the same
                            // way the namespace form does, so a declared module is trusted here too.
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

    fn collect_require(
        &mut self,
        declaration: Node<'_>,
        source: &[u8],
        configured_modules: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut trusted = BTreeSet::new();
        let (Some(name), Some(value)) = (
            declaration.child_by_field_name("name"),
            declaration.child_by_field_name("value"),
        ) else {
            return trusted;
        };
        // A direct property read of the module is the factory when it names `expect`. Route it
        // through the shared resolver so the computed spelling is recognized as the dotted one is.
        let expect_property = (name.kind() == "identifier")
            .then(|| registration_callee(value, source))
            .flatten()
            .and_then(|callee| match callee {
                RegistrationCallee::Property { object, name } if name == "expect" => Some(object),
                _ => None,
            });
        let member_expect = expect_property.is_some();
        let required = expect_property.unwrap_or(value);
        let Some(module) = required_module(required, source) else {
            return trusted;
        };
        // A declared module is trusted exactly as a recognized package is, so a CommonJS project
        // does not lose the setting by spelling the dependency with `require`. A facade merely
        // inferred from registration re-exports is deliberately excluded: inference is not evidence
        // that the module exposes a real assertion factory, and trusting it would make an unrelated
        // `expect` export semantically load-bearing.
        if !ASSERTION_MODULES.contains(&module) && !configured_modules.contains(module) {
            return trusted;
        }
        match name.kind() {
            "identifier" if module == "expect" || member_expect => {
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
                // The trust pass wants names only; the distances matter to the mutation pass.
                let mut destructured = BTreeMap::new();
                collect_expect_pattern(
                    name,
                    source,
                    &mut destructured,
                    &mut trusted,
                    &mut BTreeMap::new(),
                    false,
                );
                self.identifiers.extend(destructured.into_keys());
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
            if let Some(module) = import_module(node, source)
                .filter(|module| registrations.module_paths.contains_key(*module))
            {
                // Built-in registration packages are not evidence of an assertion facade.
                modules.insert(module.to_owned());
            }
        }
        super::ast::push_named_children_reverse(node, &mut stack);
    }
    let paths: BTreeSet<_> = modules
        .iter()
        .filter_map(|module| registrations.module_paths.get(module))
        .collect();
    modules.extend(
        registrations
            .module_paths
            .iter()
            .filter_map(|(module, path)| paths.contains(path).then_some(module.clone())),
    );
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
            "required_parameter" | "optional_parameter" => {
                if let Some(binding) = node
                    .child_by_field_name("name")
                    .or_else(|| node.child_by_field_name("pattern"))
                {
                    stack.push(binding);
                }
                continue;
            }
            "type_annotation" => continue,
            _ => {}
        }
        super::ast::push_named_children_reverse(node, &mut stack);
    }
}

/// Collects the bindings an assignment target names, and for each receiver the **depth** of the
/// property path written through it: `copy.not = x` is depth 1, `copy.not.objectContaining = x` is
/// depth 2. Depth separates replacing a slot from mutating the object held in it, which is exactly
/// the difference between a copy and the thing it was copied from. Where one receiver is written
/// at several depths the largest wins: a deep write reaches shared state whatever else the file
/// does.
fn collect_assignment_targets(
    root: Node<'_>,
    source: &[u8],
    output: &mut BTreeSet<String>,
    factory_writes: &mut BTreeMap<String, usize>,
    matcher_members: bool,
) {
    let mut stack = vec![(root, 0usize)];
    while let Some((node, depth)) = stack.pop() {
        match node.kind() {
            "identifier" | "shorthand_property_identifier_pattern" => {
                output.insert(node_text(node, source).to_owned());
            }
            // Only writes that can replace the factory invalidate namespace trust.
            "member_expression" | "subscript_expression" => {
                let property = node
                    .child_by_field_name("property")
                    .filter(|property| !node_text(*property, source).contains('\\'))
                    .map(|property| node_text(property, source).to_owned())
                    .or_else(|| {
                        // An unreadable key must invalidate trust, not look like an unrelated key.
                        node.child_by_field_name("index")
                            .and_then(|index| super::matcher::static_string_key(index, source))
                    });
                if property.is_none_or(|property| {
                    property == "expect" || matcher_members && is_matcher_member(property.as_str())
                }) {
                    if let Some(object) = node
                        .child_by_field_name("object")
                        .and_then(super::registrations::unwrap_registration_callee)
                    {
                        if matcher_members
                            && matches!(object.kind(), "member_expression" | "subscript_expression")
                        {
                            stack.push((object, depth + 1));
                        } else if object.kind() == "identifier" {
                            let reached = depth + 1;
                            factory_writes
                                .entry(node_text(object, source).to_owned())
                                .and_modify(|deepest| *deepest = (*deepest).max(reached))
                                .or_insert(reached);
                        }
                    }
                }
                continue;
            }
            "pair" | "pair_pattern" => {
                if let Some(value) = node.child_by_field_name("value") {
                    stack.push((value, depth));
                }
                continue;
            }
            "assignment_pattern" | "object_assignment_pattern" => {
                if let Some(left) = node
                    .child_by_field_name("left")
                    .or_else(|| node.named_child(0))
                {
                    stack.push((left, depth));
                }
                continue;
            }
            "type_annotation" => continue,
            _ => {}
        }
        let mut children = Vec::new();
        super::ast::push_named_children_reverse(node, &mut children);
        stack.extend(children.into_iter().map(|child| (child, depth)));
    }
}

fn assignment_target(node: Node<'_>) -> Option<Node<'_>> {
    match node.kind() {
        "assignment_expression" | "augmented_assignment_expression" => {
            node.child_by_field_name("left")
        }
        "update_expression" => node.child_by_field_name("argument"),
        "unary_expression"
            if node
                .child_by_field_name("operator")
                .is_some_and(|op| op.kind() == "delete") =>
        {
            node.child_by_field_name("argument")
        }
        "for_in_statement" => node
            .child_by_field_name("left")
            .filter(|left| loop_binding_keyword(node, *left).is_none()),
        _ => None,
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
        let assignment = assignment_target(node);
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
            _ => assignment.filter(|left| {
                matches!(
                    left.kind(),
                    "identifier" | "object_pattern" | "array_pattern"
                ) && !position_is_shadowed(shadow_ranges, expected, node)
            }),
        };
        if let Some(pattern) = pattern {
            let mut names = BTreeSet::new();
            if assignment.is_some() {
                collect_assignment_targets(
                    pattern,
                    source,
                    &mut names,
                    &mut BTreeMap::new(),
                    false,
                );
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

type BindingScopes = BTreeMap<usize, BTreeSet<String>>;
type ScopeRanges = BTreeMap<usize, (usize, usize)>;
/// Nearest enclosing local scope for every local-scope node; scopes at top level map to the
/// root's id. Built on the same walk that collects bindings, so the chain and the binding map
/// agree on which nodes count as scopes.
type ScopeParents = BTreeMap<usize, usize>;

fn namespace_alias_writes(
    root: Node<'_>,
    source: &[u8],
    scopes: &BindingScopes,
    scope_parents: &ScopeParents,
    matcher_members: bool,
    require_shadowed: bool,
) -> BTreeSet<String> {
    // A may-alias graph only removes trust; it never promotes an alias to an assertion
    // factory. Keep prior assignments conservatively and identify lexical bindings, not
    // spellings, so a shadowed local receiver cannot contaminate a module namespace.
    //
    // Resolution follows the precomputed scope chain rather than the node's ancestors: every id
    // `scopes` can hold is a local-scope node, so the chain visits exactly the ancestors the
    // lookup can hit, without paying for `Node::parent`'s root-restarting search on each step.
    // The chain starts at the enclosing scope carried by the walk. A lookup target is never
    // itself a scope that could bind the name sooner: aliasing targets are identifiers, patterns
    // and member expressions, and the require lookup only proceeds for a `require(...)` call
    // node, so a scope-kind target — an arrow initializer, say — is rejected by `required_module`
    // before its bindings could matter.
    let key = |name: &str, mut scope: usize| {
        while scope != root.id() {
            if scopes.get(&scope).is_some_and(|names| names.contains(name)) {
                return (scope, name.to_owned());
            }
            scope = scope_parents[&scope];
        }
        (root.id(), name.to_owned())
    };
    let mut required_aliases = BTreeMap::new();
    // Each edge records how many property levels the local sits below its owner, and whether the
    // step is a copy. An object rest binding holds the same property *values* as the object it
    // copied, so a write that reaches past its own slots reaches that object too, while a write to
    // a slot itself does not. Depth has to travel along the edges: `const negated = copy.not`
    // makes a depth-1 write on `negated` a depth-2 write on `copy`.
    let mut aliases = BTreeMap::<_, BTreeSet<(_, usize, bool)>>::new();
    let mut writes = BTreeMap::<_, usize>::new();
    let mut stack = vec![(root, root.id())];
    while let Some((node, scope)) = stack.pop() {
        let edge = match node.kind() {
            "variable_declarator" => node
                .child_by_field_name("name")
                .zip(node.child_by_field_name("value")),
            "assignment_pattern" | "object_assignment_pattern" => node
                .child_by_field_name("left")
                .zip(node.child_by_field_name("right")),
            "required_parameter" | "optional_parameter" => node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("pattern"))
                .zip(node.child_by_field_name("value")),
            "assignment_expression" => node
                .child_by_field_name("left")
                .zip(node.child_by_field_name("right")),
            "augmented_assignment_expression"
                if node
                    .child_by_field_name("operator")
                    .is_some_and(|operator| matches!(operator.kind(), "||=" | "&&=" | "??=")) =>
            {
                node.child_by_field_name("left")
                    .zip(node.child_by_field_name("right"))
            }
            _ => None,
        };
        if let Some((left, right)) = edge {
            if let Some(right) = super::registrations::unwrap_registration_callee(right) {
                let mut locals = BTreeMap::new();
                let mut shallow_locals = BTreeMap::new();
                let mut owners = BTreeMap::new();
                if left.kind() == "identifier" {
                    locals.insert(node_text(left, source).to_owned(), 0);
                } else if matcher_members && left.kind() == "object_pattern" {
                    collect_expect_pattern(
                        left,
                        source,
                        &mut locals,
                        &mut BTreeSet::new(),
                        &mut shallow_locals,
                        true,
                    );
                }
                let required = match registration_callee(right, source) {
                    Some(RegistrationCallee::Property { object, name }) if name == "expect" => {
                        object
                    }
                    _ => right,
                };
                // Same-module loads share mutation provenance, never callable factory trust. Two
                // `require()` calls for one module return the same cached object, so a write
                // through either binding is a write to the other — which is as true of replacing
                // the factory as it is of mutating a matcher below it, so this link is not
                // restricted to the matcher pass. Respect both module-level replacement and
                // lexical shadowing of require.
                if !require_shadowed && key("require", scope).0 == root.id() {
                    if let Some(module) = required_module(required, source)
                        .filter(|module| ASSERTION_MODULES.contains(module))
                    {
                        for (local, copied) in locals
                            .keys()
                            .map(|local| (local, false))
                            .chain(shallow_locals.keys().map(|local| (local, true)))
                        {
                            let local = key(local, scope);
                            let owner = required_aliases
                                .entry(module)
                                .or_insert_with(|| local.clone());
                            aliases.entry(owner.clone()).or_default().insert((
                                local.clone(),
                                0,
                                false,
                            ));
                            aliases
                                .entry(local)
                                .or_default()
                                .insert((owner.clone(), 0, copied));
                        }
                    }
                }
                if right.kind() == "identifier" {
                    owners.insert(node_text(right, source).to_owned(), 0);
                } else if matcher_members
                    && matches!(right.kind(), "member_expression" | "subscript_expression")
                {
                    // Only direct property access establishes an alias, not a call's arguments.
                    let mut owner_writes = BTreeMap::new();
                    collect_assignment_targets(
                        right,
                        source,
                        &mut BTreeSet::new(),
                        &mut owner_writes,
                        true,
                    );
                    owners.extend(owner_writes);
                }
                for (local, local_delta, copied) in locals
                    .iter()
                    .map(|(local, delta)| (local, delta, false))
                    .chain(
                        shallow_locals
                            .iter()
                            .map(|(local, delta)| (local, delta, true)),
                    )
                {
                    for (owner, owner_delta) in &owners {
                        aliases.entry(key(local, scope)).or_default().insert((
                            key(owner, scope),
                            local_delta + owner_delta,
                            copied,
                        ));
                    }
                }
            }
        }
        if let Some(target) = assignment_target(node) {
            let mut receivers = BTreeMap::new();
            collect_assignment_targets(
                target,
                source,
                &mut BTreeSet::new(),
                &mut receivers,
                matcher_members,
            );
            for (name, depth) in receivers {
                writes
                    .entry(key(&name, scope))
                    .and_modify(|deepest| *deepest = (*deepest).max(depth))
                    .or_insert(depth);
            }
        }
        let scope = if is_local_scope(node) {
            node.id()
        } else {
            scope
        };
        super::ast::push_named_children_reverse_scoped(node, scope, &mut stack);
    }
    // Record the deepest write already propagated from each binding rather than merely that it was
    // seen. A binding reached first by a shallow path and later by a deeper one must be revisited,
    // or the answer would depend on the order the file happens to be walked in.
    let mut deepest = BTreeMap::<_, usize>::new();
    let mut modules = BTreeSet::new();
    // Depth saturates at `COPY_CROSS_DEPTH`: it is only ever compared against that ceiling, so a
    // deeper reach decides nothing a reach at the ceiling has not already decided. Saturating it
    // bounds every binding to at most three admissions and guarantees termination even when the
    // alias graph closes a positive-delta cycle, as minified bundles do through dynamically-keyed
    // subscript reassignments among reused names.
    let mut pending: Vec<_> = writes
        .into_iter()
        .map(|(binding, depth)| (binding, depth.min(COPY_CROSS_DEPTH)))
        .collect();
    while let Some((binding, depth)) = pending.pop() {
        if deepest
            .get(&binding)
            .is_some_and(|reached| *reached >= depth)
        {
            continue;
        }
        deepest.insert(binding.clone(), depth);
        if binding.0 == root.id() {
            modules.insert(binding.1.clone());
        }
        if let Some(sources) = aliases.get(&binding) {
            for (source, delta, copied) in sources {
                // Replacing a slot on a copy changes nothing the copied object can observe, so a
                // write has to reach past that slot before it crosses. Without this, one
                // `copy.not = x` would revoke trust for every assertion the module reaches.
                if *copied && depth < COPY_CROSS_DEPTH {
                    continue;
                }
                pending.push((source.clone(), (depth + delta).min(COPY_CROSS_DEPTH)));
            }
        }
    }
    modules
}

fn collect_binding_scopes(
    root: Node<'_>,
    source: &[u8],
) -> (BindingScopes, ScopeRanges, ScopeParents) {
    let mut scopes = BTreeMap::<usize, BTreeSet<String>>::new();
    let mut scope_ranges = BTreeMap::new();
    let mut scope_parents = BTreeMap::new();
    let mut stack = vec![(root, root.id())];
    while let Some((node, enclosing)) = stack.pop() {
        let scope = if is_local_scope(node) {
            scope_ranges.insert(node.id(), (node.start_byte(), node.end_byte()));
            scope_parents.insert(node.id(), enclosing);
            node.id()
        } else {
            enclosing
        };
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
                    // A declaration head binds through `variable_declarator` children, which the
                    // generic declarator arm below already registers against the same scope: the
                    // loop itself for `let`/`const`, and the enclosing function for `var`. Only a
                    // bare pattern head, which has no declarator to visit, needs registering here.
                    if let Some(target) = target {
                        if !matches!(
                            binding.kind(),
                            "lexical_declaration" | "variable_declaration"
                        ) {
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
        super::ast::push_named_children_reverse_scoped(node, scope, &mut stack);
    }
    (scopes, scope_ranges, scope_parents)
}

fn collect_scoped_bindings(
    scopes: BindingScopes,
    scope_ranges: ScopeRanges,
) -> BTreeMap<String, Vec<(usize, usize)>> {
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
            | "internal_module"
            | "module"
            | "statement_block"
            | "for_statement"
            | "for_in_statement"
            | "switch_body"
            | "catch_clause"
    )
}

pub(super) fn nearest_function_scope(mut node: Option<Node<'_>>) -> Option<Node<'_>> {
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
                | "internal_module"
                | "module"
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
                | "internal_module"
                | "module"
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

/// Property names that reach a matcher builder from a trusted assertion factory.
///
/// Shared so the nested destructuring collector supports exactly the path the flat member walker
/// supports; a name handled by one and not the other makes equivalent spellings disagree.
fn is_matcher_member(name: &str) -> bool {
    matches!(
        name,
        "not"
            | "objectContaining"
            | "arrayContaining"
            | "stringContaining"
            | "stringMatching"
            | "anything"
            | "any"
            | "closeTo"
    )
}

/// Names the property a shorthand binding reads, or `None` when the spelling cannot be read.
///
/// An identifier may carry unicode escapes — `n\u006ft` is the identifier `not` — and nothing in
/// this module decodes them; every binding, write and registration is matched by raw spelling. An
/// unreadable spelling is therefore unknown, never a known unrelated property, so the mutation
/// pass keeps it as a possible alias instead of leaving trust in place. Two *different* spellings
/// of one identifier still do not match each other, which is the same limitation the flat alias
/// path has; normalizing here alone would make this view disagree with registration and shadowing
/// and silently lose real mutations.
fn shorthand_key_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    let text = node_text(node, source);
    (!text.contains('\\')).then(|| text.to_owned())
}

fn collect_expect_pattern(
    pattern: Node<'_>,
    source: &[u8],
    identifiers: &mut BTreeMap<String, usize>,
    trusted: &mut BTreeSet<String>,
    shallow: &mut BTreeMap<String, usize>,
    include_defaults: bool,
) {
    // An explicit worklist rather than recursion. A bound here would have to decide what the
    // bindings below it mean, and both answers are wrong: stopping silently keeps trust that a
    // write should have removed, while sweeping everything below ignores the property filter the
    // rest of this walk applies and revokes trust a write never touched. Walking iteratively keeps
    // one rule at every depth and costs no stack.
    //
    // Each local is reported with how many property levels it sits below the destructured value,
    // so an alias edge can carry that distance: `const { not: negated } = copy` makes a depth-1
    // write on `negated` a depth-2 write on `copy`. Only the outermost level can name a trusted
    // factory — reaching deeper may remove trust but never establishes it.
    let mut pending = vec![(pattern, 0usize)];
    while let Some((pattern, depth)) = pending.pop() {
        let top_level = depth == 0;
        let mut cursor = pattern.walk();
        for property in pattern.named_children(&mut cursor) {
            match property.kind() {
                "object_assignment_pattern" if include_defaults => {
                    if let Some(left) = property.child_by_field_name("left") {
                        let name = shorthand_key_name(left, source);
                        if name
                            .as_deref()
                            .is_none_or(|name| name == "expect" || is_matcher_member(name))
                        {
                            identifiers.insert(node_text(left, source).to_owned(), depth + 1);
                        }
                    }
                }
                // An object rest binding copies the remaining property *values* into a fresh
                // object, so it holds the very objects the source holds. It is reported apart
                // from a direct alias because only writes that reach past its own slots can be
                // seen by the source.
                "rest_pattern" if include_defaults => {
                    if let Some(binding) = property.named_child(0) {
                        if binding.kind() == "identifier" {
                            // A rest binding holds the same properties as the value it copied,
                            // so it sits at that value's own level, not one below it.
                            shallow.insert(node_text(binding, source).to_owned(), depth);
                        }
                    }
                }
                "shorthand_property_identifier_pattern" => {
                    let name = shorthand_key_name(property, source);
                    let known_expect = name.as_deref() == Some("expect");
                    let known_not = name.as_deref().is_some_and(is_matcher_member);
                    if known_expect || (include_defaults && (known_not || name.is_none())) {
                        let local = node_text(property, source).to_owned();
                        identifiers.insert(local.clone(), depth + 1);
                        if known_expect && top_level {
                            trusted.insert(local);
                        }
                    }
                }
                "pair_pattern" => {
                    let (Some(key), Some(value)) = (
                        property.child_by_field_name("key"),
                        property.child_by_field_name("value"),
                    ) else {
                        continue;
                    };
                    let computed = key.kind() == "computed_property_name";
                    let key = if computed {
                        key.named_child(0).unwrap_or(key)
                    } else {
                        key
                    };
                    // Defaults can create mutation aliases, but cannot establish factory trust.
                    let value = if include_defaults
                        && matches!(
                            value.kind(),
                            "assignment_pattern" | "object_assignment_pattern"
                        ) {
                        value.child_by_field_name("left").unwrap_or(value)
                    } else {
                        value
                    };
                    // Mirror the property classification the flat alias path applies: a key the
                    // decoder cannot read is *unknown*, never a known unrelated property. An
                    // unknown key may still name the matcher, so the mutation pass has to keep it
                    // as a possible alias instead of stopping the descent and leaving trust in
                    // place.
                    let text = node_text(key, source);
                    let name = if key.kind() == "string" {
                        super::matcher::static_string_key(key, source)
                    } else if computed || text.contains('\\') {
                        None
                    } else {
                        Some(text.to_owned())
                    };
                    let known_expect = name.as_deref() == Some("expect");
                    let known_not = name.as_deref().is_some_and(is_matcher_member);
                    // Unknown computed keys may alias the factory, but cannot establish trust.
                    // include_defaults is used only by the trust-removing mutation pass.
                    let supported =
                        known_expect || (include_defaults && (known_not || name.is_none()));
                    if supported && value.kind() == "identifier" {
                        let local = node_text(value, source).to_owned();
                        identifiers.insert(local.clone(), depth + 1);
                        if known_expect && top_level {
                            trusted.insert(local);
                        }
                    } else if supported && include_defaults && value.kind() == "object_pattern" {
                        // `{ expect: { not: negated } }` names the same object as
                        // `api.expect.not`, so a write through either must revoke trust
                        // identically. An unsupported key stops the descent, matching the property
                        // filter the flat path applies.
                        pending.push((value, depth + 1));
                    }
                }
                _ => {}
            }
        }
    }
}
