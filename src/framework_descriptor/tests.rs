use super::*;
use serde_json::Value as JsonValue;
use std::fs;

/// Checks framework directories expand to the appropriate native feature extensions.
#[test]
fn maps_framework_directories_to_their_native_defaults() {
    let root = tempfile::tempdir().unwrap();
    let mut warnings = Vec::new();
    assert_eq!(
        framework_pattern(
            root.path(),
            "features",
            "{feature,feature.md}",
            &mut warnings
        ),
        Some("features/**/*.{feature,feature.md}".to_owned())
    );
    assert_eq!(
        framework_pattern(root.path(), "specs", "feature", &mut warnings),
        Some("specs/**/*.feature".to_owned())
    );
    assert_eq!(
        framework_pattern(root.path(), "./", "feature", &mut warnings),
        Some("**/*.feature".to_owned())
    );
}

/// Checks path normalization handles root directories, line suffixes, Windows separators, and unsupported negation.
#[test]
fn framework_pattern_normalization_is_explicit() {
    let root = tempfile::tempdir().unwrap();
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
            framework_pattern(root.path(), input, "{feature,feature.md}", &mut warnings).as_deref(),
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

/// Checks an existing path is a file or a directory as it is on disk, whatever its extension, and
/// a missing one is a directory only when it ends in `/` or has no extension. Each row runs
/// against a root holding the path and an empty root.
#[test]
fn configured_paths_follow_the_disk_before_the_extension() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("specs.v2")).unwrap();
    fs::create_dir_all(root.path().join("named.feature")).unwrap();
    fs::write(root.path().join("specs.v2/login.feature"), "").unwrap();
    fs::write(root.path().join("specs.v2/smoke"), "").unwrap();
    let expand = |value: &str, root: &Path| {
        framework_pattern(root, value, "{feature,feature.md}", &mut Vec::new())
    };
    let absent = root.path().join("absent");
    for (input, present, missing) in [
        ("specs.v2", "specs.v2/**/*.{feature,feature.md}", "specs.v2"),
        (
            "named.feature",
            "named.feature/**/*.{feature,feature.md}",
            "named.feature",
        ),
        (
            "specs.v2/smoke",
            "specs.v2/smoke",
            "specs.v2/smoke/**/*.{feature,feature.md}",
        ),
        (
            "./specs.v2/",
            "specs.v2/**/*.{feature,feature.md}",
            "specs.v2/**/*.{feature,feature.md}",
        ),
        (
            "specs.v2/login.feature:3:5",
            "specs.v2/login.feature",
            "specs.v2/login.feature",
        ),
    ] {
        assert_eq!(
            expand(input, root.path()).as_deref(),
            Some(present),
            "{input}"
        );
        assert_eq!(expand(input, &absent).as_deref(), Some(missing), "{input}");
    }
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
/// scripted cucumber.js is found only through the adapter's reader. Paths come back as written;
/// `detect` normalizes them.
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
    };
    let found = cucumber_discovery(&bare, &[]).unwrap().unwrap();
    assert_eq!(found.patterns, ["features/ruby"]);
    assert_eq!(found.source, directory.path().join("cucumber.yml"));

    fs::remove_file(directory.path().join("cucumber.yml")).unwrap();
    fs::write(
        directory.path().join("cucumber.js"),
        "module.exports = { paths: ['js'] };",
    )
    .unwrap();
    assert!(cucumber_discovery(&bare, &[]).unwrap().is_none());
    let stub = ScriptConfigReader {
        extensions: &["js"],
        read_property: |_, _, _| Ok(LiteralProperty::Strings(vec!["js".to_owned()])),
    };
    assert_eq!(
        cucumber_discovery(&bare, &[stub])
            .unwrap()
            .unwrap()
            .patterns,
        ["js"]
    );
}

/// Checks a verbatim descriptor only drops a leading `./`, where a normalizing descriptor with the
/// same discovery expands the existing directory.
#[test]
fn verbatim_descriptors_only_drop_the_dot_slash_prefix() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("specs")).unwrap();
    let discover: FeatureDiscovery = |input| {
        Ok(Some(DiscoveredFeatures::new(
            vec!["./specs".to_owned()],
            input.root.to_path_buf(),
        )))
    };
    let input = DiscoveryInput::new(root.path(), None);
    let patterns =
        |descriptor: FrameworkDescriptor| detect([&descriptor], &input).unwrap().unwrap().patterns;
    assert_eq!(
        patterns(FrameworkDescriptor::verbatim("Verbatim", discover)),
        ["specs"]
    );
    assert_eq!(
        patterns(FrameworkDescriptor::new("Normalized", discover)),
        ["specs/**/*.{feature,feature.md}"]
    );
}
