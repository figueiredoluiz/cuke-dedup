use super::node_text;
use crate::source_adapter::language_for_path;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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

pub(super) fn registration_exports(
    importer: &Path,
    specifier: &str,
) -> Option<BTreeMap<String, String>> {
    let boundary = project_boundary(importer)?;
    let mut visited = BTreeSet::new();
    resolve_exports(importer, specifier, &boundary, &mut visited, 0)
}

fn resolve_exports(
    importer: &Path,
    specifier: &str,
    boundary: &Path,
    visited: &mut BTreeSet<PathBuf>,
    depth: usize,
) -> Option<BTreeMap<String, String>> {
    if depth >= MAX_REEXPORT_DEPTH {
        return None;
    }
    let path = resolve_module(importer, specifier, boundary)?;
    if !visited.insert(path.clone()) {
        return None;
    }
    let source = fs::read_to_string(&path).ok()?;
    let tree = parse_module(&source, &path)?;
    let mut exports = BTreeMap::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "export_statement" && !is_type_only_export(node, source.as_bytes()) {
            if let Some(module) = import_module(node, source.as_bytes()) {
                let available = if FRAMEWORK_MODULES.contains(&module) {
                    default_exports()
                } else if is_relative_module(module) {
                    resolve_exports(&path, module, boundary, visited, depth + 1).unwrap_or_default()
                } else {
                    BTreeMap::new()
                };
                collect_export_specifiers(node, source.as_bytes(), &available, &mut exports);
                if is_star_export(node, source.as_bytes()) {
                    exports.extend(available);
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
    visited.remove(&path);
    Some(exports)
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

fn collect_export_specifiers(
    export: Node<'_>,
    source: &[u8],
    available: &BTreeMap<String, String>,
    output: &mut BTreeMap<String, String>,
) {
    let mut stack = vec![export];
    while let Some(node) = stack.pop() {
        if node.kind() == "export_specifier"
            && !node_text(node, source).trim_start().starts_with("type ")
        {
            if let Some(name) = node.child_by_field_name("name") {
                let imported = node_text(name, source);
                if let Some(canonical) = available.get(imported) {
                    let exported = node
                        .child_by_field_name("alias")
                        .map_or(imported, |alias| node_text(alias, source));
                    output.insert(exported.to_owned(), canonical.clone());
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
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
