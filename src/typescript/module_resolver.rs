use super::node_text;
use crate::resource_limits::{
    read_utf8, MAX_PROJECT_INPUT_BYTES, MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES,
    MAX_REGISTRATION_RESOLUTION_STATES,
};
use crate::source_adapter::language_for_path;
use anyhow::{bail, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};

const REGISTRATIONS: [&str; 7] = [
    "Given",
    "When",
    "Then",
    "defineStep",
    "given",
    "when",
    "then",
];
const FRAMEWORK_MODULES: [&str; 3] = [
    "@cucumber/cucumber",
    "playwright-bdd",
    "@badeball/cypress-cucumber-preprocessor",
];
const MAX_REEXPORT_DEPTH: usize = 16;
const MODULE_SUFFIXES: [&str; 8] = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

#[derive(Debug, Clone)]
struct Reexport {
    module: String,
    specifiers: Vec<(String, String)>,
    star: bool,
}

#[derive(Default)]
struct ResolutionState {
    active: BTreeSet<PathBuf>,
    exports: BTreeMap<(PathBuf, usize), Option<BTreeMap<String, String>>>,
    modules: BTreeMap<PathBuf, Option<Vec<Reexport>>>,
    module_bytes: usize,
    resolutions_started: usize,
}

#[derive(Debug)]
struct ResolvedExports {
    exports: Option<BTreeMap<String, String>>,
    cacheable: bool,
}

#[derive(Default)]
pub(super) struct RegistrationResolver {
    boundary: Option<PathBuf>,
    state: ResolutionState,
}

impl RegistrationResolver {
    pub(super) fn for_root(root: &Path) -> Self {
        Self {
            boundary: root.canonicalize().ok(),
            state: ResolutionState::default(),
        }
    }

    pub(super) fn registration_exports(
        &mut self,
        importer: &Path,
        specifier: &str,
    ) -> Result<Option<BTreeMap<String, String>>> {
        let boundary = self.boundary.clone().or_else(|| project_boundary(importer));
        let Some(boundary) = boundary else {
            return Ok(None);
        };
        Ok(resolve_exports(importer, specifier, &boundary, &mut self.state, 0)?.exports)
    }
}

