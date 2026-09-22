use super::ast::{
    call_string_argument, import_has_runtime_bindings, import_module, is_star_export,
    is_top_level_variable, is_type_only_declaration, is_type_only_specifier,
    push_named_children_reverse, string_literal,
};
use super::frameworks::{
    framework_for_module, is_supported_module, registration_exports_for_framework,
    registration_exports_for_module, RegistrationExports,
};
use super::node_text;
use super::project_resolution::ProjectResolution;
use crate::model::Framework;
use crate::resource_limits::{
    read_utf8, MAX_PROJECT_INPUT_BYTES, MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES,
    MAX_REGISTRATION_RESOLUTION_STATES,
};
use crate::source_adapter::{grammar_for_language, language_for_path};
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

const MAX_REEXPORT_DEPTH: usize = 16;
type CachedExports = (Option<RegistrationExports>, Framework, bool);

#[derive(Debug, Clone)]
struct Reexport {
    module: String,
    specifiers: Vec<(String, String)>,
    star: bool,
}

#[derive(Debug, Clone)]
struct ModuleFacts {
    reexports: Vec<Reexport>,
    direct_exports: RegistrationExports,
    framework: Framework,
    /// A CommonJS export assignment used a form the analyzer does not model, so the resolved
    /// exports may be a subset of what the module registers.
    exports_incomplete: bool,
}

#[derive(Default)]
struct ResolutionState {
    active: BTreeSet<PathBuf>,
    exports: BTreeMap<(PathBuf, usize), CachedExports>,
    modules: BTreeMap<PathBuf, Option<ModuleFacts>>,
    module_bytes: usize,
    resolutions_started: usize,
}

#[derive(Debug)]
struct ResolvedExports {
    exports: Option<RegistrationExports>,
    framework: Framework,
    cacheable: bool,
    /// A resolved module (or one it re-exports) declared exports in a form the analyzer does not
    /// model, so `exports` may be a subset of the real registration set.
    exports_incomplete: bool,
}

#[derive(Debug)]
pub(super) struct RegistrationResolution {
    pub(super) module_path: Option<PathBuf>,
    pub(super) exports: RegistrationExports,
    pub(super) framework: Framework,
}

/// Outcome of resolving one import specifier to its registration exports.
///
/// A specifier that cannot be resolved statically is a recall limitation, not an analyzer
/// failure: the importing file simply contributes no registration aliases. `reason` carries the
/// static-resolution failure so the caller can explain *why* in a source-localized diagnostic
/// rather than aborting the run.
#[derive(Debug, Default)]
pub(super) struct RegistrationOutcome {
    pub(super) resolution: Option<RegistrationResolution>,
    pub(super) reason: Option<String>,
}

#[derive(Default)]
pub(super) struct RegistrationResolver {
    boundary: Option<PathBuf>,
    project: ProjectResolution,
    state: ResolutionState,
}

impl RegistrationResolver {
    pub(super) fn for_root(root: &Path) -> Self {
        Self {
            boundary: root.canonicalize().ok(),
            project: ProjectResolution::for_root(root),
            state: ResolutionState::default(),
        }
    }

    pub(super) fn registration_exports(
        &mut self,
        importer: &Path,
        specifier: &str,
    ) -> Result<RegistrationOutcome> {
        let boundary = self.boundary.clone().or_else(|| project_boundary(importer));
        let Some(boundary) = boundary else {
            return Ok(RegistrationOutcome::default());
        };
        match resolve_exports(
            importer,
            specifier,
            &boundary,
            &mut self.project,
            &mut self.state,
            0,
        ) {
            Ok(resolved) => {
                // A module that resolved but declared some exports in an unmodeled CommonJS form
                // may register more than the analyzer could see. Report it so the corpus is marked
                // incomplete, while still using the exports that did resolve.
                let reason = resolved.exports_incomplete.then(|| {
                    "some exports use a CommonJS form the analyzer does not model".to_owned()
                });
                Ok(RegistrationOutcome {
                    resolution: resolved.exports.map(|exports| RegistrationResolution {
                        module_path: self
                            .project
                            .resolve(importer, specifier, &boundary)
                            .ok()
                            .flatten(),
                        exports,
                        framework: resolved.framework,
                    }),
                    reason,
                })
            }
            // A module the filesystem refused to hand over is an operational failure, exactly as
            // it is for a definition source: the analyzer could not read input it was told to
            // read, and silently continuing would understate the corpus. Everything else here
            // means "this specifier is not statically resolvable" — outside the analysis root, a
            // malformed or cyclic project config, or a resolution resource limit — which bounds
            // recall without making any surviving finding wrong, so it degrades to a warning.
            Err(error) if is_io_failure(&error) => Err(error),
            Err(error) => Ok(RegistrationOutcome {
                resolution: None,
                reason: Some(format!("{error:#}")),
            }),
        }
    }
}

