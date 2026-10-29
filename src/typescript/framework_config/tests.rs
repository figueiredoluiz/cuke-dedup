use super::*;
use std::fs;

/// Checks static Cucumber path extraction selects the default profile instead of other profiles.
#[test]
fn extracts_cucumber_default_profile_paths() {
    let source = "export default { default: { paths: ['specs', 'acceptance/**/*.feature.md'] }, ci: { paths: ['wrong'] } }";
    assert_eq!(
        static_object_property(source, "js", None, "paths").unwrap(),
        LiteralProperty::Strings(vec![
            "specs".to_owned(),
            "acceptance/**/*.feature.md".to_owned()
        ])
    );
}

/// Checks Playwright-BDD feature extraction ignores unrelated object properties.
#[test]
fn extracts_only_define_bdd_config_features() {
    let source = "const unrelated = { features: ['wrong'] }; const testDir = defineBddConfig({ features: ['tests/**/*.spec'] });";
    assert_eq!(
        static_object_property(source, "ts", Some("defineBddConfig"), "features").unwrap(),
        LiteralProperty::Strings(vec!["tests/**/*.spec".to_owned()])
    );
}

/// Checks Cypress feature patterns come from e2e configuration rather than component settings.
#[test]
fn extracts_cypress_e2e_spec_pattern_from_project_config() {
    let source = r#"
        export default defineConfig({
            component: { specPattern: '**/*.cy.ts' },
            e2e: { specPattern: ['cypress/e2e/**/*.feature', 'acceptance/**/*.feature.md'] }
        });
    "#;
    assert_eq!(
        static_object_property_path(source, "ts", Some("defineConfig"), &["e2e", "specPattern"])
            .unwrap(),
        LiteralProperty::Strings(vec![
            "cypress/e2e/**/*.feature".to_owned(),
            "acceptance/**/*.feature.md".to_owned()
        ])
    );
}

/// Checks Playwright and Cypress distinguish absent configuration from dynamic paths and framework defaults, and that a feature path with an extension expands only when it names an existing directory.
#[test]
fn framework_files_cover_missing_dynamic_and_default_paths() {
    let directory = tempfile::tempdir().unwrap();

    let playwright = directory.path().join("playwright.config.js");
    fs::write(&playwright, "export default { testDir: 'tests' }").unwrap();
    assert!(playwright_config(directory.path(), &playwright)
        .unwrap()
        .is_none());
    fs::write(
        &playwright,
        "const root = dynamic(); defineBddConfig({ featuresRoot: root });",
    )
    .unwrap();
    let dynamic = playwright_config(directory.path(), &playwright)
        .unwrap()
        .unwrap();
    assert_eq!(dynamic.patterns, ["**/*.{feature,feature.md}"]);
    assert_eq!(dynamic.warnings.len(), 1);
    fs::write(&playwright, "defineBddConfig({});").unwrap();
    assert_eq!(
        playwright_config(directory.path(), &playwright)
            .unwrap()
            .unwrap()
            .patterns,
        ["features/**/*.feature"]
    );
    fs::create_dir_all(directory.path().join("specs.v2")).unwrap();
    fs::write(
        &playwright,
        "defineBddConfig({ features: ['specs.v2', 'absent.v2'] });",
    )
    .unwrap();
    assert_eq!(
        playwright_config(directory.path(), &playwright)
            .unwrap()
            .unwrap()
            .patterns,
        ["specs.v2/**/*.feature", "absent.v2"]
    );
    fs::write(&playwright, "defineBddConfig({ features: dynamic() });").unwrap();
    assert_eq!(
        playwright_config(directory.path(), &playwright)
            .unwrap()
            .unwrap()
            .warnings
            .len(),
        1
    );

    let cypress = directory.path().join("cypress.config.ts");
    fs::write(&cypress, "defineConfig({ component: {} });").unwrap();
    assert!(cypress_config(&cypress).unwrap().is_none());
    fs::write(
        &cypress,
        "const pattern = dynamic(); defineConfig({ e2e: { specPattern: pattern } });",
    )
    .unwrap();
    let dynamic = cypress_config(&cypress).unwrap().unwrap();
    assert_eq!(dynamic.patterns, ["**/*.{feature,feature.md}"]);
    assert_eq!(dynamic.warnings.len(), 1);
}

/// Checks static object queries distinguish absent and dynamic paths and decode supported quoted strings.
#[test]
fn static_object_queries_cover_absent_and_dynamic_shapes() {
    assert_eq!(
        static_object_property("const value = {};", "js", Some("missing"), "paths").unwrap(),
        LiteralProperty::Missing
    );
    assert_eq!(
        static_object_property("export default { paths: 42 };", "js", None, "paths").unwrap(),
        LiteralProperty::Dynamic
    );
    assert_eq!(
        static_object_property_path(
            "defineConfig({ e2e: dynamic() })",
            "js",
            Some("defineConfig"),
            &["e2e", "specPattern"]
        )
        .unwrap(),
        LiteralProperty::Dynamic
    );
    assert_eq!(
        static_object_property_path(
            "defineConfig({ e2e: {} })",
            "js",
            Some("defineConfig"),
            &["e2e", "specPattern"]
        )
        .unwrap(),
        LiteralProperty::Missing
    );
    assert_eq!(
        static_object_property_path("const x = {};", "js", None, &[]).unwrap(),
        LiteralProperty::Missing
    );
    assert_eq!(unquote("'a\\/b'"), Some("a/b".to_owned()));
    assert_eq!(unquote("dynamic"), None);
}

/// Checks object shapes the static reader skips or rejects: a non-object `default`, a call without
/// an object argument, spread and shorthand entries, and arrays with non-string items.
#[test]
fn static_object_reader_skips_unsupported_entries() {
    let strings = |values: &[&str]| {
        LiteralProperty::Strings(values.iter().map(|value| (*value).to_owned()).collect())
    };
    for (source, call, expected) in [
        (
            "export default { default: 'x', paths: ['a'] };",
            None,
            strings(&["a"]),
        ),
        (
            "export default { default: { paths: ['d'] }, paths: ['a'] };",
            None,
            strings(&["d"]),
        ),
        (
            "const base = {}; export default { ...base, base, paths: ['a'] };",
            None,
            strings(&["a"]),
        ),
        (
            "export default { paths: ['a', b] };",
            None,
            LiteralProperty::Dynamic,
        ),
        (
            "export default { paths: ['a', 'b'] };",
            None,
            strings(&["a", "b"]),
        ),
        (
            "defineBddConfig(options); defineBddConfig({ paths: ['a'] });",
            Some("defineBddConfig"),
            strings(&["a"]),
        ),
        (
            "defineBddConfig(options);",
            Some("defineBddConfig"),
            LiteralProperty::Missing,
        ),
    ] {
        assert_eq!(
            static_object_property(source, "js", call, "paths").unwrap(),
            expected,
            "{source}"
        );
    }
}