fn resolve_exports(
    importer: &Path,
    specifier: &str,
    boundary: &Path,
    state: &mut ResolutionState,
    depth: usize,
) -> Result<ResolvedExports> {
    if depth >= MAX_REEXPORT_DEPTH {
        return Ok(ResolvedExports {
            exports: None,
            cacheable: true,
        });
    }
    let Some(path) = resolve_module(importer, specifier, boundary) else {
        return Ok(ResolvedExports {
            exports: None,
            cacheable: true,
        });
    };
    let remaining_depth = MAX_REEXPORT_DEPTH - depth;
    let cache_key = (path.clone(), remaining_depth);
    if let Some(exports) = state.exports.get(&cache_key) {
        return Ok(ResolvedExports {
            exports: exports.clone(),
            cacheable: true,
        });
    }
    if !state.active.insert(path.clone()) {
        return Ok(ResolvedExports {
            exports: None,
            cacheable: false,
        });
    }
    if state.resolutions_started >= MAX_REGISTRATION_RESOLUTION_STATES {
        state.active.remove(&path);
        bail!(
            "registration module graph exceeds the {}-state resolution limit",
            MAX_REGISTRATION_RESOLUTION_STATES
        );
    }
    state.resolutions_started += 1;

    let Some(reexports) = load_module(&path, state)? else {
        state.active.remove(&path);
        state.exports.insert(cache_key, None);
        return Ok(ResolvedExports {
            exports: None,
            cacheable: true,
        });
    };
    let mut exports = BTreeMap::new();
    let mut cacheable = true;
    for reexport in reexports {
        let available = if FRAMEWORK_MODULES.contains(&reexport.module.as_str()) {
            default_exports()
        } else if is_relative_module(&reexport.module) {
            let resolved = resolve_exports(&path, &reexport.module, boundary, state, depth + 1)?;
            cacheable &= resolved.cacheable;
            resolved.exports.unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        for (imported, exported) in &reexport.specifiers {
            if let Some(canonical) = available.get(imported) {
                exports.insert(exported.clone(), canonical.clone());
            }
        }
        if reexport.star {
            exports.extend(available);
        }
    }
    state.active.remove(&path);
    let exports = Some(exports);
    if cacheable {
        state.exports.insert(cache_key, exports.clone());
    }
    Ok(ResolvedExports { exports, cacheable })
}

fn load_module(path: &Path, state: &mut ResolutionState) -> Result<Option<Vec<Reexport>>> {
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
    let module = parse_module(&source, path).map(|tree| collect_reexports(&tree, &source));
    state.modules.insert(path.to_owned(), module.clone());
    Ok(module)
}

fn collect_reexports(tree: &tree_sitter::Tree, source: &str) -> Vec<Reexport> {
    let mut reexports = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "export_statement" && !is_type_only_export(node, source.as_bytes()) {
            if let Some(module) = import_module(node, source.as_bytes()) {
                reexports.push(Reexport {
                    module: module.to_owned(),
                    specifiers: export_specifiers(node, source.as_bytes()),
                    star: is_star_export(node, source.as_bytes()),
                });
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
    reexports
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

fn resolve_module(importer: &Path, specifier: &str, boundary: &Path) -> Option<PathBuf> {
    if !is_relative_module(specifier) {
        return None;
    }
    let base = importer.parent()?.join(specifier);
    let mut candidates = vec![base.clone()];
    for suffix in MODULE_SUFFIXES {
        candidates.push(PathBuf::from(format!("{}{suffix}", base.display())));
        candidates.push(base.join(format!("index{suffix}")));
    }
    candidates.into_iter().find_map(|candidate| {
        if language_for_path(&candidate).is_none() || !candidate.is_file() {
            return None;
        }
        let canonical = candidate.canonicalize().ok()?;
        canonical.starts_with(boundary).then_some(canonical)
    })
}

fn parse_module(source: &str, path: &Path) -> Option<tree_sitter::Tree> {
    let language = language_for_path(path)?;
    let grammar = match language {
        crate::source_adapter::SourceLanguage::JavaScript => {
            tree_sitter_javascript::LANGUAGE.into()
        }
        crate::source_adapter::SourceLanguage::TypeScript => {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
        }
        crate::source_adapter::SourceLanguage::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
    };
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    parser.parse(source, None)
}

fn export_specifiers(export: Node<'_>, source: &[u8]) -> Vec<(String, String)> {
    let mut specifiers = Vec::new();
    let mut stack = vec![export];
    while let Some(node) = stack.pop() {
        if node.kind() == "export_specifier"
            && !node_text(node, source).trim_start().starts_with("type ")
        {
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

fn import_module<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let module = node.child_by_field_name("source")?;
    let text = node_text(module, source);
    let quote = text.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') || text.as_bytes().last().copied()? != quote {
        return None;
    }
    text.get(1..text.len() - 1)
}

fn is_star_export(node: Node<'_>, source: &[u8]) -> bool {
    node_text(node, source)
        .trim_start()
        .strip_prefix("export")
        .and_then(|rest| rest.trim_start().strip_prefix('*'))
        .is_some_and(|rest| rest.trim_start().starts_with("from"))
}

fn is_type_only_export(node: Node<'_>, source: &[u8]) -> bool {
    node_text(node, source)
        .trim_start()
        .strip_prefix("export")
        .is_some_and(|rest| {
            let rest = rest.trim_start();
            rest.strip_prefix("type").is_some_and(|after_type| {
                after_type.is_empty()
                    || after_type.starts_with(char::is_whitespace)
                    || after_type.starts_with('{')
            })
        })
}

fn is_relative_module(module: &str) -> bool {
    module == "." || module == ".." || module.starts_with("./") || module.starts_with("../")
}

fn default_exports() -> BTreeMap<String, String> {
    REGISTRATIONS
        .into_iter()
        .map(|name| (name.to_owned(), name.to_owned()))
        .collect()
}

fn push_named_children_reverse<'tree>(node: Node<'tree>, stack: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    stack.extend(children.into_iter().rev());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn module_resolution_supports_files_and_indexes_but_rejects_escapes_and_bare_names() {
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

        let cases = [
            ("extension inference", "../direct", Some("direct.ts")),
            ("explicit extension", "../direct.ts", Some("direct.ts")),
            ("directory index", "../support", Some("support/index.ts")),
            ("bare package", "@example/support", None),
            ("missing relative", "../missing", None),
        ];
        for (name, specifier, expected_suffix) in cases {
            let resolved = resolve_module(&importer, specifier, &boundary);
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
            "../../{}",
            outside.path().file_name().unwrap().to_string_lossy()
        );
        assert_eq!(resolve_module(&importer, &escape, &boundary), None);
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
        let mut state = ResolutionState::default();
        let resolved = resolve_exports(&importer, "./root", &boundary, &mut state, 0).unwrap();

        assert_eq!(
            resolved.exports.unwrap().get("Given").map(String::as_str),
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
        resolver.registration_exports(&first, "../support").unwrap();
        let modules_after_first = resolver.state.modules.len();
        let resolutions_after_first = resolver.state.resolutions_started;
        resolver
            .registration_exports(&second, "../support")
            .unwrap();

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
            .unwrap();

        assert_eq!(exports.get("Given").map(String::as_str), Some("Given"));
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
            .unwrap();
        let shared = directory.path().join("shared.ts").canonicalize().unwrap();
        let shared_states = resolver
            .state
            .exports
            .keys()
            .filter(|(path, _)| path == &shared)
            .count();

        assert_eq!(exports.get("Given").map(String::as_str), Some("Given"));
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
        let mut state = ResolutionState::default();
        let from_a = resolve_exports(&importer, "./a", &boundary, &mut state, 0).unwrap();
        let from_b = resolve_exports(&importer, "./b", &boundary, &mut state, 0).unwrap();

        assert_eq!(
            from_a.exports.unwrap().get("Given").map(String::as_str),
            Some("Given")
        );
        assert_eq!(
            from_b.exports.unwrap().get("Given").map(String::as_str),
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
        let mut resolution_state = ResolutionState {
            resolutions_started: MAX_REGISTRATION_RESOLUTION_STATES,
            ..ResolutionState::default()
        };
        let error = resolve_exports(&importer, "./module", &boundary, &mut resolution_state, 0)
            .unwrap_err();
        assert!(error.to_string().contains("state resolution limit"));
    }
}