fn resolve_exports(
    importer: &Path,
    specifier: &str,
    boundary: &Path,
    project: &mut ProjectResolution,
    state: &mut ResolutionState,
    depth: usize,
) -> Result<ResolvedExports> {
    if depth >= MAX_REEXPORT_DEPTH {
        return Ok(ResolvedExports {
            exports: None,
            framework: Framework::Unknown,
            cacheable: true,
            exports_incomplete: false,
        });
    }
    let Some(path) = project.resolve(importer, specifier, boundary)? else {
        return Ok(ResolvedExports {
            exports: None,
            framework: Framework::Unknown,
            cacheable: true,
            exports_incomplete: false,
        });
    };
    let remaining_depth = MAX_REEXPORT_DEPTH - depth;
    let cache_key = (path.clone(), remaining_depth);
    if let Some((exports, framework, exports_incomplete)) = state.exports.get(&cache_key) {
        return Ok(ResolvedExports {
            exports: exports.clone(),
            framework: *framework,
            cacheable: true,
            exports_incomplete: *exports_incomplete,
        });
    }
    if !state.active.insert(path.clone()) {
        return Ok(ResolvedExports {
            exports: None,
            framework: Framework::Unknown,
            cacheable: false,
            exports_incomplete: false,
        });
    }
    let resolved = resolve_active_exports(&path, boundary, project, state, depth);
    // Resolution failures are converted to source-local diagnostics by the caller. Always clear
    // the active-path marker before propagating one, or a later independent import is mistaken
    // for a cycle and silently loses registrations.
    state.active.remove(&path);
    let resolved = resolved?;
    if resolved.cacheable {
        state.exports.insert(
            cache_key,
            (
                resolved.exports.clone(),
                resolved.framework,
                resolved.exports_incomplete,
            ),
        );
    }
    Ok(resolved)
}

fn resolve_active_exports(
    path: &Path,
    boundary: &Path,
    project: &mut ProjectResolution,
    state: &mut ResolutionState,
    depth: usize,
) -> Result<ResolvedExports> {
    if state.resolutions_started >= MAX_REGISTRATION_RESOLUTION_STATES {
        bail!(
            "registration module graph exceeds the {}-state resolution limit",
            MAX_REGISTRATION_RESOLUTION_STATES
        );
    }
    state.resolutions_started += 1;

    let Some(module) = load_module(path, state)? else {
        return Ok(ResolvedExports {
            exports: None,
            framework: Framework::Unknown,
            cacheable: true,
            exports_incomplete: false,
        });
    };
    let mut exports = module.direct_exports;
    let mut framework = module.framework;
    let mut cacheable = true;
    let mut exports_incomplete = module.exports_incomplete;
    for reexport in module.reexports {
        let (available, available_framework) = if is_supported_module(&reexport.module) {
            (
                registration_exports_for_module(&reexport.module),
                framework_for_module(&reexport.module),
            )
        } else {
            let resolved =
                resolve_exports(path, &reexport.module, boundary, project, state, depth + 1)?;
            cacheable &= resolved.cacheable;
            exports_incomplete |= resolved.exports_incomplete;
            (resolved.exports.unwrap_or_default(), resolved.framework)
        };
        framework = merge_framework(framework, available_framework);
        for (imported, exported) in &reexport.specifiers {
            if let Some(canonical) = available.get(imported) {
                exports.insert(exported.clone(), canonical.clone());
            }
        }
        if reexport.star {
            exports.extend(available);
        }
    }
    Ok(ResolvedExports {
        exports: Some(exports),
        framework,
        cacheable,
        exports_incomplete,
    })
}

/// Returns whether `error` was caused by the filesystem refusing a read.
///
/// Resource limits carry their own typed cause, so an `io::Error` anywhere in the chain means a
/// genuine read failure — a permission change, a vanished file, or exhausted descriptors.
fn is_io_failure(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
}

