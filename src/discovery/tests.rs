use super::*;
use crate::config::ConfigOverrides;
use std::fs;

/// Default discovery finds one feature and one definition, skipping a source in a
/// `.gitignore`d directory.
#[test]
fn discovers_sources_and_features_but_uses_gitignore() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("features/steps")).unwrap();
    fs::create_dir_all(directory.path().join("ignored")).unwrap();
    fs::write(directory.path().join(".gitignore"), "ignored/\n").unwrap();
    fs::write(
        directory.path().join("features/example.feature"),
        "Feature: x",
    )
    .unwrap();
    fs::write(
        directory.path().join("features/steps/example.ts"),
        "Given('x', () => {})",
    )
    .unwrap();
    fs::write(
        directory.path().join("ignored/step.ts"),
        "Given('y', () => {})",
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();

    let files = discover(&config).unwrap();
    assert_eq!(files.features.len(), 1);
    assert_eq!(files.definitions.len(), 1);
}

/// `.cuke-dedupignore` excludes by directory and glob, `!` re-includes, and a nested file's
/// rules apply only below its own directory.
#[test]
fn cuke_dedup_ignore_supports_scoped_gitignore_rules_and_negation() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("ignored")).unwrap();
    fs::create_dir_all(directory.path().join("packages/first")).unwrap();
    fs::create_dir_all(directory.path().join("packages/second")).unwrap();
    fs::write(
        directory.path().join(".cuke-dedupignore"),
        "ignored/\n*.generated.ts\n!keep.generated.ts\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("packages/first/.cuke-dedupignore"),
        "local.ts\n",
    )
    .unwrap();
    for path in [
        "included.ts",
        "ignored/step.ts",
        "drop.generated.ts",
        "keep.generated.ts",
        "packages/first/local.ts",
        "packages/second/local.ts",
    ] {
        fs::write(directory.path().join(path), "Given('x', () => work())").unwrap();
    }

    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let root = config.root.clone();
    let files = discover(&config).unwrap();
    let relative = files
        .definitions
        .iter()
        .map(|file| {
            file.path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();

    assert_eq!(
        relative,
        vec![
            "included.ts",
            "keep.generated.ts",
            "packages/second/local.ts"
        ]
    );
}

/// `.feature` and `.feature.md` files are discovered with their respective formats, and no
/// feature pattern is reported unmatched.
#[test]
fn discovers_classic_and_markdown_features_with_parser_metadata() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("features")).unwrap();
    fs::write(directory.path().join("features/a.feature"), "Feature: A").unwrap();
    fs::write(
        directory.path().join("features/b.feature.md"),
        "# Feature: B",
    )
    .unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();

    let files = discover(&config).unwrap();
    assert_eq!(files.features.len(), 2);
    assert_eq!(files.features[0].format, FeatureFormat::Gherkin);
    assert_eq!(files.features[1].format, FeatureFormat::GherkinMarkdown);
    assert!(files.unmatched_feature_patterns.is_empty());
}

/// A file matched by two feature patterns leaves neither reported as unmatched.
#[test]
fn overlapping_feature_patterns_are_both_counted_as_matched() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("features")).unwrap();
    fs::write(directory.path().join("features/a.feature"), "Feature: A").unwrap();
    let config = Config::load(
        directory.path(),
        ConfigOverrides {
            features: Some(vec![
                "**/*.feature".to_owned(),
                "features/**/*.feature".to_owned(),
            ]),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();

    let files = discover(&config).unwrap();
    assert!(files.unmatched_feature_patterns.is_empty());
}

/// A configured definition glob selects only the source it matches and is not reported
/// unmatched.
#[test]
fn configured_definition_globs_narrow_source_files() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("steps")).unwrap();
    fs::write(directory.path().join("steps/a.ts"), "").unwrap();
    fs::write(directory.path().join("other.ts"), "").unwrap();
    let overrides = ConfigOverrides {
        definitions: Some(vec!["steps/**/*.ts".to_owned()]),
        ..ConfigOverrides::default()
    };
    let config = Config::load(directory.path(), overrides).unwrap();

    let files = discover(&config).unwrap();
    assert_eq!(files.definitions.len(), 1);
    assert!(files.definitions[0].path.ends_with("steps/a.ts"));
    assert!(files.unmatched_definition_patterns.is_empty());
}

