use super::ast::{
    export_has_runtime_bindings, framework_for_module, import_has_runtime_bindings,
    import_has_runtime_module_reference, import_module, is_supported_module,
    is_type_only_declaration, is_type_only_specifier, push_named_children_reverse,
    registration_exports_for_framework, registration_exports_for_module, string_literal,
    RegistrationExport, RegistrationExportKind, RegistrationExports, DEFAULT_REGISTRATIONS,
    REGISTRATIONS,
};
use super::matcher::decode_js_string;
use super::module_resolver::{merge_framework, RegistrationResolver};
use super::node_text;
use crate::model::Framework;
use anyhow::Result;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use tree_sitter::Node;

/// A specifier that failed static resolution, with the import that requested it.
pub(super) struct UnresolvedModuleReason {
    pub(super) reason: String,
    pub(super) row: usize,
    pub(super) byte_offset: usize,
    pub(super) byte_column: usize,
}

pub(super) struct RegistrationNames {
    pub(super) module_paths: BTreeMap<String, std::path::PathBuf>,
    aliases: RegistrationExports,
    namespaces: BTreeMap<String, RegistrationExports>,
    unresolved_aliases: BTreeMap<String, String>,
    unresolved_namespaces: BTreeMap<String, String>,
    /// Why a specifier could not be resolved statically, keyed by module specifier.
    unresolved_reasons: BTreeMap<String, UnresolvedModuleReason>,
    pub(super) framework: Framework,
}

impl RegistrationNames {
    pub(super) fn recognizes_alias(&self, name: &str) -> bool {
        self.aliases.contains_key(name)
    }

    pub(super) fn recognizes_namespace(&self, name: &str) -> bool {
        self.namespaces.contains_key(name)
    }
}

pub(super) enum RegistrationCallee<'tree, 'source> {
    Identifier(&'source str),
    Property {
        object: Node<'tree>,
        name: Cow<'source, str>,
    },
}

#[derive(Default)]
struct RegistrationDiscovery<'tree> {
    aliases: RegistrationExports,
    namespaces: BTreeMap<String, RegistrationExports>,
    assignments: Vec<(String, String)>,
    namespace_destructures: Vec<(Node<'tree>, String)>,
    shadowed_defaults: BTreeSet<String>,
    unresolved_aliases: BTreeMap<String, String>,
    unresolved_namespaces: BTreeMap<String, String>,
    unresolved_reasons: BTreeMap<String, UnresolvedModuleReason>,
    /// Local identifiers proven to be Playwright-BDD's `createBdd` export.
    create_bdd_factories: BTreeSet<String>,
    /// Locally declared functions that may forward to a registration, resolved after imports.
    wrapper_candidates: Vec<WrapperCandidate>,
}

/// A local function that forwards its own leading parameters to `forwards_to`.
///
/// The forward is validated once, when the declaration is visited, so resolution afterwards is a
/// pure name lookup rather than a repeated syntax-tree walk.
struct WrapperCandidate {
    name: String,
    forwards_to: String,
}

pub(super) fn registration_name(
    function: Node<'_>,
    source: &[u8],
    registrations: &RegistrationNames,
) -> Option<(String, String, Framework)> {
    let (callee, registration) = registration_binding(function, source, registrations)?;
    (registration.kind == RegistrationExportKind::Call).then(|| {
        (
            callee,
            registration.canonical.clone(),
            registration.framework,
        )
    })
}

/// Resolves only registrations exported as class-method decorators.
pub(super) fn decorator_registration_name(
    function: Node<'_>,
    source: &[u8],
    registrations: &RegistrationNames,
) -> Option<(String, String, Framework)> {
    let (callee, registration) = registration_binding(function, source, registrations)?;
    (registration.kind == RegistrationExportKind::Decorator).then(|| {
        (
            callee,
            registration.canonical.clone(),
            registration.framework,
        )
    })
}

fn registration_binding<'a>(
    function: Node<'_>,
    source: &[u8],
    registrations: &'a RegistrationNames,
) -> Option<(String, &'a RegistrationExport)> {
    let registration = match registration_callee(function, source)? {
        RegistrationCallee::Identifier(name) => registrations.aliases.get(name),
        RegistrationCallee::Property { object, name } => registrations
            .namespaces
            .get(node_text(object, source))?
            .get(name.as_ref()),
    }?;
    Some((node_text(function, source).to_owned(), registration))
}