pub(super) fn merge_framework(current: Framework, evidence: Framework) -> Framework {
    match (current, evidence) {
        (Framework::PlaywrightBdd, _) | (_, Framework::PlaywrightBdd) => Framework::PlaywrightBdd,
        (Framework::CypressCucumber, _) | (_, Framework::CypressCucumber) => {
            Framework::CypressCucumber
        }
        (Framework::CucumberJs, _) | (_, Framework::CucumberJs) => Framework::CucumberJs,
        _ => Framework::Unknown,
    }
}

fn load_module(path: &Path, state: &mut ResolutionState) -> Result<Option<ModuleFacts>> {
    if let Some(module) = state.modules.get(path) {
        return Ok(module.clone());
    }
    if state.modules.len() >= MAX_REGISTRATION_MODULES {
        bail!(
            "registration module graph exceeds the {}-module resolution limit",
            MAX_REGISTRATION_MODULES
        );
    }
    let source = read_utf8(path, "registration module", MAX_PROJECT_INPUT_BYTES)?;
    let module_bytes = state.module_bytes.saturating_add(source.len());
    if module_bytes > MAX_REGISTRATION_MODULE_BYTES {
        bail!(
            "registration module graph exceeds the {}-byte aggregate resolution limit",
            MAX_REGISTRATION_MODULE_BYTES
        );
    }
    state.module_bytes = module_bytes;
    let module = if let Some(tree) = parse_module(&source, path) {
        if tree.root_node().has_error() {
            bail!(
                "registration module {} contains JavaScript/TypeScript syntax errors; refusing recovered exports",
                path.display()
            );
        }
        Some(collect_module_facts(&tree, &source))
    } else {
        None
    };
    state.modules.insert(path.to_owned(), module.clone());
    Ok(module)
}

fn collect_module_facts(tree: &tree_sitter::Tree, source: &str) -> ModuleFacts {
    let source = source.as_bytes();
    let mut create_bdd_aliases = BTreeSet::new();
    let mut local_registrations = RegistrationExports::new();
    let mut imported_bindings = BTreeMap::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_statement" if import_has_runtime_bindings(node) => {
                if let Some(module) = import_module(node, source) {
                    collect_imported_binding_origins(node, source, module, &mut imported_bindings);
                    if is_supported_module(module) {
                        let available = registration_exports_for_module(module);
                        collect_imported_registrations(
                            node,
                            source,
                            &available,
                            &mut local_registrations,
                        );
                    }
                    collect_factory_aliases(
                        node,
                        source,
                        &registration_exports_for_module(module),
                        &mut create_bdd_aliases,
                    );
                }
            }
            "variable_declarator" if is_top_level_variable(node) => {
                collect_commonjs_imports(
                    node,
                    source,
                    &mut create_bdd_aliases,
                    &mut local_registrations,
                );
            }
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }

    let mut direct_export_names = Vec::new();
    let mut reexports = Vec::new();
    // Set when a CommonJS `module.exports`/`exports.x` assignment uses a form the analyzer cannot
    // turn into an export, so the file resolves to *fewer* registrations than it really has. The
    // caller reports the file incomplete rather than treating the partial result as authoritative.
    let mut exports_incomplete = false;
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "variable_declarator" if is_top_level_variable(node) => {
                collect_create_bdd_bindings(
                    node,
                    source,
                    &create_bdd_aliases,
                    &mut local_registrations,
                    &mut direct_export_names,
                );
            }
            "export_statement" if !is_type_only_declaration(node) => {
                if let Some(module) = import_module(node, source) {
                    let specifiers = export_specifiers(node, source);
                    let star = is_star_export(node);
                    if star || !specifiers.is_empty() {
                        reexports.push(Reexport {
                            module: module.to_owned(),
                            specifiers,
                            star,
                        });
                    }
                } else {
                    direct_export_names.extend(export_specifiers(node, source));
                }
            }
            "assignment_expression" if is_module_scoped_expression(node) => {
                collect_commonjs_export_assignment(
                    node,
                    source,
                    &mut direct_export_names,
                    &mut reexports,
                    &mut exports_incomplete,
                );
            }
            "call_expression" if is_module_scoped_expression(node) => {
                collect_object_assign_exports(
                    node,
                    source,
                    &mut reexports,
                    &mut exports_incomplete,
                );
            }
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }

    let mut direct_exports = RegistrationExports::new();
    for (local, exported) in direct_export_names {
        if let Some(registration) = local_registrations.get(&local).cloned() {
            direct_exports.insert(exported, registration);
        } else if let Some((module, imported)) = imported_bindings.get(&local) {
            // `import { Given as Setup } from './a'; export { Setup as Given }` is semantically
            // a re-export even though its syntax is split across two statements. Keeping the
            // module edge preserves provenance through arbitrarily chained local barrels.
            reexports.push(Reexport {
                module: module.clone(),
                specifiers: vec![(imported.clone(), exported)],
                star: false,
            });
        }
    }
    let framework = direct_exports
        .values()
        .fold(Framework::Unknown, |current, export| {
            merge_framework(current, export.framework)
        });
    ModuleFacts {
        reexports,
        direct_exports,
        framework,
        exports_incomplete,
    }
}

