//! Bounded source-only resolution. No Ruby, gem metadata or installation hooks are executed.

use super::{descendants, literal_string, location, syntax_error, text};
use crate::config::{canonical_platform_path, Config};
use crate::discovery::DiscoveredFiles;
use crate::discovery::ExcludeSet;
use crate::resource_limits::{
    known_input_limit_exceeded, read_utf8, WorkBudget, MAX_PROJECT_INPUT_BYTES,
    MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES,
};
use crate::source_adapter::{DependencyGap, SourceDependency, SourceFile};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

/// Resolves the Ruby source graph from the selected files into discovery's edges and gaps.
pub(crate) fn resolve(config: &Config, excludes: &ExcludeSet, files: &mut DiscoveredFiles) {
    if !files
        .definitions
        .iter()
        .any(|file| file.language == super::RUBY)
    {
        return;
    }
    let budget = (MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES);
    if let Err(error) = resolve_graph(config, excludes, files, budget) {
        files.errors.push(format!(
            "Ruby dependency discovery is incomplete: {error:#}"
        ));
    }
    files.definitions.sort_by(|a, b| a.path.cmp(&b.path));
}

/// Dependency-only modules admitted so far and their bytes, plus the sticky refusal reason.
struct Expansion {
    modules: WorkBudget,
    bytes: WorkBudget,
    refused: Option<String>,
}

impl Expansion {
    /// An expansion with nothing admitted against `(modules, bytes)`.
    fn new((max_modules, max_bytes): (usize, usize)) -> Self {
        Self {
            modules: WorkBudget::new(max_modules),
            bytes: WorkBudget::new(max_bytes),
            refused: None,
        }
    }

    /// Charges a new dependency-only target. Selected entry points are never charged. Once a limit
    /// refuses a target every later new target is refused with the same reason, so a smaller file
    /// cannot slip in after the graph was cut.
    fn admit(&mut self, target: &Path) -> Option<String> {
        if self.refused.is_none() {
            // A target whose metadata vanishes after `is_file` is charged no bytes here; reading it
            // fails next and extraction reports the file, so no false byte limit is blamed.
            let size = std::fs::metadata(target)
                .map_or(0, |meta| usize::try_from(meta.len()).unwrap_or(usize::MAX));
            // `admit` does not latch, so `refused` is the latch; it also carries this adapter's
            // reason. A target refused for bytes leaves its module charged, but nothing is
            // charged after a refusal.
            if !self.modules.admit(1) {
                self.refused = Some(format!(
                    "the source graph exceeds the {}-file limit",
                    grouped(self.modules.limit())
                ));
            } else if !self.bytes.admit(size) {
                self.refused = Some(format!(
                    "the source graph exceeds the {} limit",
                    byte_limit(self.bytes.limit())
                ));
            }
        }
        self.refused.clone()
    }
}

/// `1024` as `1,024`, matching how the limits are documented.
pub(super) fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut text = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// A byte limit in whole MiB when it is one (`64 MiB`), otherwise in bytes (`15-byte`).
pub(super) fn byte_limit(bytes: usize) -> String {
    const MIB: usize = 1 << 20;
    if bytes >= MIB && bytes.is_multiple_of(MIB) {
        format!("{} MiB", grouped(bytes / MIB))
    } else {
        format!("{}-byte", grouped(bytes))
    }
}