/// Returns why `module` could not be resolved statically, when a specific cause was recorded.
pub(super) fn unresolved_module_reason<'a>(
    registrations: &'a RegistrationNames,
    module: &str,
) -> Option<&'a str> {
    registrations
        .unresolved_reasons
        .get(module)
        .map(|recorded| recorded.reason.as_str())
}

/// Returns every specifier that failed static resolution, with its cause.
pub(super) fn unresolved_module_reasons(
    registrations: &RegistrationNames,
) -> impl Iterator<Item = (&str, &UnresolvedModuleReason)> {
    registrations
        .unresolved_reasons
        .iter()
        .map(|(module, recorded)| (module.as_str(), recorded))
}

pub(super) fn unresolved_registration_module<'a>(
    function: Node<'_>,
    source: &[u8],
    registrations: &'a RegistrationNames,
) -> Option<&'a str> {
    match registration_callee(function, source)? {
        RegistrationCallee::Identifier(name) => registrations
            .unresolved_aliases
            .get(name)
            .map(String::as_str),
        RegistrationCallee::Property { object, name } if REGISTRATIONS.contains(&name.as_ref()) => {
            registrations
                .unresolved_namespaces
                .get(node_text(object, source))
                .map(String::as_str)
        }
        RegistrationCallee::Property { .. } => None,
    }
}

pub(super) fn registration_callee<'tree, 'source>(
    function: Node<'tree>,
    source: &'source [u8],
) -> Option<RegistrationCallee<'tree, 'source>> {
    let function = unwrap_registration_callee(function)?;
    match function.kind() {
        "identifier" => Some(RegistrationCallee::Identifier(node_text(function, source))),
        "member_expression" => Some(RegistrationCallee::Property {
            object: unwrap_registration_callee(function.child_by_field_name("object")?)?,
            name: Cow::Borrowed(node_text(function.child_by_field_name("property")?, source)),
        }),
        "subscript_expression" => Some(RegistrationCallee::Property {
            object: unwrap_registration_callee(function.child_by_field_name("object")?)?,
            name: Cow::Owned(decode_js_string(node_text(
                function.child_by_field_name("index")?,
                source,
            ))?),
        }),
        _ => None,
    }
}

