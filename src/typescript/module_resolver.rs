use super::ast::{
    framework_for_module, import_has_runtime_bindings, import_module, is_star_export,
    is_type_only_declaration, is_type_only_specifier, push_named_children_reverse,
    registration_exports_for_framework, registration_exports_for_module, string_literal,
    RegistrationExports, FRAMEWORK_MODULES,
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
type CachedExports = (Option<RegistrationExports>, Framework);

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
}

#[derive(Debug)]
pub(super) struct RegistrationResolution {
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
            Ok(resolved) => Ok(RegistrationOutcome {
                resolution: resolved.exports.map(|exports| RegistrationResolution {
                    exports,
                    framework: resolved.framework,
                }),
                reason: None,
            }),
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
        });
    }
    let Some(path) = project.resolve(importer, specifier, boundary)? else {
        return Ok(ResolvedExports {
            exports: None,
            framework: Framework::Unknown,
            cacheable: true,
        });
    };
    let remaining_depth = MAX_REEXPORT_DEPTH - depth;
    let cache_key = (path.clone(), remaining_depth);
    if let Some((exports, framework)) = state.exports.get(&cache_key) {
        return Ok(ResolvedExports {
            exports: exports.clone(),
            framework: *framework,
            cacheable: true,
        });
    }
    if !state.active.insert(path.clone()) {
        return Ok(ResolvedExports {
            exports: None,
            framework: Framework::Unknown,
            cacheable: false,
        });
    }
    let resolved = resolve_active_exports(&path, boundary, project, state, depth);
    // Resolution failures are converted to source-local diagnostics by the caller. Always clear
    // the active-path marker before propagating one, or a later independent import is mistaken
    // for a cycle and silently loses registrations.
    state.active.remove(&path);
    let resolved = resolved?;
    if resolved.cacheable {
        state
            .exports
            .insert(cache_key, (resolved.exports.clone(), resolved.framework));
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
        });
    };
    let mut exports = module.direct_exports;
    let mut framework = module.framework;
    let mut cacheable = true;
    for reexport in module.reexports {
        let (available, available_framework) =
            if FRAMEWORK_MODULES.contains(&reexport.module.as_str()) {
                (
                    registration_exports_for_module(&reexport.module),
                    framework_for_module(&reexport.module),
                )
            } else {
                let resolved =
                    resolve_exports(path, &reexport.module, boundary, project, state, depth + 1)?;
                cacheable &= resolved.cacheable;
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
                    if FRAMEWORK_MODULES.contains(&module) {
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
                    export.kind == super::ast::RegistrationExportKind::Factory
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
    if FRAMEWORK_MODULES.contains(&module) {
        collect_pattern_registrations(
            pattern,
            source,
            &registration_exports_for_module(module),
            local_registrations,
        );
    }
    if module == super::ast::PLAYWRIGHT_MODULE {
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

fn call_string_argument<'a>(call: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let argument = arguments.named_children(&mut cursor).next()?;
    string_literal(argument, source)
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
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn project_resolution_supports_relative_modules_and_rejects_escapes() {
        let directory = tempfile::tempdir().unwrap();
        let boundary = directory.path().canonicalize().unwrap();
        let importer = directory.path().join("nested/steps.ts");
        fs::create_dir(directory.path().join("nested")).unwrap();
        fs::create_dir(directory.path().join("support")).unwrap();
        fs::write(&importer, "").unwrap();
        fs::write(directory.path().join("direct.ts"), "").unwrap();
        fs::write(directory.path().join("support/index.ts"), "").unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("escaped.ts"), "").unwrap();
        let mut project = ProjectResolution::for_root(directory.path());

        let cases = [
            ("extension inference", "../direct", Some("direct.ts")),
            ("explicit extension", "../direct.ts", Some("direct.ts")),
            ("directory index", "../support", Some("support/index.ts")),
            ("bare package", "@example/support", None),
            ("missing relative", "../missing", None),
        ];
        for (name, specifier, expected_suffix) in cases {
            let resolved = project.resolve(&importer, specifier, &boundary).unwrap();
            assert_eq!(
                resolved.as_ref().map(|path| {
                    path.strip_prefix(&boundary)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/")
                }),
                expected_suffix.map(str::to_owned),
                "{name}"
            );
        }

        let escape = format!(
            "../../{}/escaped",
            outside.path().file_name().unwrap().to_string_lossy()
        );
        let error = project.resolve(&importer, &escape, &boundary).unwrap_err();
        assert!(error
            .to_string()
            .contains("outside package or analysis root"));
    }

    #[test]
    fn branching_diamond_reexports_are_memoized_by_path_and_remaining_depth() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), "{}").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        fs::write(
            directory.path().join("root.ts"),
            "export * from './a0';\nexport * from './b0';\n",
        )
        .unwrap();

        let layers = 10;
        for layer in 0..layers {
            let contents = if layer + 1 == layers {
                "export * from '@cucumber/cucumber';\n".to_owned()
            } else {
                format!(
                    "export * from './a{}';\nexport * from './b{}';\n",
                    layer + 1,
                    layer + 1
                )
            };
            fs::write(directory.path().join(format!("a{layer}.ts")), &contents).unwrap();
            fs::write(directory.path().join(format!("b{layer}.ts")), &contents).unwrap();
        }

        let boundary = project_boundary(&importer).unwrap();
        let mut project = ProjectResolution::for_root(directory.path());
        let mut state = ResolutionState::default();
        let resolved =
            resolve_exports(&importer, "./root", &boundary, &mut project, &mut state, 0).unwrap();

        assert_eq!(
            resolved
                .exports
                .unwrap()
                .get("Given")
                .map(|export| export.canonical.as_str()),
            Some("Given")
        );
        assert_eq!(state.modules.len(), 1 + layers * 2);
        assert_eq!(state.resolutions_started, 1 + layers * 2);
    }

    #[test]
    fn resolver_reuses_cached_modules_across_importing_files() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), "{}").unwrap();
        fs::create_dir(directory.path().join("first")).unwrap();
        fs::create_dir(directory.path().join("second")).unwrap();
        let first = directory.path().join("first/steps.ts");
        let second = directory.path().join("second/steps.ts");
        fs::write(&first, "").unwrap();
        fs::write(&second, "").unwrap();
        fs::write(
            directory.path().join("support.ts"),
            "export { Given } from '@cucumber/cucumber';\n",
        )
        .unwrap();

        let mut resolver = RegistrationResolver::for_root(directory.path());
        let _ = resolver.registration_exports(&first, "../support");
        let modules_after_first = resolver.state.modules.len();
        let resolutions_after_first = resolver.state.resolutions_started;
        let _ = resolver.registration_exports(&second, "../support");

        assert_eq!(resolver.state.modules.len(), modules_after_first);
        assert_eq!(resolver.state.resolutions_started, resolutions_after_first);
    }

    #[test]
    fn configured_analysis_root_allows_cross_package_reexports() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("packages/app")).unwrap();
        fs::write(directory.path().join("packages/app/package.json"), "{}").unwrap();
        let importer = directory.path().join("packages/app/steps.ts");
        fs::write(&importer, "").unwrap();
        fs::write(
            directory.path().join("support.ts"),
            "export { Given } from '@cucumber/cucumber';\n",
        )
        .unwrap();

        let mut resolver = RegistrationResolver::for_root(directory.path());
        let exports = resolver
            .registration_exports(&importer, "../../support")
            .unwrap()
            .resolution
            .unwrap();

        assert_eq!(
            exports
                .exports
                .get("Given")
                .map(|export| export.canonical.as_str()),
            Some("Given")
        );
        assert_eq!(exports.framework, Framework::CucumberJs);
    }

    #[test]
    fn depth_diverse_graphs_cache_each_module_depth_within_the_state_budget() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), "{}").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        fs::write(
            directory.path().join("shared.ts"),
            "export { Given } from '@cucumber/cucumber';\n",
        )
        .unwrap();
        fs::write(
            directory.path().join("root.ts"),
            "export * from './shared';\nexport * from './chain0';\n",
        )
        .unwrap();
        let chain_depth = 8;
        for depth in 0..chain_depth {
            let next = if depth + 1 == chain_depth {
                String::new()
            } else {
                format!("export * from './chain{}';\n", depth + 1)
            };
            fs::write(
                directory.path().join(format!("chain{depth}.ts")),
                format!("export * from './shared';\n{next}"),
            )
            .unwrap();
        }

        let mut resolver = RegistrationResolver::for_root(directory.path());
        let exports = resolver
            .registration_exports(&importer, "./root")
            .unwrap()
            .resolution
            .unwrap();
        let shared = directory.path().join("shared.ts").canonicalize().unwrap();
        let shared_states = resolver
            .state
            .exports
            .keys()
            .filter(|(path, _)| path == &shared)
            .count();

        assert_eq!(
            exports
                .exports
                .get("Given")
                .map(|export| export.canonical.as_str()),
            Some("Given")
        );
        assert_eq!(exports.framework, Framework::CucumberJs);
        assert_eq!(shared_states, chain_depth + 1);
        assert!(resolver.state.resolutions_started < MAX_REGISTRATION_RESOLUTION_STATES);
    }

    #[test]
    fn cycles_do_not_poison_context_dependent_export_results() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), "{}").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        fs::write(
            directory.path().join("a.ts"),
            "export * from './b';\nexport { Given } from '@cucumber/cucumber';\n",
        )
        .unwrap();
        fs::write(directory.path().join("b.ts"), "export * from './a';\n").unwrap();

        let boundary = project_boundary(&importer).unwrap();
        let mut project = ProjectResolution::for_root(directory.path());
        let mut state = ResolutionState::default();
        let from_a =
            resolve_exports(&importer, "./a", &boundary, &mut project, &mut state, 0).unwrap();
        let from_b =
            resolve_exports(&importer, "./b", &boundary, &mut project, &mut state, 0).unwrap();

        assert_eq!(
            from_a
                .exports
                .unwrap()
                .get("Given")
                .map(|export| export.canonical.as_str()),
            Some("Given")
        );
        assert_eq!(
            from_b
                .exports
                .unwrap()
                .get("Given")
                .map(|export| export.canonical.as_str()),
            Some("Given")
        );
    }

    #[test]
    fn aggregate_module_graph_limits_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), "{}").unwrap();
        let importer = directory.path().join("steps.ts");
        let module = directory.path().join("module.ts");
        let module_source = "export { Given } from '@cucumber/cucumber';\n";
        fs::write(&importer, "").unwrap();
        fs::write(&module, module_source).unwrap();

        let mut module_count_state = ResolutionState::default();
        for index in 0..MAX_REGISTRATION_MODULES {
            module_count_state
                .modules
                .insert(PathBuf::from(format!("cached-{index}")), None);
        }
        let error = load_module(&module, &mut module_count_state).unwrap_err();
        assert!(error.to_string().contains("module resolution limit"));

        let mut byte_state = ResolutionState {
            module_bytes: MAX_REGISTRATION_MODULE_BYTES,
            ..ResolutionState::default()
        };
        let error = load_module(&module, &mut byte_state).unwrap_err();
        assert!(error.to_string().contains("aggregate resolution limit"));

        let mut exact_byte_state = ResolutionState {
            module_bytes: MAX_REGISTRATION_MODULE_BYTES - module_source.len(),
            ..ResolutionState::default()
        };
        assert!(load_module(&module, &mut exact_byte_state)
            .unwrap()
            .is_some());
        assert_eq!(exact_byte_state.module_bytes, MAX_REGISTRATION_MODULE_BYTES);

        let boundary = project_boundary(&importer).unwrap();
        let mut project = ProjectResolution::for_root(directory.path());
        let mut resolution_state = ResolutionState {
            resolutions_started: MAX_REGISTRATION_RESOLUTION_STATES,
            ..ResolutionState::default()
        };
        let error = resolve_exports(
            &importer,
            "./module",
            &boundary,
            &mut project,
            &mut resolution_state,
            0,
        )
        .unwrap_err();
        assert!(error.to_string().contains("state resolution limit"));
    }

    #[test]
    fn malformed_reexport_modules_cannot_supply_recovered_registrations() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), "{}").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        fs::write(
            directory.path().join("broken.ts"),
            "export { Given } from '@cucumber/cucumber';\nconst broken = ;\n",
        )
        .unwrap();

        let mut resolver = RegistrationResolver::for_root(directory.path());
        let outcome = resolver
            .registration_exports(&importer, "./broken")
            .expect("a malformed barrel is not an operational failure");

        // A malformed barrel yields no registrations and an explainable reason, never an error
        // that would abort analysis of the importing file.
        assert!(outcome.resolution.is_none());
        let reason = outcome.reason.expect("static resolution reason");
        assert!(reason.contains("syntax errors"), "{reason}");
        assert!(reason.contains("refusing recovered exports"), "{reason}");
    }

    #[test]
    fn namespace_star_exports_are_not_flattened_into_direct_registrations() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), "{}").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        fs::write(
            directory.path().join("support.ts"),
            "export { Given } from '@cucumber/cucumber';\n",
        )
        .unwrap();
        fs::write(
            directory.path().join("plain.ts"),
            "export /* comment */ * from './support';\n",
        )
        .unwrap();
        fs::write(
            directory.path().join("namespace.ts"),
            "export /* comment */ * as cucumber from './support';\n",
        )
        .unwrap();
        fs::write(
            directory.path().join("types.ts"),
            "export { type Config } from 'playwright-bdd';\n",
        )
        .unwrap();

        let mut resolver = RegistrationResolver::for_root(directory.path());
        let plain = resolver
            .registration_exports(&importer, "./plain")
            .unwrap()
            .resolution
            .unwrap();
        assert_eq!(
            plain
                .exports
                .get("Given")
                .map(|export| export.canonical.as_str()),
            Some("Given")
        );

        let namespace = resolver
            .registration_exports(&importer, "./namespace")
            .unwrap()
            .resolution
            .unwrap();
        assert!(namespace.exports.is_empty());
        assert_eq!(namespace.framework, Framework::Unknown);

        let types = resolver
            .registration_exports(&importer, "./types")
            .unwrap()
            .resolution
            .unwrap();
        assert!(types.exports.is_empty());
        assert_eq!(types.framework, Framework::Unknown);
    }
}
