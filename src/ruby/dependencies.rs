//! Bounded source-only resolution. No Ruby, gem metadata or installation hooks are executed.

use super::{descendants, literal_string, location, text};
use crate::config::Config;
use crate::discovery::DiscoveredFiles;
use crate::resource_limits::{
    read_utf8, MAX_PROJECT_INPUT_BYTES, MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES,
};
use crate::source_adapter::{SourceDependency, SourceFile, SourceLanguage};
use globset::GlobSet;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

pub(crate) fn resolve(config: &Config, excludes: &GlobSet, files: &mut DiscoveredFiles) {
    if !files
        .definitions
        .iter()
        .any(|file| file.language == SourceLanguage::Ruby)
    {
        return;
    }
    if let Err(error) = resolve_graph(config, excludes, files) {
        files.errors.push(format!(
            "Ruby dependency discovery is incomplete: {error:#}"
        ));
    }
    files.definitions.sort_by(|a, b| a.path.cmp(&b.path));
}

fn resolve_graph(
    config: &Config,
    excludes: &GlobSet,
    files: &mut DiscoveredFiles,
) -> anyhow::Result<()> {
    let root = config.root.canonicalize()?;
    let load_paths = config
        .ruby_load_paths()
        .iter()
        .map(|path| root.join(path).canonicalize())
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(
        load_paths.iter().all(|p| p.is_dir()),
        "rubyLoadPaths must contain source directories"
    );
    let mut queue: VecDeque<_> = files
        .definitions
        .iter()
        .filter(|f| f.language == SourceLanguage::Ruby)
        .cloned()
        .collect();
    let mut known: BTreeSet<_> = queue
        .iter()
        .map(|f| f.path.canonicalize())
        .collect::<Result<_, _>>()?;
    let mut bytes = 0_usize;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_ruby::LANGUAGE.into())?;
    while let Some(file) = queue.pop_front() {
        anyhow::ensure!(
            known.len() <= MAX_REGISTRATION_MODULES,
            "source graph exceeds the module limit"
        );
        let source = read_utf8(
            &file.path,
            "Ruby dependency source",
            MAX_PROJECT_INPUT_BYTES,
        )?;
        bytes = bytes.saturating_add(source.len());
        anyhow::ensure!(
            bytes <= MAX_REGISTRATION_MODULE_BYTES,
            "source graph exceeds the byte limit"
        );
        let tree = parser
            .parse(&source, None)
            .ok_or_else(|| anyhow::anyhow!("Ruby parser produced no dependency tree"))?;
        let program = tree.root_node();
        if program.has_error() {
            continue;
        }
        for node in descendants(program) {
            if node.kind() != "call"
                || node.parent() != Some(program)
                || node.child_by_field_name("receiver").is_some()
            {
                continue;
            }
            let Some(method) = node.child_by_field_name("method").map(|n| text(n, &source)) else {
                continue;
            };
            if !matches!(method, "require" | "require_relative") {
                continue;
            }
            let Some(args) = node
                .child_by_field_name("arguments")
                .filter(|a| a.named_child_count() == 1)
            else {
                continue;
            };
            let Some(argument) = args.named_child(0).filter(|a| a.kind() == "string") else {
                continue;
            };
            if descendants(argument)
                .iter()
                .any(|n| n.kind() == "interpolation")
            {
                continue;
            }
            let Some(request) = literal_string(text(argument, &source)) else {
                continue;
            };
            let bases = if method == "require_relative" {
                vec![file.path.parent().unwrap_or(&root).to_owned()]
            } else if request.starts_with("./")
                || request.starts_with("../")
                || Path::new(&request).is_absolute()
            {
                vec![root.clone()]
            } else {
                load_paths.clone()
            };
            let candidate = bases
                .iter()
                .find_map(|base| ruby_file(&base.join(&request)));
            let Some(target) = candidate else { continue };
            if !target.starts_with(&root) && !load_paths.iter().any(|path| target.starts_with(path))
            {
                continue;
            }
            if target.strip_prefix(&root).is_ok_and(|relative| {
                relative.ancestors().any(|part| {
                    excludes.is_match(part) || excludes.is_match(format!("{}/", part.display()))
                })
            }) {
                continue;
            }
            if known.insert(target.clone()) {
                anyhow::ensure!(
                    known.len() <= MAX_REGISTRATION_MODULES,
                    "source graph exceeds the module limit"
                );
                let source_file = SourceFile {
                    path: target.clone(),
                    language: SourceLanguage::Ruby,
                };
                files.definitions.push(source_file.clone());
                queue.push_back(source_file);
            }
            files.dependencies.push(SourceDependency::new(
                location(&file, node, &source),
                target,
            ));
        }
    }
    Ok(())
}

fn ruby_file(path: &Path) -> Option<PathBuf> {
    let path = match path.extension() {
        Some(extension) if extension == "rb" => path.to_owned(),
        None => PathBuf::from(format!("{}.rb", path.display())),
        _ => return None,
    };
    path.is_file().then(|| path.canonicalize().ok()).flatten()
}