/// Records a CommonJS `module.exports = …` or `exports.<name> = …` assignment as exports.
///
/// Only forms that can hide a registration set incompleteness — a factory call, an aliased object,
/// an unresolvable spread. A plain value export (`exports.helper = () => {}`) is not a registration
/// container and is left silent, so an ordinary CommonJS module does not warn.
fn collect_commonjs_export_assignment(
    node: Node<'_>,
    source: &[u8],
    direct_export_names: &mut Vec<(String, String)>,
    reexports: &mut Vec<Reexport>,
    incomplete: &mut bool,
) {
    // A well-formed assignment always has both sides; a partial one from error recovery matches
    // nothing below, so no separate guard is needed.
    let target = node
        .child_by_field_name("left")
        .and_then(|left| commonjs_export_target(left, source));
    let right = node.child_by_field_name("right");
    match (target, right) {
        (Some(CommonjsExportTarget::All), Some(right)) => {
            collect_commonjs_object_exports(
                right,
                source,
                direct_export_names,
                reexports,
                incomplete,
            );
        }
        (Some(CommonjsExportTarget::Named(name)), Some(right)) => {
            if let Some((module, imported)) = required_member(right, source) {
                // `exports.step = require('./a').step`
                reexports.push(Reexport {
                    module: module.to_owned(),
                    specifiers: vec![(imported.to_owned(), name.to_owned())],
                    star: false,
                });
            } else if require_specifier(right, source).is_some() {
                // `exports.steps = require('./a')` assigns the whole module object to one name; the
                // analyzer does not model namespace-member registration, so it may hide steps.
                *incomplete = true;
            } else if right.kind() == "identifier" {
                // `exports.Given = Given` re-exports a local binding; resolves against
                // `local_registrations`, contributing nothing when the local is not a registration.
                direct_export_names.push((node_text(right, source).to_owned(), name.to_owned()));
            } else if matches!(
                right.kind(),
                "call_expression" | "member_expression" | "subscript_expression"
            ) {
                // `exports.Given = makeGiven()` or `= cucumber.Given` can carry a registration the
                // analyzer cannot resolve, so it fails closed rather than dropping it silently. A
                // literal or other inert value exports nothing and stays quiet.
                *incomplete = true;
            }
        }
        _ => {}
    }
}

/// Records the object or call on the right of `module.exports = …`.
fn collect_commonjs_object_exports(
    right: Node<'_>,
    source: &[u8],
    direct_export_names: &mut Vec<(String, String)>,
    reexports: &mut Vec<Reexport>,
    incomplete: &mut bool,
) {
    if let Some(module) = require_specifier(right, source) {
        // `module.exports = require('./a')`
        reexports.push(Reexport {
            module: module.to_owned(),
            specifiers: Vec::new(),
            star: true,
        });
        return;
    }
    if right.kind() != "object" {
        // A factory call, an aliased identifier, or any other whole-object form the analyzer
        // cannot introspect can hide registrations.
        *incomplete = true;
        return;
    }
    let mut cursor = right.walk();
    for property in right.named_children(&mut cursor) {
        match property.kind() {
            "shorthand_property_identifier" => {
                let name = node_text(property, source);
                direct_export_names.push((name.to_owned(), name.to_owned()));
            }
            "pair" => {
                let exported = property
                    .child_by_field_name("key")
                    .and_then(|key| property_name(key, source));
                let value = property.child_by_field_name("value");
                if let (Some(exported), Some(value)) = (exported, value) {
                    if value.kind() == "identifier" {
                        direct_export_names
                            .push((node_text(value, source).to_owned(), exported.to_owned()));
                    } else {
                        // `Given: makeGiven()` and other non-identifier values are containers the
                        // analyzer cannot introspect.
                        *incomplete = true;
                    }
                } else {
                    // A computed key, or a malformed pair, is a form the analyzer cannot model.
                    *incomplete = true;
                }
            }
            "spread_element" => {
                if let Some(module) = property
                    .named_child(0)
                    .and_then(|argument| require_specifier(argument, source))
                {
                    reexports.push(Reexport {
                        module: module.to_owned(),
                        specifiers: Vec::new(),
                        star: true,
                    });
                } else {
                    *incomplete = true;
                }
            }
            // A method definition or computed key can carry a registration the analyzer cannot see.
            _ => *incomplete = true,
        }
    }
}

