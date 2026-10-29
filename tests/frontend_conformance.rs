use assert_cmd::Command;
use cuke_dedup::config::{Config, ConfigOverrides};
use cuke_dedup::discovery::{SourceFile, SourceLanguage};
use cuke_dedup::ruby::RUBY;
use cuke_dedup::source_adapter::{AdapterRegistry, SourceExclusion};
use cuke_dedup::typescript::{JAVASCRIPT, TYPESCRIPT};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

/// Runs the analyzer with JSONL output and optional definition patterns, returning its captured process output.
fn run(root: &std::path::Path, definitions: Option<&str>, extra: &[&str]) -> std::process::Output {
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .arg(root)
        .args(["--reporters", "jsonl", "--no-metrics"]);
    if let Some(pattern) = definitions {
        command.args(["--definitions", pattern]);
    }
    command.args(extra).output().unwrap()
}

/// Parses JSONL output and returns the summary record, panicking if records are invalid or the summary is absent.
fn summary(stdout: &[u8]) -> Value {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| row.get("summary").is_some())
        .unwrap()
}

/// Checks Ruby opt-in, dependency discovery, unresolved inputs, and mixed-language rejection retain expected CLI outcomes.
#[test]
fn ruby_selection_dependencies_and_mixed_domain_keep_cli_outcomes() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("steps.js"),
        "Given('js', () => work());",
    )
    .unwrap();
    fs::write(project.path().join("steps.rb"), "Given('ruby') { work() }").unwrap();
    let automatic = run(project.path(), None, &["--require-definitions"]);
    assert_eq!(automatic.status.code(), Some(0));
    assert_eq!(
        summary(&automatic.stdout)["summary"]["definitionsAnalyzed"],
        1
    );

    fs::create_dir(project.path().join("lib")).unwrap();
    fs::write(
        project.path().join("entry.rb"),
        "require_relative 'lib/support'",
    )
    .unwrap();
    fs::write(
        project.path().join("lib/support.rb"),
        "Given('from dependency') { work() }",
    )
    .unwrap();
    let explicit = run(project.path(), Some("entry.rb"), &["--require-definitions"]);
    assert_eq!(explicit.status.code(), Some(0));
    assert_eq!(
        summary(&explicit.stdout)["summary"]["definitionsAnalyzed"],
        1
    );

    for (source, excluded) in [
        ("require_relative 'missing'", false),
        ("require_relative 'lib/support'", true),
    ] {
        fs::write(project.path().join("entry.rb"), source).unwrap();
        let mut args = vec!["--fail-on-incomplete"];
        if excluded {
            args.extend(["--exclude", "lib/**"]);
        }
        let unresolved = run(project.path(), Some("entry.rb"), &args);
        assert_eq!(unresolved.status.code(), Some(2));
        assert_eq!(summary(&unresolved.stdout)["corpus"]["incomplete"], true);
    }

    let mixed = run(project.path(), Some("steps.rb,steps.js"), &[]);
    assert_eq!(mixed.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&mixed.stderr).contains("separate analysis runs"));
}

/// Checks NUL-byte binary exclusion belongs to the JavaScript adapter and does not reject Ruby source.
#[test]
fn source_exclusion_stays_with_javascript_adapter() {
    let project = tempfile::tempdir().unwrap();
    let js = project.path().join("steps.js");
    let ruby = project.path().join("steps.rb");
    fs::write(&js, b"const data = '\0';").unwrap();
    fs::write(&ruby, b"Given('step') { work() }\0").unwrap();
    let adapter = |language| {
        AdapterRegistry::builtin()
            .registrations()
            .iter()
            .map(|entry| entry.adapter)
            .find(|adapter| adapter.language() == language)
            .unwrap()
    };
    let js_file = SourceFile {
        path: js,
        language: JAVASCRIPT,
    };
    let ruby_file = SourceFile {
        path: ruby,
        language: RUBY,
    };
    assert_eq!(
        adapter(JAVASCRIPT).inspect_source(&js_file, &[]),
        Some(SourceExclusion::Binary)
    );
    assert_eq!(adapter(RUBY).inspect_source(&ruby_file, &[]), None);
}

