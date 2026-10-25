use super::*;
use serde_json::Value as JsonValue;
use std::fs;

/// Checks framework directories expand to the appropriate native feature extensions.
#[test]
fn maps_framework_directories_to_their_native_defaults() {
    let mut warnings = Vec::new();
    assert_eq!(
        framework_pattern("features", "{feature,feature.md}", &mut warnings),
        Some("features/**/*.{feature,feature.md}".to_owned())
    );
    assert_eq!(
        framework_pattern("specs", "feature", &mut warnings),
        Some("specs/**/*.feature".to_owned())
    );
    assert_eq!(
        framework_pattern("./", "feature", &mut warnings),
        Some("**/*.feature".to_owned())
    );
}

/// Checks path normalization handles root directories, line suffixes, Windows separators, and unsupported negation.
#[test]
fn framework_pattern_normalization_is_explicit() {
    let mut warnings = Vec::new();
    for (input, expected) in [
        ("", None),
        ("././", Some("**/*.{feature,feature.md}")),
        (
            "./features/login.feature:42",
            Some("features/login.feature"),
        ),
        ("features/**/*.feature", Some("features/**/*.feature")),
        (
            "windows\\features",
            Some("windows/features/**/*.{feature,feature.md}"),
        ),
        ("!generated/**", None),
    ] {
        assert_eq!(
            framework_pattern(input, "{feature,feature.md}", &mut warnings).as_deref(),
            expected,
            "{input}"
        );
    }
    assert_eq!(warnings.len(), 1);
    assert_eq!(directory_pattern(".", "feature"), "**/*.feature");
    assert!(is_glob("features/{a,b}.feature"));
    assert!(!has_extension("features"));
    assert!(has_extension("features/a.feature"));
}

/// Checks JSON and YAML path extraction distinguishes missing values, literal strings, and mixed dynamic lists.
#[test]
fn structured_config_values_distinguish_missing_dynamic_and_strings() {
    assert_eq!(json_strings(None), LiteralProperty::Missing);
    assert_eq!(
        json_strings(Some(&JsonValue::String("a".to_owned()))),
        LiteralProperty::Strings(vec!["a".to_owned()])
    );
    assert_eq!(
        json_strings(Some(&serde_json::json!(["a", "b"]))),
        LiteralProperty::Strings(vec!["a".to_owned(), "b".to_owned()])
    );
    assert_eq!(
        json_strings(Some(&serde_json::json!(["a", 1]))),
        LiteralProperty::Dynamic
    );
    assert_eq!(
        cucumber_json_paths(r#"{"paths":"root.feature"}"#).unwrap(),
        LiteralProperty::Strings(vec!["root.feature".to_owned()])
    );
    assert_eq!(
        cucumber_yaml_paths("paths: dynamic").unwrap(),
        LiteralProperty::Strings(vec!["dynamic".to_owned()])
    );
    assert_eq!(
        cucumber_yaml_paths("paths: [one, 2]").unwrap(),
        LiteralProperty::Dynamic
    );
    assert_eq!(
        cucumber_yaml_paths("default: {}").unwrap(),
        LiteralProperty::Missing
    );
}

/// Checks a Ruby project's cucumber.yml is discovered with no scripted reader registered, while a
/// scripted cucumber.js is found only through the adapter's reader.
#[test]
fn structured_cucumber_discovery_needs_no_script_reader() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("cucumber.yml"),
        "default:\n  paths: [features/ruby]\n",
    )
    .unwrap();
    let bare = DiscoveryInput {
        root: directory.path(),
        package: None,
        script_readers: &[],
    };
    let found = (CUCUMBER.discover)(&bare).unwrap().unwrap();
    assert_eq!(found.patterns, ["features/ruby/**/*.{feature,feature.md}"]);
    assert_eq!(found.source, directory.path().join("cucumber.yml"));

    fs::remove_file(directory.path().join("cucumber.yml")).unwrap();
    fs::write(
        directory.path().join("cucumber.js"),
        "module.exports = { paths: ['js'] };",
    )
    .unwrap();
    assert!((CUCUMBER.discover)(&bare).unwrap().is_none());
    let stub = ScriptConfigReader {
        extensions: &["js"],
        read_property: |_, _, _| Ok(LiteralProperty::Strings(vec!["js".to_owned()])),
    };
    let scripted = DiscoveryInput {
        script_readers: &[stub],
        ..bare
    };
    assert_eq!(
        (CUCUMBER.discover)(&scripted).unwrap().unwrap().patterns,
        ["js/**/*.{feature,feature.md}"]
    );
}
