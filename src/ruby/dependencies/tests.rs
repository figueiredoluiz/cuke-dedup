use super::*;

/// Resolves the given selected files under `root` against an injected `(modules, bytes)` budget.
fn resolve_with(root: &Path, selected: &[&str], budget: (usize, usize)) -> DiscoveredFiles {
    let config = Config::load(root, Default::default()).unwrap();
    let mut files = DiscoveredFiles {
        definitions: selected
            .iter()
            .map(|name| SourceFile {
                path: root.join(name),
                language: crate::ruby::RUBY,
            })
            .collect(),
        ..DiscoveredFiles::default()
    };
    resolve_graph(&config, &ExcludeSet::default(), &mut files, budget).unwrap();
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
    let root = canonical_platform_path(directory.path()).unwrap();
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
    let root = canonical_platform_path(directory.path()).unwrap();
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
    let root = canonical_platform_path(directory.path()).unwrap();
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
    let root = canonical_platform_path(directory.path()).unwrap();
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
    let root = canonical_platform_path(directory.path()).unwrap();
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
        language: crate::ruby::RUBY,
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
    resolve_graph(&config, &ExcludeSet::default(), &mut files, (3, usize::MAX)).unwrap();
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
    let root = canonical_platform_path(directory.path()).unwrap();
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