/// Checks both Ruby load-path forms (`ruby.loadPaths` and the flat `rubyLoadPaths` alias) in a
/// standalone file and in `package.json`, the one-form-per-source rule, section validation,
/// top-level unknown keys, and that the effective config still serializes the flat key only when
/// set.
#[test]
fn ruby_configuration_keeps_flat_compatibility_key() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::load(root.path(), ConfigOverrides::default()).unwrap();
    assert!(config.ruby_load_paths().is_empty());
    assert!(serde_json::to_value(&config)
        .unwrap()
        .get("rubyLoadPaths")
        .is_none());

    // (package.json `cukeDedup`, standalone `.cuke-dedup.json`) -> load paths. The standalone file
    // is read by `from_str` and the package by `from_value`; both go through the flattened keys.
    let lib = || vec![PathBuf::from("lib")];
    for (package, standalone, expected) in [
        (None, Some(r#"{"rubyLoadPaths":["lib"]}"#), lib()),
        (None, Some(r#"{"ruby":{"loadPaths":["lib"]}}"#), lib()),
        (Some(r#"{"ruby":{"loadPaths":["lib"]}}"#), None, lib()),
        (Some(r#"{"rubyLoadPaths":["lib"]}"#), None, lib()),
        // A section without `loadPaths`, or a null one, sets nothing.
        (None, Some(r#"{"ruby":{}}"#), Vec::new()),
        (None, Some(r#"{"ruby":null}"#), Vec::new()),
    ] {
        let root = tempfile::tempdir().unwrap();
        if let Some(package) = package {
            fs::write(
                root.path().join("package.json"),
                format!(r#"{{"cukeDedup":{package}}}"#),
            )
            .unwrap();
        }
        if let Some(standalone) = standalone {
            fs::write(root.path().join(".cuke-dedup.json"), standalone).unwrap();
        }
        let config = Config::load(root.path(), ConfigOverrides::default())
            .unwrap_or_else(|error| panic!("{package:?} {standalone:?}: {error:#}"));
        // An auto-discovered file that fails to parse only warns and falls back to defaults, which
        // would pass the empty rows; a parsed source leaves no warning.
        assert!(
            config.config_warnings.is_empty(),
            "{standalone:?}: {:?}",
            config.config_warnings
        );
        assert_eq!(
            config.ruby_load_paths(),
            expected.as_slice(),
            "{package:?} {standalone:?}"
        );
        let printed = serde_json::to_value(&config).unwrap();
        assert!(
            printed.get("ruby").is_none(),
            "only the flat key is printed"
        );
        assert_eq!(
            printed.get("rubyLoadPaths").is_some(),
            !expected.is_empty(),
            "{package:?} {standalone:?}"
        );
    }

    // Rejected sources, each with the error text that names the cause. Errors go through
    // `--config` so an invalid file fails the run instead of falling back with a warning.
    for (source, error) in [
        (
            r#"{"rubyLoadPaths":["lib"],"ruby":{"loadPaths":["lib"]}}"#,
            // The file is named as written or relative to the root, depending on how the root resolves.
            "config.json: set Ruby load paths with `ruby.loadPaths` or `rubyLoadPaths`, not both",
        ),
        (
            r#"{"ruby":{"loadpaths":["lib"]}}"#,
            "unknown field `loadpaths`",
        ),
        (r#"{"ruby":["lib"]}"#, "invalid type"),
        (
            r#"{"rubyLoadPath":["lib"]}"#,
            "unknown field `rubyLoadPath`",
        ),
        // A core key error keeps its position although the frontend keys are flattened in.
        ("{\n\"threshold\": \"x\"}", "line 2 column"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        fs::write(&path, source).unwrap();
        let overrides = ConfigOverrides {
            config_file: Some(path),
            ..ConfigOverrides::default()
        };
        let message = format!(
            "{:#}",
            Config::load(root.path(), overrides).expect_err(source)
        );
        assert!(message.contains(error), "{source}: {message}");
    }
    // The conflict names `package.json` when that is the source.
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("package.json"),
        r#"{"cukeDedup":{"rubyLoadPaths":["lib"],"ruby":{"loadPaths":["lib"]}}}"#,
    )
    .unwrap();
    let message = format!(
        "{:#}",
        Config::load(root.path(), ConfigOverrides::default()).unwrap_err()
    );
    assert!(
        message.starts_with("invalid configuration in package.json#cukeDedup: "),
        "{message}"
    );
}

/// Whether the handler of the only definition `source` registers counts as a stub.
fn handler_is_stub(language: SourceLanguage, path: &str, source: &str) -> bool {
    let adapter = AdapterRegistry::builtin()
        .registrations()
        .iter()
        .map(|entry| entry.adapter)
        .find(|adapter| adapter.language() == language)
        .unwrap();
    let file = SourceFile {
        path: path.into(),
        language,
    };
    let mut extraction = adapter.extract(source, &file).unwrap();
    assert_eq!(extraction.definitions.len(), 1, "{source}");
    extraction.definitions.remove(0).handler.trivial
}

/// Both adapters apply one stub vocabulary: a spelling that marks a stub in one listed syntax marks
/// it in every listed syntax of both languages, and a near miss marks none. Each row is (Ruby identifier,
/// TypeScript identifier, string spelling, is a stub); the controls are the rows that are not.
#[test]
fn adapters_share_one_stub_vocabulary() {
    let rows = [
        ("pending", "pending", "PENDING", true),
        ("not_implemented", "notImplemented", "Not implemented", true),
        ("todo", "TODO", "to-do", true),
        ("pending_review", "pendingReview", "pending review", false),
        ("done", "done", "done", false),
    ];
    for (ruby, typescript, label, stub) in rows {
        for source in [
            format!("Given('a') {{ {ruby}() }}"),
            format!("Given('a') {{ raise \"{label}\" }}"),
        ] {
            assert_eq!(handler_is_stub(RUBY, "steps.rb", &source), stub, "{source}");
        }
        for source in [
            format!("Given('a', () => {typescript}());"),
            format!("Given('a', () => {{ throw new Error('{label}'); }});"),
            format!("Given('a', () => '{label}');"),
        ] {
            assert_eq!(
                handler_is_stub(TYPESCRIPT, "steps.ts", &source),
                stub,
                "{source}"
            );
        }
    }
}