pub(super) fn unwrap_registration_callee(mut function: Node<'_>) -> Option<Node<'_>> {
    loop {
        function = match function.kind() {
            "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression" => function.named_child(0)?,
            "type_assertion" => {
                let last_child =
                    u32::try_from(function.named_child_count().checked_sub(1)?).ok()?;
                function.named_child(last_child)?
            }
            "instantiation_expression" => function.named_child(0)?,
            _ => return Some(function),
        };
    }
}

pub(super) fn detect_framework(root: Node<'_>, source: &[u8]) -> Framework {
    let mut stack = vec![root];
    let mut framework = Framework::Unknown;
    while let Some(node) = stack.pop() {
        let runtime_module_reference = match node.kind() {
            "import_statement" => import_has_runtime_module_reference(node),
            "export_statement" => export_has_runtime_bindings(node),
            _ => false,
        };
        if runtime_module_reference {
            if let Some(module) = import_module(node, source) {
                framework = merge_framework(framework, framework_for_module(module));
            }
        } else if node.kind() == "call_expression" && call_name(node, source) == Some("require") {
            if let Some(evidence) = call_string_argument(node, source).map(framework_for_module) {
                framework = merge_framework(framework, evidence);
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
    framework
}

pub(super) fn detect_registrations(
    root: Node<'_>,
    source: &[u8],
    framework: Framework,
    file_path: &Path,
    resolver: &mut RegistrationResolver,
    configured: &BTreeSet<String>,
) -> Result<RegistrationNames> {
    let mut discovered = RegistrationDiscovery {
        create_bdd_factories: collect_create_bdd_factories(root, source, file_path, resolver)?,
        ..RegistrationDiscovery::default()
    };
    let mut effective_framework = framework;
    let mut module_paths = BTreeMap::new();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_statement" if import_has_runtime_bindings(node) => {
                let module = import_module(node, source);
                let exports = match module {
                    Some(module) if is_supported_module(module) => {
                        Some(registration_exports_for_module(module))
                    }
                    Some(module) => {
                        let outcome = resolver.registration_exports(file_path, module)?;
                        if let Some(reason) = outcome.reason {
                            discovered
                                .unresolved_reasons
                                .entry(module.to_owned())
                                .or_insert(UnresolvedModuleReason {
                                    reason,
                                    row: node.start_position().row,
                                    byte_offset: node.start_byte(),
                                    byte_column: node.start_position().column,
                                });
                        }
                        outcome.resolution.map(|resolution| {
                            if let Some(path) = resolution.module_path {
                                module_paths.insert(module.to_owned(), path);
                            }
                            effective_framework =
                                merge_framework(effective_framework, resolution.framework);
                            resolution.exports
                        })
                    }
                    _ => None,
                };
                if !is_type_only_declaration(node)
                    && exports.as_ref().is_some_and(|exports| !exports.is_empty())
                {
                    collect_imports(
                        node,
                        source,
                        &exports.unwrap_or_default(),
                        &mut discovered.aliases,
                        &mut discovered.namespaces,
                        &mut discovered.create_bdd_factories,
                    );
                } else if !is_type_only_declaration(node) && exports.is_none() {
                    if let Some(module) = module {
                        collect_unresolved_imports(node, source, module, &mut discovered);
                    }
                }
                // Every runtime import owns its local bindings, including unsupported exports
                // from an otherwise known module. Otherwise `Fixture as Given`, for example,
                // would accidentally re-enable the ambient `Given` fallback.
                collect_shadowing_imports(node, source, &mut discovered.shadowed_defaults);
            }
            "export_statement"
                if export_has_runtime_bindings(node)
                    && import_module(node, source).is_some_and(is_supported_module) =>
            {
                if let Some(module) = import_module(node, source) {
                    let available = registration_exports_for_module(module);
                    collect_exports(node, source, &available, &mut discovered.aliases);
                }
            }
            "variable_declarator" => collect_variable_registration(node, source, &mut discovered),
            "function_declaration" => {
                shadow_named_declaration(node, source, &mut discovered.shadowed_defaults);
                collect_wrapper_candidate(node, source, &mut discovered);
            }
            "generator_function_declaration" => {
                shadow_named_declaration(node, source, &mut discovered.shadowed_defaults)
            }
            "class_declaration"
            | "abstract_class_declaration"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration" => {
                shadow_named_declaration(node, source, &mut discovered.shadowed_defaults)
            }
            "assignment_expression" => collect_assignment(node, source, &mut discovered),
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }

    for (pattern, namespace) in &discovered.namespace_destructures {
        if let Some(exports) = discovered.namespaces.get(namespace) {
            collect_pattern_aliases(*pattern, source, exports, &mut discovered.aliases);
        }
    }

    for name in DEFAULT_REGISTRATIONS {
        if !discovered.shadowed_defaults.contains(name) && !discovered.aliases.contains_key(name) {
            discovered.aliases.insert(
                name.to_owned(),
                RegistrationExport {
                    canonical: name.to_owned(),
                    kind: RegistrationExportKind::Call,
                    framework: effective_framework,
                },
            );
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for (alias, target) in &discovered.assignments {
            if let Some(registration) = discovered.aliases.get(target).cloned() {
                changed |= discovered
                    .aliases
                    .insert(alias.clone(), registration)
                    .is_none();
            }
        }
    }

    // Project-declared wrappers win over inference: the operator asserted these names register
    // steps, so they apply even when the body is too dynamic to analyze.
    for name in configured {
        discovered
            .aliases
            .entry(name.clone())
            .or_insert_with(|| RegistrationExport {
                canonical: name.clone(),
                kind: RegistrationExportKind::Call,
                framework: effective_framework,
            });
    }
    resolve_wrapper_candidates(&mut discovered);

    Ok(RegistrationNames {
        module_paths,
        aliases: discovered.aliases,
        namespaces: discovered.namespaces,
        unresolved_aliases: discovered.unresolved_aliases,
        unresolved_namespaces: discovered.unresolved_namespaces,
        unresolved_reasons: discovered.unresolved_reasons,
        framework: effective_framework,
    })
}

/// Records a locally declared function that may be a thin step-registration wrapper.
fn collect_wrapper_candidate<'tree>(
    declaration: Node<'tree>,
    source: &[u8],
    discovered: &mut RegistrationDiscovery<'tree>,
) {
    // Registration aliases are file-scoped. Inferring a nested declaration as a global alias
    // would leak that local binding into unrelated call sites elsewhere in the source.
    let Some(parent) = declaration.parent() else {
        return;
    };
    let top_level = parent.kind() == "program"
        || (parent.kind() == "export_statement"
            && parent.parent().is_some_and(|node| node.kind() == "program"));
    // Calling an async function or generator does not guarantee that its body reaches the
    // registration call immediately. Dynamic wrappers remain available through configuration.
    if !top_level
        || node_text(declaration, source)
            .trim_start()
            .starts_with("async ")
    {
        return;
    }
    let (Some(name), Some(parameters), Some(body)) = (
        declaration.child_by_field_name("name"),
        declaration.child_by_field_name("parameters"),
        declaration.child_by_field_name("body"),
    ) else {
        return;
    };
    if name.kind() != "identifier" {
        return;
    }
    let Some(parameters) = plain_parameter_names(parameters, source) else {
        return;
    };
    if parameters.len() < 2 {
        return;
    }
    let name = node_text(name, source).to_owned();
    let Some(forwards_to) = forwarded_callee(body, &name, &parameters, source) else {
        return;
    };
    discovered
        .wrapper_candidates
        .push(WrapperCandidate { name, forwards_to });
}

/// Returns the parameter identifiers in order, or `None` if any parameter is not a plain binding.
///
/// Destructuring, defaults, and rest parameters all break the positional correspondence the
/// wrapper rule depends on, so they disqualify the candidate rather than being guessed at.
fn plain_parameter_names(parameters: Node<'_>, source: &[u8]) -> Option<Vec<String>> {
    let mut cursor = parameters.walk();
    let mut names = Vec::new();
    for parameter in parameters.named_children(&mut cursor) {
        let identifier = match parameter.kind() {
            "identifier" => parameter,
            // `text: string` keeps a plain binding; only the type annotation is extra.
            "required_parameter" => parameter.child_by_field_name("pattern")?,
            _ => return None,
        };
        if identifier.kind() != "identifier" {
            return None;
        }
        names.push(node_text(identifier, source).to_owned());
    }
    Some(names)
}

/// Promotes wrappers whose forward target resolves to a known registration.
///
/// Wrappers form a forward graph, so resolving one can unlock the wrappers that call it. Walking
/// that graph from the already-known registrations visits each wrapper at most once, instead of
/// re-scanning every candidate until a fixpoint settles.
fn resolve_wrapper_candidates(discovered: &mut RegistrationDiscovery<'_>) {
    let mut callers: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, candidate) in discovered.wrapper_candidates.iter().enumerate() {
        callers
            .entry(candidate.forwards_to.as_str())
            .or_default()
            .push(index);
    }
    let mut pending: Vec<usize> = (0..discovered.wrapper_candidates.len()).collect();
    while let Some(index) = pending.pop() {
        let candidate = &discovered.wrapper_candidates[index];
        if discovered.aliases.contains_key(&candidate.name) {
            continue;
        }
        let Some(registration) = discovered.aliases.get(&candidate.forwards_to).cloned() else {
            continue;
        };
        let name = candidate.name.clone();
        discovered.aliases.insert(name.clone(), registration);
        // Only the wrappers that forward to the name just resolved can newly become resolvable.
        if let Some(unlocked) = callers.get(name.as_str()) {
            pending.extend(unlocked.iter().copied());
        }
    }
}