/// Records `Object.assign(module.exports, require('./a'), { … })` as re-exports.
fn collect_object_assign_exports(
    node: Node<'_>,
    source: &[u8],
    reexports: &mut Vec<Reexport>,
    incomplete: &mut bool,
) {
    // An `Object.assign` call always carries an argument list; the absent case matches nothing.
    let arguments = (call_member(node, source) == Some(("Object", "assign")))
        .then(|| node.child_by_field_name("arguments"))
        .flatten();
    let Some(arguments) = arguments else {
        return;
    };
    let mut cursor = arguments.walk();
    let mut arguments = arguments.named_children(&mut cursor);
    // The first argument is the assignment target; it must be `module.exports` for this to be an
    // export merge rather than an unrelated `Object.assign`.
    let is_export_merge = arguments.next().is_some_and(|target| {
        matches!(
            commonjs_export_target(target, source),
            Some(CommonjsExportTarget::All)
        )
    });
    if !is_export_merge {
        return;
    }
    for argument in arguments {
        if let Some(module) = require_specifier(argument, source) {
            reexports.push(Reexport {
                module: module.to_owned(),
                specifiers: Vec::new(),
                star: true,
            });
        } else {
            *incomplete = true;
        }
    }
}

enum CommonjsExportTarget<'a> {
    All,
    Named(&'a str),
}

/// Whether an assignment or call is evaluated at module load — a statement of the program body —
/// rather than inside a function or class that may never run. Only such an operation is a real
/// CommonJS export; the same syntax nested in a helper assigns to whatever `module`/`exports` names
/// it can see, not the module record.
fn is_module_scoped_expression(node: Node<'_>) -> bool {
    node.parent().is_some_and(|statement| {
        statement.kind() == "expression_statement"
            && statement
                .parent()
                .is_some_and(|parent| parent.kind() == "program")
    })
}

/// Recognizes `module.exports` and `module.exports.<name>` (→ `All`/`Named`) and `exports.<name>`
/// (→ `Named`) on an assignment's left.
fn commonjs_export_target<'a>(
    node: Node<'_>,
    source: &'a [u8],
) -> Option<CommonjsExportTarget<'a>> {
    if node.kind() != "member_expression" {
        return None;
    }
    let object = node.child_by_field_name("object")?;
    let name = node
        .child_by_field_name("property")
        .filter(|property| property.kind() == "property_identifier")
        .map(|property| node_text(property, source))?;
    match object.kind() {
        // `module.exports = …` / `exports.name = …`
        "identifier" => match (node_text(object, source), name) {
            ("module", "exports") => Some(CommonjsExportTarget::All),
            ("exports", _) => Some(CommonjsExportTarget::Named(name)),
            _ => None,
        },
        // `module.exports.name = …` names a single export off the module object.
        "member_expression" if is_module_exports(object, source) => {
            Some(CommonjsExportTarget::Named(name))
        }
        _ => None,
    }
}

/// Whether `node` is exactly the member expression `module.exports`.
fn is_module_exports(node: Node<'_>, source: &[u8]) -> bool {
    node.kind() == "member_expression"
        && node.child_by_field_name("object").is_some_and(|object| {
            object.kind() == "identifier" && node_text(object, source) == "module"
        })
        && node
            .child_by_field_name("property")
            .is_some_and(|property| node_text(property, source) == "exports")
}

/// Returns the specifier of a `require('…')` call, unwrapping transparent wrappers.
fn require_specifier<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    (call_identifier(node, source) == Some("require"))
        .then(|| call_string_argument(node, source))
        .flatten()
}

