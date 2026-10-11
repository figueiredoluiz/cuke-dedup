use assert_cmd::Command;
use cuke_dedup::config::{Config, ConfigOverrides};
use cuke_dedup::discovery::{SourceFile, SourceLanguage};
use cuke_dedup::source_adapter::{SourceExclusion, SOURCE_ADAPTER_REGISTRY};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

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

fn summary(stdout: &[u8]) -> Value {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| row.get("summary").is_some())
        .unwrap()
}

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

#[test]
fn source_exclusion_stays_with_javascript_adapter() {
    let project = tempfile::tempdir().unwrap();
    let js = project.path().join("steps.js");
    let ruby = project.path().join("steps.rb");
    fs::write(&js, b"const data = '\0';").unwrap();
    fs::write(&ruby, b"Given('step') { work() }\0").unwrap();
    let adapter = |language| {
        SOURCE_ADAPTER_REGISTRY
            .iter()
            .map(|entry| entry.adapter)
            .find(|adapter| adapter.language() == language)
            .unwrap()
    };
    let js_file = SourceFile {
        path: js,
        language: SourceLanguage::JavaScript,
    };
    let ruby_file = SourceFile {
        path: ruby,
        language: SourceLanguage::Ruby,
    };
    assert_eq!(
        adapter(SourceLanguage::JavaScript).inspect_source(&js_file, &[]),
        Some(SourceExclusion::Binary)
    );
    assert_eq!(
        adapter(SourceLanguage::Ruby).inspect_source(&ruby_file, &[]),
        None
    );
}

#[test]
fn ruby_configuration_keeps_flat_compatibility_key() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::load(root.path(), ConfigOverrides::default()).unwrap();
    assert!(config.ruby_load_paths().is_empty());
    assert!(serde_json::to_value(&config)
        .unwrap()
        .get("rubyLoadPaths")
        .is_none());
    fs::create_dir(root.path().join("lib")).unwrap();
    fs::write(
        root.path().join(".cuke-dedup.json"),
        r#"{"rubyLoadPaths":["lib"]}"#,
    )
    .unwrap();
    let config_with_load_path = Config::load(root.path(), ConfigOverrides::default()).unwrap();
    assert_eq!(
        config_with_load_path.ruby_load_paths(),
        &[PathBuf::from("lib")]
    );
    assert_eq!(
        serde_json::to_value(config_with_load_path).unwrap()["rubyLoadPaths"],
        serde_json::json!(["lib"])
    );
}