/// Walks the Ruby source graph from the selected files, recording each followed top-level load
/// as an edge and each load refused by the expansion `budget` as a gap.
fn resolve_graph(
    config: &Config,
    excludes: &ExcludeSet,
    files: &mut DiscoveredFiles,
    budget: (usize, usize),
) -> anyhow::Result<()> {
    let root = config.root.clone();
    let load_paths = config
        .ruby_load_paths()
        .iter()
        .map(|path| canonical_platform_path(&root.join(path)))
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(
        load_paths.iter().all(|p| p.is_dir()),
        "rubyLoadPaths must contain source directories"
    );
    let mut queue: VecDeque<_> = files
        .definitions
        .iter()
        .filter(|f| f.language == super::RUBY)
        .cloned()
        .collect();
    // An entry point that cannot be canonicalized loses only its own edges; extraction reports it.
    let entry_points: BTreeSet<_> = queue
        .iter()
        .filter_map(|f| canonical_platform_path(&f.path).ok())
        .collect();
    let mut known = entry_points.clone();
    // Skipped Ruby files by canonical path. A load this graph admits analyzes the file, so it is no
    // longer skipped; targets are canonical, discovered paths derive from the normalized root.
    let mut skipped: std::collections::BTreeMap<_, _> = files
        .skipped_definitions
        .iter()
        .filter(|file| file.language == super::RUBY)
        .filter_map(|file| Some((canonical_platform_path(&file.path).ok()?, file.path.clone())))
        .collect();
    let mut promoted = BTreeSet::new();
    let mut expansion = Expansion::new(budget);
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_ruby::LANGUAGE.into())?;
    while let Some(file) = queue.pop_front() {
        // Extraction reports an unreadable source itself; only this file's loads stay unresolved.
        let Ok(source) = read_utf8(
            &file.path,
            "Ruby dependency source",
            MAX_PROJECT_INPUT_BYTES,
        ) else {
            continue;
        };
        let source = super::prepare_nuls(&source).into_owned();
        let tree = parser
            .parse(&source, None)
            .ok_or_else(|| anyhow::anyhow!("Ruby parser produced no dependency tree"))?;
        let program = tree.root_node();
        if syntax_error(program, &source).is_some() {
            continue;
        }
        // Top-level statements come in source order, so the first loads are admitted and a gap
        // lands on the load that crossed the limit.
        let mut cursor = program.walk();
        let statements: Vec<_> = program.named_children(&mut cursor).collect();
        for node in statements {
            if node.kind() != "call" || node.child_by_field_name("receiver").is_some() {
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
            let Some(target) = request_target(&bases, &request) else {
                continue;
            };
            if !target.starts_with(&root) && !load_paths.iter().any(|path| target.starts_with(path))
            {
                continue;
            }
            if target
                .strip_prefix(&root)
                .is_ok_and(|relative| excludes.excludes(relative))
            {
                continue;
            }
            if !known.contains(&target) {
                if let Some(reason) = known_input_limit_exceeded(
                    &target,
                    "Ruby dependency source",
                    MAX_PROJECT_INPUT_BYTES,
                ) {
                    files
                        .dependency_gaps
                        .push(DependencyGap::new(location(&file, node, &source), reason));
                    continue;
                }
                if let Some(reason) = expansion.admit(&target) {
                    files
                        .dependency_gaps
                        .push(DependencyGap::new(location(&file, node, &source), reason));
                    continue;
                }
                known.insert(target.clone());
                if let Some(discovered) = skipped.remove(&target) {
                    promoted.insert(discovered);
                }
                let source_file = SourceFile {
                    path: target.clone(),
                    language: super::RUBY,
                };
                files.definitions.push(source_file.clone());
                queue.push_back(source_file);
            }
            let dependency_only = !entry_points.contains(&target);
            files.dependencies.push(
                SourceDependency::new(location(&file, node, &source), target)
                    .with_dependency_only_target(dependency_only),
            );
        }
    }
    // One pass after the walk: removing each promoted file as it is admitted would rescan the
    // unbounded skipped list once per admitted file.
    files
        .skipped_definitions
        .retain(|file| !promoted.contains(&file.path));
    Ok(())
}

/// Suffixes Ruby treats as native libraries: `.so`, `.o` and `.dll` map to the platform's library
/// suffix, and `.bundle` is that suffix on macOS.
const NATIVE_SUFFIXES: [&str; 4] = ["so", "o", "dll", "bundle"];

/// The Ruby source that `request` loads from the first of `bases` holding it, as Ruby resolves
/// `require`/`require_relative`. A `.rb` request loads as written and every other request with
/// `.rb` appended, dotted names included (`checkout.v2` loads `checkout.v2.rb`, `trailing.`
/// loads `trailing..rb`); the extension test is case-sensitive. A native-suffix request falls back
/// to `<request>.rb` only when no base holds a native library for it.
pub(super) fn request_target(bases: &[PathBuf], request: &str) -> Option<PathBuf> {
    let native = Path::new(request)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| NATIVE_SUFFIXES.contains(&extension));
    // fail-closed: Ruby searches every base for a native library before any `.rb` fallback, and
    // which suffix it accepts depends on the platform, so any native candidate leaves the load
    // unresolved rather than guessing.
    if native
        && bases.iter().any(|base| {
            NATIVE_SUFFIXES
                .iter()
                .any(|suffix| base.join(request).with_extension(suffix).is_file())
        })
    {
        return None;
    }
    bases.iter().find_map(|base| ruby_file(&base.join(request)))
}

/// `path` as a Ruby source: kept when it ends in `.rb`, otherwise with `.rb` appended; canonical
/// when that file exists.
fn ruby_file(path: &Path) -> Option<PathBuf> {
    let path = if path.extension().is_some_and(|extension| extension == "rb") {
        path.to_owned()
    } else {
        let mut candidate = path.as_os_str().to_owned();
        candidate.push(".rb");
        PathBuf::from(candidate)
    };
    path.is_file()
        .then(|| canonical_platform_path(&path).ok())
        .flatten()
}

#[cfg(test)]
mod tests;