/// Returns `(module, member)` for `require('…').member`.
fn required_member<'a>(node: Node<'_>, source: &'a [u8]) -> Option<(&'a str, &'a str)> {
    if node.kind() != "member_expression" {
        return None;
    }
    let module = require_specifier(node.child_by_field_name("object")?, source)?;
    let property = node.child_by_field_name("property")?;
    (property.kind() == "property_identifier").then(|| (module, node_text(property, source)))
}

/// Returns the `(object, property)` identifiers of a plain `object.property(...)` call.
fn call_member<'a>(call: Node<'_>, source: &'a [u8]) -> Option<(&'a str, &'a str)> {
    let function = call.child_by_field_name("function")?;
    if function.kind() != "member_expression" {
        return None;
    }
    let object = function.child_by_field_name("object")?;
    let property = function.child_by_field_name("property")?;
    (object.kind() == "identifier" && property.kind() == "property_identifier")
        .then(|| (node_text(object, source), node_text(property, source)))
}

/// Returns the static name of an object-literal property key.
fn property_name<'a>(key: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    match key.kind() {
        "property_identifier" => Some(node_text(key, source)),
        "string" => string_literal(key, source),
        _ => None,
    }
}

fn collect_imported_registrations(
    import: Node<'_>,
    source: &[u8],
    available: &RegistrationExports,
    local_registrations: &mut RegistrationExports,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_specifier" && !is_type_only_specifier(node) {
            if let Some(name) = node.child_by_field_name("name") {
                let imported = node_text(name, source);
                if let Some(registration) = available.get(imported) {
                    let local = node
                        .child_by_field_name("alias")
                        .map_or(imported, |alias| node_text(alias, source));
                    local_registrations.insert(local.to_owned(), registration.clone());
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_imported_binding_origins(
    import: Node<'_>,
    source: &[u8],
    module: &str,
    origins: &mut BTreeMap<String, (String, String)>,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_specifier" && !is_type_only_specifier(node) {
            if let Some(name) = node.child_by_field_name("name") {
                let imported = node_text(name, source);
                let local = node
                    .child_by_field_name("alias")
                    .map_or(imported, |alias| node_text(alias, source));
                origins.insert(local.to_owned(), (module.to_owned(), imported.to_owned()));
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_factory_aliases(
    import: Node<'_>,
    source: &[u8],
    available: &RegistrationExports,
    aliases: &mut BTreeSet<String>,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_specifier" && !is_type_only_specifier(node) {
            if let Some(name) = node.child_by_field_name("name") {
                let imported = node_text(name, source);
                if available.get(imported).is_some_and(|export| {
                    export.kind == super::frameworks::RegistrationExportKind::Factory
                }) {
                    let local = node
                        .child_by_field_name("alias")
                        .map_or(imported, |alias| node_text(alias, source));
                    aliases.insert(local.to_owned());
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_commonjs_imports(
    declarator: Node<'_>,
    source: &[u8],
    create_bdd_aliases: &mut BTreeSet<String>,
    local_registrations: &mut RegistrationExports,
) {
    let (Some(pattern), Some(value)) = (
        declarator.child_by_field_name("name"),
        declarator.child_by_field_name("value"),
    ) else {
        return;
    };
    if pattern.kind() != "object_pattern" || call_identifier(value, source) != Some("require") {
        return;
    }
    let Some(module) = call_string_argument(value, source) else {
        return;
    };
    if is_supported_module(module) {
        collect_pattern_registrations(
            pattern,
            source,
            &registration_exports_for_module(module),
            local_registrations,
        );
    }
    if module == super::frameworks::PLAYWRIGHT_MODULE {
        collect_pattern_alias(pattern, source, "createBdd", create_bdd_aliases);
    }
}

fn collect_pattern_registrations(
    pattern: Node<'_>,
    source: &[u8],
    available: &RegistrationExports,
    local_registrations: &mut RegistrationExports,
) {
    let mut cursor = pattern.walk();
    for binding in pattern.named_children(&mut cursor) {
        let (imported, local) = pattern_binding(binding, source);
        if let (Some(imported), Some(local)) = (imported, local) {
            if let Some(registration) = available.get(imported) {
                local_registrations.insert(local.to_owned(), registration.clone());
            }
        }
    }
}

fn collect_pattern_alias(
    pattern: Node<'_>,
    source: &[u8],
    expected: &str,
    aliases: &mut BTreeSet<String>,
) {
    let mut cursor = pattern.walk();
    for binding in pattern.named_children(&mut cursor) {
        let (imported, local) = pattern_binding(binding, source);
        if imported == Some(expected) {
            if let Some(local) = local {
                aliases.insert(local.to_owned());
            }
        }
    }
}

fn pattern_binding<'a>(binding: Node<'_>, source: &'a [u8]) -> (Option<&'a str>, Option<&'a str>) {
    match binding.kind() {
        "shorthand_property_identifier_pattern" => {
            let name = node_text(binding, source);
            (Some(name), Some(name))
        }
        "pair_pattern" => {
            let key = binding.child_by_field_name("key");
            let value = binding.child_by_field_name("value");
            match (key, value) {
                (Some(key), Some(value)) if value.kind() == "identifier" => {
                    (Some(node_text(key, source)), Some(node_text(value, source)))
                }
                _ => (None, None),
            }
        }
        _ => (None, None),
    }
}

fn collect_create_bdd_bindings(
    declarator: Node<'_>,
    source: &[u8],
    create_bdd_aliases: &BTreeSet<String>,
    local_registrations: &mut RegistrationExports,
    direct_export_names: &mut Vec<(String, String)>,
) {
    let (Some(pattern), Some(value)) = (
        declarator.child_by_field_name("name"),
        declarator.child_by_field_name("value"),
    ) else {
        return;
    };
    if pattern.kind() != "object_pattern"
        || call_identifier(value, source).is_none_or(|name| !create_bdd_aliases.contains(name))
    {
        return;
    }

    let directly_exported = is_directly_exported_variable(declarator);
    let available = registration_exports_for_framework(Framework::PlaywrightBdd);
    let mut cursor = pattern.walk();
    for binding in pattern.named_children(&mut cursor) {
        let (registration, local) = match binding.kind() {
            "shorthand_property_identifier_pattern" => {
                let name = node_text(binding, source);
                (name, name)
            }
            "pair_pattern" => {
                let (Some(key), Some(value)) = (
                    binding.child_by_field_name("key"),
                    binding.child_by_field_name("value"),
                ) else {
                    continue;
                };
                if value.kind() != "identifier" {
                    continue;
                }
                (node_text(key, source), node_text(value, source))
            }
            _ => continue,
        };
        if let Some(registration) = available.get(registration) {
            local_registrations.insert(local.to_owned(), registration.clone());
            if directly_exported {
                direct_export_names.push((local.to_owned(), local.to_owned()));
            }
        }
    }
}

fn call_identifier<'a>(mut call: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    while matches!(
        call.kind(),
        "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression"
    ) {
        call = call.named_child(0)?;
    }
    if call.kind() != "call_expression" {
        return None;
    }
    let mut function = call.child_by_field_name("function")?;
    while matches!(
        function.kind(),
        "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression"
            | "instantiation_expression"
    ) {
        function = function.named_child(0)?;
    }
    (function.kind() == "identifier").then(|| node_text(function, source))
}

fn is_directly_exported_variable(declarator: Node<'_>) -> bool {
    declarator
        .parent()
        .and_then(|declaration| declaration.parent())
        .is_some_and(|parent| parent.kind() == "export_statement")
}

fn project_boundary(importer: &Path) -> Option<PathBuf> {
    let parent = importer.parent()?;
    let parent = parent.canonicalize().ok()?;
    for ancestor in parent.ancestors() {
        if ancestor.join("package.json").is_file() {
            return Some(ancestor.to_owned());
        }
    }
    Some(parent)
}

fn parse_module(source: &str, path: &Path) -> Option<tree_sitter::Tree> {
    let language = language_for_path(path)?;
    let mut parser = Parser::new();
    parser.set_language(&grammar_for_language(language)).ok()?;
    parser.parse(source, None)
}

fn export_specifiers(export: Node<'_>, source: &[u8]) -> Vec<(String, String)> {
    let mut specifiers = Vec::new();
    let mut stack = vec![export];
    while let Some(node) = stack.pop() {
        if node.kind() == "export_specifier" && !is_type_only_specifier(node) {
            if let Some(name) = node.child_by_field_name("name") {
                let imported = node_text(name, source);
                let exported = node
                    .child_by_field_name("alias")
                    .map_or(imported, |alias| node_text(alias, source));
                specifiers.push((imported.to_owned(), exported.to_owned()));
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
    specifiers
}

#[cfg(test)]
mod tests;