/// Compiled globs let `*` cross path separators and expand `{a,b}` braces, and still reject a
/// path whose name matches no alternative.
#[test]
fn configured_globs_use_documented_cross_separator_and_brace_semantics() {
    let globs = compile_globs(
        &[
            "*.ts".to_owned(),
            "features/*.{feature,feature.md}".to_owned(),
        ],
        "test",
    )
    .unwrap();
    assert!(globs.is_match(Path::new("nested/deep/steps.ts")));
    assert!(globs.is_match(Path::new("features/nested/example.feature.md")));
    assert!(!globs.is_match(Path::new("features/example.md")));
}

/// A definition glob that matches no file selects no definitions and is reported unmatched by
/// its original text.
#[test]
fn reports_each_unmatched_definition_glob() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("steps.ts"),
        "Given('x', () => work())",
    )
    .unwrap();
    let config = Config::load(
        directory.path(),
        ConfigOverrides {
            definitions: Some(vec!["stpes/**/*.ts".to_owned()]),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();

    let files = discover(&config).unwrap();
    assert!(files.definitions.is_empty());
    assert_eq!(files.unmatched_definition_patterns, ["stpes/**/*.ts"]);
}

/// `.d.ts`, `.d.mts`, `.d.cts` and `.js.map` paths map to no source language.
#[test]
fn declaration_and_source_map_files_are_not_definition_sources() {
    for path in ["types.d.ts", "types.d.mts", "types.d.cts", "code.js.map"] {
        assert_eq!(
            crate::source_adapter::language_for_path(Path::new(path)),
            None,
            "{path}"
        );
    }
}

/// A source under `node_modules` is excluded by default and selected once default excludes are
/// disabled and a definition glob names it.
#[test]
fn built_in_excludes_prune_sources_unless_explicitly_disabled() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("node_modules/workspace")).unwrap();
    fs::write(
        directory.path().join("node_modules/workspace/steps.ts"),
        "Given('nested', () => work())",
    )
    .unwrap();

    let default_config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert!(discover(&default_config).unwrap().definitions.is_empty());

    let enabled_config = Config::load(
        directory.path(),
        ConfigOverrides {
            definitions: Some(vec!["node_modules/workspace/**/*.ts".to_owned()]),
            exclude_defaults: Some(false),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    let files = discover(&enabled_config).unwrap();
    assert_eq!(files.definitions.len(), 1);
}

/// A source in a hidden directory is skipped by default and found with `include_hidden` and a
/// matching definition glob.
#[test]
fn hidden_sources_require_the_include_hidden_option() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join(".steps")).unwrap();
    fs::write(
        directory.path().join(".steps/example.ts"),
        "Given('hidden', () => work())",
    )
    .unwrap();
    let default_config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    assert!(discover(&default_config).unwrap().definitions.is_empty());

    let visible_config = Config::load(
        directory.path(),
        ConfigOverrides {
            definitions: Some(vec![".steps/**/*.ts".to_owned()]),
            include_hidden: Some(true),
            ..ConfigOverrides::default()
        },
    )
    .unwrap();
    assert_eq!(discover(&visible_config).unwrap().definitions.len(), 1);
}

/// An unreadable directory records a traversal error while the readable source is still
/// discovered. Unix only.
#[cfg(unix)]
#[test]
fn traversal_errors_are_collected_without_discarding_other_files() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("visible.ts"),
        "Given('visible', () => work())",
    )
    .unwrap();
    let locked = directory.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("hidden.ts"), "Given('hidden', () => work())").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let files = discover(&config).unwrap();

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(files.definitions.len(), 1);
    assert!(!files.errors.is_empty());
}

/// An unreadable directory under excluded `node_modules` is never entered, so discovery records
/// no traversal error. Unix only.
#[cfg(unix)]
#[test]
fn excluded_directories_are_pruned_before_traversal() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let locked = directory.path().join("node_modules/locked");
    fs::create_dir_all(&locked).unwrap();
    fs::write(locked.join("step.ts"), "Given('hidden', () => work())").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let files = discover(&config).unwrap();

    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(files.definitions.is_empty());
    assert!(files.errors.is_empty());
}
