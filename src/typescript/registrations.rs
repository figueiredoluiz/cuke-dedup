use super::ast::{
    default_registration_exports, import_module, is_supported_module, push_named_children_reverse,
    string_literal, CUCUMBER_MODULE, CYPRESS_MODULE, DEFAULT_REGISTRATIONS, PLAYWRIGHT_MODULE,
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
    aliases: BTreeMap<String, String>,
    namespaces: BTreeMap<String, BTreeMap<String, String>>,
    unresolved_aliases: BTreeMap<String, String>,
    unresolved_namespaces: BTreeMap<String, String>,
    /// Why a specifier could not be resolved statically, keyed by module specifier.
    unresolved_reasons: BTreeMap<String, UnresolvedModuleReason>,
    pub(super) framework: Framework,
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
    aliases: BTreeMap<String, String>,
    namespaces: BTreeMap<String, BTreeMap<String, String>>,
    assignments: Vec<(String, String)>,
    namespace_destructures: Vec<(Node<'tree>, String)>,
    shadowed_defaults: BTreeSet<String>,
    unresolved_aliases: BTreeMap<String, String>,
    unresolved_namespaces: BTreeMap<String, String>,
    unresolved_reasons: BTreeMap<String, UnresolvedModuleReason>,
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
) -> Option<(String, String)> {
    match registration_callee(function, source)? {
        RegistrationCallee::Identifier(name) => registrations
            .aliases
            .get(name)
            .cloned()
            .map(|registration| (node_text(function, source).to_owned(), registration)),
        RegistrationCallee::Property { object, name } => {
            let exports = registrations.namespaces.get(node_text(object, source))?;
            exports
                .get(name.as_ref())
                .cloned()
                .map(|registration| (node_text(function, source).to_owned(), registration))
        }
    }
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

fn unwrap_registration_callee(mut function: Node<'_>) -> Option<Node<'_>> {
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
    let mut cucumber = false;
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "import_statement" | "export_statement") {
            if let Some(module) = import_module(node, source) {
                match module {
                    PLAYWRIGHT_MODULE => return Framework::PlaywrightBdd,
                    CYPRESS_MODULE => return Framework::CypressCucumber,
                    CUCUMBER_MODULE => cucumber = true,
                    _ => {}
                }
            }
        } else if node.kind() == "call_expression" {
            if call_name(node, source) == Some("createBdd") {
                return Framework::PlaywrightBdd;
            }
            if call_name(node, source) == Some("require") {
                match call_string_argument(node, source) {
                    Some(PLAYWRIGHT_MODULE) => return Framework::PlaywrightBdd,
                    Some(CYPRESS_MODULE) => return Framework::CypressCucumber,
                    Some(CUCUMBER_MODULE) => cucumber = true,
                    _ => {}
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
    if cucumber {
        Framework::CucumberJs
    } else {
        Framework::Unknown
    }
}

pub(super) fn detect_registrations(
    root: Node<'_>,
    source: &[u8],
    framework: Framework,
    file_path: &Path,
    resolver: &mut RegistrationResolver,
    configured: &BTreeSet<String>,
) -> Result<RegistrationNames> {
    let mut discovered = RegistrationDiscovery::default();
    let mut effective_framework = framework;
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_statement" => {
                let module = import_module(node, source);
                let exports = match module {
                    Some(module) if is_supported_module(module) => {
                        Some(default_registration_exports())
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
                            effective_framework =
                                merge_framework(effective_framework, resolution.framework);
                            resolution.exports
                        })
                    }
                    _ => None,
                };
                if !is_type_only_import(node, source)
                    && exports.as_ref().is_some_and(|exports| !exports.is_empty())
                {
                    collect_imports(
                        node,
                        source,
                        &exports.unwrap_or_default(),
                        &mut discovered.aliases,
                        &mut discovered.namespaces,
                    );
                } else {
                    if !is_type_only_import(node, source) && exports.is_none() {
                        if let Some(module) = module {
                            collect_unresolved_imports(node, source, module, &mut discovered);
                        }
                    }
                    collect_shadowing_imports(node, source, &mut discovered.shadowed_defaults);
                }
            }
            "export_statement" if import_module(node, source).is_some_and(is_supported_module) => {
                collect_exports(node, source, &mut discovered.aliases);
            }
            "variable_declarator" => {
                collect_variable_registration(node, source, effective_framework, &mut discovered)
            }
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
            discovered.aliases.insert(name.to_owned(), name.to_owned());
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
            .or_insert_with(|| name.clone());
    }
    resolve_wrapper_candidates(&mut discovered);

    Ok(RegistrationNames {
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
                if node_text(node, source).trim_start().starts_with("type ") {
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
    exports: &BTreeMap<String, String>,
    aliases: &mut BTreeMap<String, String>,
    namespaces: &mut BTreeMap<String, BTreeMap<String, String>>,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_specifier" => {
                if node_text(node, source).trim_start().starts_with("type ") {
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
                    aliases.insert(alias.to_owned(), registration.clone());
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

fn collect_exports(export: Node<'_>, source: &[u8], aliases: &mut BTreeMap<String, String>) {
    let mut stack = vec![export];
    while let Some(node) = stack.pop() {
        if node.kind() == "export_specifier" {
            let Some(name) = node.child_by_field_name("name") else {
                continue;
            };
            let original = node_text(name, source);
            if REGISTRATIONS.contains(&original) {
                let alias = node
                    .child_by_field_name("alias")
                    .map_or(original, |alias| node_text(alias, source));
                aliases.insert(alias.to_owned(), original.to_owned());
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
        if node.kind() == "import_specifier" {
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
    framework: Framework,
    discovered: &mut RegistrationDiscovery<'tree>,
) {
    let Some(name) = declaration.child_by_field_name("name") else {
        return;
    };
    let Some(value) = declaration.child_by_field_name("value") else {
        shadow_default_name(name, source, &mut discovered.shadowed_defaults);
        return;
    };

    if value.kind() == "call_expression" {
        let function = call_name(value, source);
        let supported_require = function == Some("require")
            && call_string_argument(value, source).is_some_and(is_supported_module);
        let create_bdd = framework == Framework::PlaywrightBdd && function == Some("createBdd");
        if supported_require || create_bdd {
            let exports = default_registration_exports();
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
    if name.kind() == "identifier" || name.kind() == "type_identifier" {
        let name = node_text(name, source);
        if DEFAULT_REGISTRATIONS.contains(&name) {
            shadowed_defaults.insert(name.to_owned());
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
    exports: &BTreeMap<String, String>,
    aliases: &mut BTreeMap<String, String>,
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

fn is_type_only_import(import: Node<'_>, source: &[u8]) -> bool {
    let Some(after_import) = node_text(import, source)
        .trim_start()
        .strip_prefix("import")
    else {
        return false;
    };
    let after_import = after_import.trim_start();
    after_import.strip_prefix("type").is_some_and(|rest| {
        rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('{')
    })
}
