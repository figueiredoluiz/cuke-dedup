//! Bounded source-only resolution. No Ruby, gem metadata or installation hooks are executed.

use super::{descendants, literal_string, location, text};
use crate::config::Config;
use crate::discovery::DiscoveredFiles;
use crate::resource_limits::{
    read_utf8, MAX_PROJECT_INPUT_BYTES, MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES,
};
use crate::source_adapter::{DependencyGap, SourceDependency, SourceFile, SourceLanguage};
use globset::GlobSet;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

/// Resolves the Ruby source graph from the selected files into discovery's edges and gaps.
pub(crate) fn resolve(config: &Config, excludes: &GlobSet, files: &mut DiscoveredFiles) {
    if !files
        .definitions
        .iter()
        .any(|file| file.language == SourceLanguage::Ruby)
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
#[derive(Default)]
struct Expansion {
    modules: usize,
    bytes: usize,
    refused: Option<String>,
}

impl Expansion {
    /// Charges a new dependency-only target against `(modules, bytes)`. Selected entry points are
    /// never charged. Once a limit refuses a target every later new target is refused with the
    /// same reason, so a smaller file cannot slip in after the graph was cut.
    fn admit(&mut self, target: &Path, (max_modules, max_bytes): (usize, usize)) -> Option<String> {
        if self.refused.is_none() {
            // A target whose metadata vanishes after `is_file` is charged nothing here; reading it
            // fails next and extraction reports the file, so no false limit is blamed.
            let size = std::fs::metadata(target)
                .map_or(0, |meta| usize::try_from(meta.len()).unwrap_or(usize::MAX));
            let bytes = self.bytes.saturating_add(size);
            if self.modules >= max_modules {
                self.refused = Some(format!(
                    "the source graph exceeds the {}-file limit",
                    grouped(max_modules)
                ));
            } else if bytes > max_bytes {
                self.refused = Some(format!(
                    "the source graph exceeds the {} limit",
                    byte_limit(max_bytes)
                ));
            } else {
                self.modules += 1;
                self.bytes = bytes;
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
    excludes: &GlobSet,
    files: &mut DiscoveredFiles,
    budget: (usize, usize),
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
    // An entry point that cannot be canonicalized loses only its own edges; extraction reports it.
    let entry_points: BTreeSet<_> = queue
        .iter()
        .filter_map(|f| f.path.canonicalize().ok())
        .collect();
    let mut known = entry_points.clone();
    // Skipped Ruby files by canonical path. A load this graph admits analyzes the file, so it is no
    // longer skipped; targets are canonical, discovered paths derive from the normalized root.
    let mut skipped: std::collections::BTreeMap<_, _> = files
        .skipped_definitions
        .iter()
        .filter(|file| file.language == SourceLanguage::Ruby)
        .filter_map(|file| Some((file.path.canonicalize().ok()?, file.path.clone())))
        .collect();
    let mut expansion = Expansion::default();
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
        let tree = parser
            .parse(&source, None)
            .ok_or_else(|| anyhow::anyhow!("Ruby parser produced no dependency tree"))?;
        let program = tree.root_node();
        if program.has_error() {
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
            if target.strip_prefix(&root).is_ok_and(|relative| {
                relative.ancestors().any(|part| {
                    excludes.is_match(part) || excludes.is_match(format!("{}/", part.display()))
                })
            }) {
                continue;
            }
            if !known.contains(&target) {
                if let Some(reason) = expansion.admit(&target, budget) {
                    files
                        .dependency_gaps
                        .push(DependencyGap::new(location(&file, node, &source), reason));
                    continue;
                }
                known.insert(target.clone());
                if let Some(discovered) = skipped.remove(&target) {
                    files
                        .skipped_definitions
                        .retain(|file| file.path != discovered);
                }
                let source_file = SourceFile {
                    path: target.clone(),
                    language: SourceLanguage::Ruby,
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
    path.is_file().then(|| path.canonicalize().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolves the given selected files under `root` against an injected `(modules, bytes)` budget.
    fn resolve_with(root: &Path, selected: &[&str], budget: (usize, usize)) -> DiscoveredFiles {
        let config = Config::load(root, Default::default()).unwrap();
        let mut files = DiscoveredFiles {
            definitions: selected
                .iter()
                .map(|name| SourceFile {
                    path: root.join(name),
                    language: SourceLanguage::Ruby,
                })
                .collect(),
            ..DiscoveredFiles::default()
        };
        resolve_graph(&config, &GlobSet::empty(), &mut files, budget).unwrap();
        files
    }

    /// Lines of the loads that resolved to an edge, and of the loads refused with each reason.
    fn outcome(files: &DiscoveredFiles) -> (Vec<usize>, Vec<(usize, String)>) {
        let edges = files
            .dependencies
            .iter()
            .map(|edge| edge.location.line)
            .collect();
        let gaps = files
            .dependency_gaps
            .iter()
            .map(|gap| (gap.location.line, gap.reason.clone()))
            .collect();
        (edges, gaps)
    }

    /// A load past the byte or module budget is refused, and every later new load is refused too.
    #[test]
    fn expansion_budget_refuses_past_its_limits_and_stays_exhausted() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("a.rb"), "A = 1 # ten\n").unwrap();
        std::fs::write(root.join("b.rb"), "B = 1 # ten\n").unwrap();
        std::fs::write(root.join("c.rb"), "C\n").unwrap();
        std::fs::write(
            root.join("entry.rb"),
            "require_relative 'a'\nrequire_relative 'b'\nrequire_relative 'c'\nrequire_relative 'a'\n",
        )
        .unwrap();
        let bytes = "the source graph exceeds the 15-byte limit".to_owned();
        // `b` would take the graph past 15 bytes. The refusal is sticky: the 2-byte `c` that
        // would still fit is refused too, while the second load of the admitted `a` keeps its edge.
        let files = resolve_with(&root, &["entry.rb"], (8, 15));
        assert_eq!(
            outcome(&files),
            (vec![1, 4], vec![(2, bytes.clone()), (3, bytes)])
        );
        assert_eq!(files.definitions.len(), 2);
        assert!(files.errors.is_empty());

        let modules = "the source graph exceeds the 1-file limit".to_owned();
        let files = resolve_with(&root, &["entry.rb"], (1, usize::MAX));
        assert_eq!(
            outcome(&files),
            (vec![1, 4], vec![(2, modules.clone()), (3, modules)])
        );
    }

    /// Selected files are not charged; only dependency-only expansion spends the budget.
    #[test]
    fn selected_entry_points_never_charge_the_expansion_budget() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(
            root.join("first.rb"),
            "require_relative 'second'\nrequire_relative 'helper'\n",
        )
        .unwrap();
        std::fs::write(root.join("second.rb"), "SECOND = 1\n").unwrap();
        std::fs::write(root.join("helper.rb"), "HELPER = 1\n").unwrap();
        // Discriminating shape: two selected files and a one-module budget. The load between
        // selected files is not expansion, and the dependency-only `helper` fits only because the
        // two entry points are not charged; charging them would refuse it.
        let files = resolve_with(&root, &["first.rb", "second.rb"], (1, usize::MAX));
        assert_eq!(outcome(&files), (vec![1, 2], vec![]));
        let only = |edge: &SourceDependency| edge.dependency_only;
        assert_eq!(
            files.dependencies.iter().map(only).collect::<Vec<_>>(),
            [false, true]
        );
        // Opposite answer: an empty budget refuses the helper but still resolves the selected load.
        let files = resolve_with(&root, &["first.rb", "second.rb"], (0, 0));
        assert_eq!(
            outcome(&files),
            (
                vec![1],
                vec![(2, "the source graph exceeds the 0-file limit".to_owned())]
            )
        );
    }

    /// Requests resolve like Ruby: `.rb` as written, every other name with `.rb` appended, and a
    /// native-suffix name only when no native library shadows it.
    #[test]
    fn requests_resolve_to_the_file_ruby_loads() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        for name in [
            "checkout.v2.rb",
            "config.local.rb",
            "trailing..rb",
            "helper.rb",
            "plain.rb",
            "free.so.rb",
            "free.o.rb",
            "free.dll.rb",
            "free.bundle.rb",
            "upper.rb",
            "shadow_so.so.rb",
            "shadow_o.o.rb",
            "shadow_dll.dll.rb",
            "shadow_bundle.bundle.rb",
        ] {
            std::fs::write(root.join(name), "VALUE = 1\n").unwrap();
        }
        // An extensionless file is not a Ruby source; Ruby needs `bare.rb`. Each `shadow_*` load
        // has a native candidate under a different suffix, so every suffix is both a request and a
        // candidate in some row.
        for name in [
            "bare",
            "shadow_so.bundle",
            "shadow_o.so",
            "shadow_dll.o",
            "shadow_bundle.dll",
        ] {
            std::fs::write(root.join(name), "not a library\n").unwrap();
        }
        let requests = [
            "checkout.v2",
            "config.local",
            "trailing.",
            "helper.rb",
            "plain",
            "free.so",
            "free.o",
            "free.dll",
            "free.bundle",
            "upper.RB",
            "bare",
            "shadow_so.so",
            "shadow_o.o",
            "shadow_dll.dll",
            "shadow_bundle.bundle",
        ];
        let entry: Vec<_> = requests
            .iter()
            .map(|request| format!("require_relative '{request}'"))
            .collect();
        std::fs::write(root.join("entry.rb"), entry.join("\n")).unwrap();
        let files = resolve_with(&root, &["entry.rb"], (32, usize::MAX));
        assert_eq!(outcome(&files), ((1..=9).collect::<Vec<_>>(), vec![]));
        let targets: Vec<_> = files
            .dependencies
            .iter()
            .map(|edge| {
                edge.target
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            targets,
            [
                "checkout.v2.rb",
                "config.local.rb",
                "trailing..rb",
                "helper.rb",
                "plain.rb",
                "free.so.rb",
                "free.o.rb",
                "free.dll.rb",
                "free.bundle.rb",
            ]
        );
    }

    /// A native library in any searched base shadows a `.rb` fallback in a later one, as Ruby
    /// searches every load path for a native library first.
    #[test]
    fn a_native_candidate_in_any_base_shadows_the_ruby_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let (first, second) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(second.join("x.so.rb"), "X = 1\n").unwrap();
        let bases = [first.clone(), second.clone()];
        assert_eq!(request_target(&bases, "x.so"), Some(second.join("x.so.rb")));
        std::fs::write(first.join("x.bundle"), "not a library\n").unwrap();
        assert_eq!(request_target(&bases, "x.so"), None);
        // A non-native request ignores native files entirely.
        std::fs::write(second.join("y.v2.rb"), "Y = 1\n").unwrap();
        std::fs::write(first.join("y.bundle"), "not a library\n").unwrap();
        assert_eq!(request_target(&bases, "y.v2"), Some(second.join("y.v2.rb")));
    }

    /// A skipped Ruby file leaves the skipped list exactly when the graph admits a load of it:
    /// directly, through `rubyLoadPaths`, or transitively. A load refused by the budget, nested
    /// inside a method, or absent leaves the file skipped.
    #[test]
    fn admitted_loads_remove_files_from_the_skipped_list() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        std::fs::write(
            root.join(".cuke-dedup.json"),
            r#"{"rubyLoadPaths":["lib"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("entry.rb"),
            "require_relative 'a'\nrequire 'k'\ndef m\n  require_relative 'd'\nend\n",
        )
        .unwrap();
        std::fs::write(
            root.join("a.rb"),
            "require_relative 'c'\nrequire_relative 'g'\n",
        )
        .unwrap();
        for name in ["c.rb", "g.rb", "d.rb", "h.rb", "lib/k.rb"] {
            std::fs::write(root.join(name), "VALUE = 1\n").unwrap();
        }
        let config = Config::load(&root, Default::default()).unwrap();
        let ruby = |name: &str| SourceFile {
            path: root.join(name),
            language: SourceLanguage::Ruby,
        };
        let mut files = DiscoveredFiles {
            definitions: vec![ruby("entry.rb")],
            skipped_definitions: ["a.rb", "c.rb", "g.rb", "d.rb", "h.rb", "lib/k.rb"]
                .map(ruby)
                .to_vec(),
            ..DiscoveredFiles::default()
        };
        // Three dependency-only files fit: `a`, `k` (through the load path) and `c` (loaded by
        // `a`); `g` is the fourth and is refused.
        resolve_graph(&config, &GlobSet::empty(), &mut files, (3, usize::MAX)).unwrap();
        let skipped: Vec<_> = files
            .skipped_definitions
            .iter()
            .map(|file| {
                file.path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(skipped, ["g.rb", "d.rb", "h.rb"]);
        assert_eq!(files.dependency_gaps.len(), 1);
        assert_eq!(files.definitions.len(), 4);
    }

    /// An unreadable selected file skips only its own loads; the graph keeps resolving.
    #[cfg(unix)]
    #[test]
    fn an_unreadable_selected_file_loses_only_its_own_loads() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        std::fs::write(root.join("helper.rb"), "HELPER = 1\n").unwrap();
        std::fs::write(root.join("locked.rb"), "require_relative 'helper'\n").unwrap();
        std::fs::write(root.join("open.rb"), "\nrequire_relative 'helper'\n").unwrap();
        std::fs::set_permissions(
            root.join("locked.rb"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        // Root ignores file modes, so `locked.rb` stays readable and its line-1 load resolves too.
        let readable = std::fs::read(root.join("locked.rb")).is_ok();
        let expected_edges: Vec<usize> = [1, 2]
            .into_iter()
            .filter(|line| readable || *line == 2)
            .collect();
        let files = resolve_with(&root, &["locked.rb", "open.rb"], (8, usize::MAX));
        std::fs::set_permissions(
            root.join("locked.rb"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        // Discovery records no error; `open.rb` still resolves its load on line 2.
        assert!(files.errors.is_empty(), "{:?}", files.errors);
        assert_eq!(outcome(&files), (expected_edges, vec![]));
    }
}

#[cfg(test)]
mod wording {
    use super::{byte_limit, grouped};

    /// Limit numbers read like the documented table.
    #[test]
    fn limits_are_worded_like_the_documentation() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_024), "1,024");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(byte_limit(64 << 20), "64 MiB");
        assert_eq!(byte_limit(15), "15-byte");
        assert_eq!(byte_limit((1 << 20) + 1), "1,048,577-byte");
    }
}