/// Returns the registration a wrapper forwards to, when the forward is positional and exact.
fn unwrap_parenthesized(mut node: Node<'_>) -> Node<'_> {
    while node.kind() == "parenthesized_expression" {
        let Some(inner) = node.named_child(0) else {
            break;
        };
        node = inner;
    }
    node
}

/// Returns the function a wrapper body forwards its leading parameters to, if it does exactly that.
///
/// A safely inferred wrapper has one directly executed statement. Descending into callbacks,
/// nested functions, branches, or loops would confuse code that merely contains a future or
/// conditional registration with code that registers a step when the wrapper is called.
///
/// Only an exact positional forward is accepted: argument *i* of the inner call must be parameter
/// *i* of the wrapper, for the matcher and handler positions. A wrapper that reorders, rewrites,
/// or synthesizes those arguments would make the extracted matcher and handler belong to
/// different steps.
fn forwarded_callee(
    body: Node<'_>,
    name: &str,
    parameters: &[String],
    source: &[u8],
) -> Option<String> {
    if body.named_child_count() != 1 {
        return None;
    }
    let statement = body.named_child(0)?;
    let expression = match statement.kind() {
        "expression_statement" | "return_statement" => statement.named_child(0)?,
        _ => return None,
    };
    let call = unwrap_parenthesized(expression);
    if call.kind() != "call_expression" {
        return None;
    }
    let function = call.child_by_field_name("function")?;
    let callee = match registration_callee(function, source)? {
        RegistrationCallee::Identifier(callee) => callee,
        RegistrationCallee::Property { .. } => return None,
    };
    // A wrapper that calls itself proves nothing and would otherwise resolve to itself.
    if callee == name {
        return None;
    }
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let arguments: Vec<_> = arguments.named_children(&mut cursor).collect();
    if arguments.len() < 2 {
        return None;
    }
    for (position, argument) in arguments.iter().take(2).enumerate() {
        if argument.kind() != "identifier"
            || Some(node_text(*argument, source)) != parameters.get(position).map(String::as_str)
        {
            return None;
        }
    }
    Some(callee.to_owned())
}

fn collect_unresolved_imports(
    import: Node<'_>,
    source: &[u8],
    module: &str,
    discovered: &mut RegistrationDiscovery<'_>,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_specifier" => {
                if is_type_only_specifier(node) {
                    continue;
                }
                let Some(name) = node.child_by_field_name("name") else {
                    continue;
                };
                let original = node_text(name, source);
                if REGISTRATIONS.contains(&original) {
                    let local = node
                        .child_by_field_name("alias")
                        .map_or(original, |alias| node_text(alias, source));
                    discovered
                        .unresolved_aliases
                        .insert(local.to_owned(), module.to_owned());
                }
            }
            "namespace_import" => {
                if let Some(identifier) = first_named_kind(node, "identifier") {
                    discovered
                        .unresolved_namespaces
                        .insert(node_text(identifier, source).to_owned(), module.to_owned());
                }
            }
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_imports(
    import: Node<'_>,
    source: &[u8],
    exports: &RegistrationExports,
    aliases: &mut RegistrationExports,
    namespaces: &mut BTreeMap<String, RegistrationExports>,
    create_bdd_factories: &mut BTreeSet<String>,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_specifier" => {
                if is_type_only_specifier(node) {
                    continue;
                }
                let Some(name) = node.child_by_field_name("name") else {
                    continue;
                };
                let original = node_text(name, source);
                if let Some(registration) = exports.get(original) {
                    let alias = node
                        .child_by_field_name("alias")
                        .map_or(original, |alias| node_text(alias, source));
                    if registration.kind == RegistrationExportKind::Factory {
                        create_bdd_factories.insert(alias.to_owned());
                    } else {
                        aliases.insert(alias.to_owned(), registration.clone());
                    }
                }
            }
            "namespace_import" => {
                if let Some(identifier) = first_named_kind(node, "identifier") {
                    namespaces.insert(node_text(identifier, source).to_owned(), exports.clone());
                }
            }
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_exports(
    export: Node<'_>,
    source: &[u8],
    available: &RegistrationExports,
    aliases: &mut RegistrationExports,
) {
    let mut stack = vec![export];
    while let Some(node) = stack.pop() {
        if node.kind() == "export_specifier" && !is_type_only_specifier(node) {
            let Some(name) = node.child_by_field_name("name") else {
                continue;
            };
            let original = node_text(name, source);
            if let Some(registration) = available.get(original) {
                let alias = node
                    .child_by_field_name("alias")
                    .map_or(original, |alias| node_text(alias, source));
                aliases.insert(alias.to_owned(), registration.clone());
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_shadowing_imports(
    import: Node<'_>,
    source: &[u8],
    shadowed_defaults: &mut BTreeSet<String>,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_specifier" && !is_type_only_specifier(node) {
            let local = node
                .child_by_field_name("alias")
                .or_else(|| node.child_by_field_name("name"));
            if let Some(local) = local {
                let local = node_text(local, source);
                if DEFAULT_REGISTRATIONS.contains(&local) {
                    shadowed_defaults.insert(local.to_owned());
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_variable_registration<'tree>(
    declaration: Node<'tree>,
    source: &[u8],
    discovered: &mut RegistrationDiscovery<'tree>,
) {
    let Some(name) = declaration.child_by_field_name("name") else {
        return;
    };
    if name.kind() == "object_pattern" {
        shadow_pattern_defaults(name, source, &mut discovered.shadowed_defaults);
    }
    let Some(value) = declaration.child_by_field_name("value") else {
        shadow_default_name(name, source, &mut discovered.shadowed_defaults);
        return;
    };

    if value.kind() == "call_expression" {
        let function = call_name(value, source);
        let supported_require = function == Some("require")
            && call_string_argument(value, source).is_some_and(is_supported_module);
        let create_bdd =
            function.is_some_and(|name| discovered.create_bdd_factories.contains(name));
        if supported_require || create_bdd {
            let exports = call_string_argument(value, source)
                .map(registration_exports_for_module)
                .unwrap_or_else(|| registration_exports_for_framework(Framework::PlaywrightBdd));
            match name.kind() {
                "object_pattern" => {
                    collect_pattern_aliases(name, source, &exports, &mut discovered.aliases)
                }
                "identifier" => {
                    discovered
                        .namespaces
                        .insert(node_text(name, source).to_owned(), exports);
                }
                _ => {}
            }
        }
    } else if name.kind() == "object_pattern" && value.kind() == "identifier" {
        discovered
            .namespace_destructures
            .push((name, node_text(value, source).to_owned()));
    } else if name.kind() == "identifier" && value.kind() == "identifier" {
        discovered.assignments.push((
            node_text(name, source).to_owned(),
            node_text(value, source).to_owned(),
        ));
    }

    if name.kind() == "identifier" {
        let local = node_text(name, source);
        if DEFAULT_REGISTRATIONS.contains(&local) {
            discovered.shadowed_defaults.insert(local.to_owned());
        }
    }
}

/// Collects only bindings with static module evidence for Playwright-BDD's factory.
///
/// The factory name is not treated as an ambient global: unrelated libraries and local helpers
/// commonly use generic factory names, and trusting one would misattribute every destructured
/// registration it returns. ESM aliases retain that evidence through resolved local barrels;
/// CommonJS destructuring requires the exact package specifier.
fn collect_create_bdd_factories(
    root: Node<'_>,
    source: &[u8],
    file_path: &Path,
    resolver: &mut RegistrationResolver,
) -> Result<BTreeSet<String>> {
    let mut factories = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_statement" if import_has_runtime_bindings(node) => {
                let exports = match import_module(node, source) {
                    Some(module) if is_supported_module(module) => {
                        Some(registration_exports_for_module(module))
                    }
                    Some(module) => resolver
                        .registration_exports(file_path, module)?
                        .resolution
                        .map(|resolution| resolution.exports),
                    None => None,
                };
                if let Some(exports) = exports {
                    collect_factory_import_aliases(node, source, &exports, &mut factories);
                }
            }
            "variable_declarator" if is_top_level_variable(node) => {
                let (Some(pattern), Some(value)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("value"),
                ) else {
                    push_named_children_reverse(node, &mut stack);
                    continue;
                };
                if pattern.kind() == "object_pattern"
                    && value.kind() == "call_expression"
                    && call_name(value, source) == Some("require")
                    && call_string_argument(value, source) == Some(super::ast::PLAYWRIGHT_MODULE)
                {
                    collect_named_binding_aliases(
                        pattern,
                        source,
                        "pair_pattern",
                        "createBdd",
                        &mut factories,
                    );
                    let mut cursor = pattern.walk();
                    for binding in pattern.named_children(&mut cursor) {
                        if binding.kind() == "shorthand_property_identifier_pattern"
                            && node_text(binding, source) == "createBdd"
                        {
                            factories.insert("createBdd".to_owned());
                        }
                    }
                }
            }
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }
    Ok(factories)
}

fn collect_factory_import_aliases(
    import: Node<'_>,
    source: &[u8],
    exports: &RegistrationExports,
    factories: &mut BTreeSet<String>,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_specifier" && !is_type_only_specifier(node) {
            if let Some(name) = node.child_by_field_name("name") {
                let imported = node_text(name, source);
                if exports
                    .get(imported)
                    .is_some_and(|export| export.kind == RegistrationExportKind::Factory)
                {
                    let local = node
                        .child_by_field_name("alias")
                        .map_or(imported, |alias| node_text(alias, source));
                    factories.insert(local.to_owned());
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
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

fn collect_named_binding_aliases(
    root: Node<'_>,
    source: &[u8],
    binding_kind: &str,
    expected: &str,
    aliases: &mut BTreeSet<String>,
) {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == binding_kind && !is_type_only_specifier(node) {
            if let Some(name) = node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("key"))
            {
                if node_text(name, source) == expected {
                    let alias = node
                        .child_by_field_name("alias")
                        .or_else(|| node.child_by_field_name("value"))
                        .map_or(expected, |alias| node_text(alias, source));
                    if alias.chars().all(is_identifier_character) {
                        aliases.insert(alias.to_owned());
                    }
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_assignment(
    assignment: Node<'_>,
    source: &[u8],
    discovered: &mut RegistrationDiscovery<'_>,
) {
    let (Some(left), Some(right)) = (
        assignment.child_by_field_name("left"),
        assignment.child_by_field_name("right"),
    ) else {
        return;
    };
    if left.kind() != "identifier" {
        return;
    }
    let local = node_text(left, source);
    if DEFAULT_REGISTRATIONS.contains(&local) {
        discovered.shadowed_defaults.insert(local.to_owned());
    }
    if right.kind() == "identifier" {
        discovered
            .assignments
            .push((local.to_owned(), node_text(right, source).to_owned()));
    }
}

fn shadow_named_declaration(
    declaration: Node<'_>,
    source: &[u8],
    shadowed_defaults: &mut BTreeSet<String>,
) {
    if let Some(name) = declaration.child_by_field_name("name") {
        shadow_default_name(name, source, shadowed_defaults);
    }
}

fn shadow_default_name(name: Node<'_>, source: &[u8], shadowed_defaults: &mut BTreeSet<String>) {
    if matches!(
        name.kind(),
        "identifier" | "type_identifier" | "shorthand_property_identifier_pattern"
    ) {
        let name = node_text(name, source);
        if DEFAULT_REGISTRATIONS.contains(&name) {
            shadowed_defaults.insert(name.to_owned());
        }
    }
}

fn shadow_pattern_defaults(
    pattern: Node<'_>,
    source: &[u8],
    shadowed_defaults: &mut BTreeSet<String>,
) {
    let mut stack = vec![pattern];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "identifier" | "shorthand_property_identifier_pattern" => {
                shadow_default_name(node, source, shadowed_defaults);
            }
            // The key names an object property; only the value introduces a local binding.
            "pair_pattern" => {
                if let Some(value) = node.child_by_field_name("value") {
                    stack.push(value);
                }
            }
            "assignment_pattern" | "object_assignment_pattern" => {
                if let Some(left) = node
                    .child_by_field_name("left")
                    .or_else(|| node.named_child(0))
                {
                    stack.push(left);
                }
            }
            _ => push_named_children_reverse(node, &mut stack),
        }
    }
}

fn call_name<'a>(call: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let function = call.child_by_field_name("function")?;
    (function.kind() == "identifier").then(|| node_text(function, source))
}

fn call_string_argument<'a>(call: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let argument = arguments.named_children(&mut cursor).next()?;
    string_literal(argument, source)
}

fn first_named_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == kind);
    found
}

fn collect_pattern_aliases(
    pattern: Node<'_>,
    source: &[u8],
    exports: &RegistrationExports,
    aliases: &mut RegistrationExports,
) {
    let mut cursor = pattern.walk();
    for child in pattern.named_children(&mut cursor) {
        let (original, alias) = match child.kind() {
            "shorthand_property_identifier_pattern" => {
                let name = node_text(child, source);
                (name, name)
            }
            "pair_pattern" => {
                let (Some(key), Some(value)) = (
                    child.child_by_field_name("key"),
                    child.child_by_field_name("value"),
                ) else {
                    continue;
                };
                (node_text(key, source), node_text(value, source))
            }
            _ => continue,
        };
        if let Some(registration) = exports.get(original) {
            if alias.chars().all(is_identifier_character) {
                aliases.insert(alias.to_owned(), registration.clone());
            }
        }
    }
}

fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '$')
}
