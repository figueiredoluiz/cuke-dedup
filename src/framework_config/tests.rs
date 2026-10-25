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

/// Checks framework directories expand to the appropriate native feature extensions.
#[test]
fn maps_framework_directories_to_their_native_defaults() {
    let mut warnings = Vec::new();
    assert_eq!(
        framework_pattern("features", FrameworkKind::Cucumber, &mut warnings),
        Some("features/**/*.{feature,feature.md}".to_owned())
    );
    assert_eq!(
        framework_pattern("specs", FrameworkKind::Playwright, &mut warnings),
        Some("specs/**/*.feature".to_owned())
    );
    assert_eq!(
        framework_pattern("./", FrameworkKind::Playwright, &mut warnings),
        Some("**/*.feature".to_owned())
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

/// Checks recognized framework files and current or legacy dependency names enable the expected discovery defaults.
#[test]
fn detects_supported_frameworks_and_dependency_fallbacks() {
    let playwright = tempfile::tempdir().unwrap();
    fs::write(
        playwright.path().join("playwright.config.ts"),
        "const dir = defineBddConfig({ featuresRoot: 'specs' });",
    )
    .unwrap();
    let detected = detect(playwright.path(), None).unwrap().unwrap();
    assert_eq!(detected.framework, "Playwright-BDD");
    assert_eq!(detected.patterns, ["specs/**/*.feature"]);

    let cypress = tempfile::tempdir().unwrap();
    fs::write(
        cypress.path().join("cypress.config.js"),
        "export default defineConfig({ e2e: { specPattern: 'specs/**/*.feature' } });",
    )
    .unwrap();
    let package = serde_json::json!({
        "devDependencies": {"@badeball/cypress-cucumber-preprocessor": "1"}
    });
    let detected = detect(cypress.path(), Some(&package)).unwrap().unwrap();
    assert_eq!(detected.framework, "Cypress Cucumber");
    assert_eq!(detected.patterns, ["specs/**/*.feature"]);

    let cucumber = tempfile::tempdir().unwrap();
    let package = serde_json::json!({
        "peerDependencies": {"@cucumber/cucumber": "1"}
    });
    let detected = detect(cucumber.path(), Some(&package)).unwrap().unwrap();
    assert_eq!(detected.patterns, [CUCUMBER_DEFAULT]);

    let legacy_cucumber = tempfile::tempdir().unwrap();
    let package = serde_json::json!({
        "devDependencies": {"cucumber": "6"}
    });
    let detected = detect(legacy_cucumber.path(), Some(&package))
        .unwrap()
        .unwrap();
    assert_eq!(detected.patterns, [CUCUMBER_DEFAULT]);

    let legacy_cypress = tempfile::tempdir().unwrap();
    fs::write(
        legacy_cypress.path().join("cypress.config.js"),
        "export default defineConfig({ e2e: { specPattern: 'legacy/**/*.feature' } });",
    )
    .unwrap();
    let package = serde_json::json!({
        "dependencies": {"cypress-cucumber-preprocessor": "4"}
    });
    let detected = detect(legacy_cypress.path(), Some(&package))
        .unwrap()
        .unwrap();
    assert_eq!(detected.framework, "Cypress Cucumber");
    assert_eq!(detected.patterns, ["legacy/**/*.feature"]);

    assert!(detect(tempfile::tempdir().unwrap().path(), None)
        .unwrap()
        .is_none());
}

/// Checks recognized Playwright-BDD configuration takes precedence over Cypress and Cucumber providers.
#[test]
fn registry_precedence_prefers_recognized_playwright_bdd_configuration() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("playwright.config.ts"),
        "defineBddConfig({ features: ['pw/**/*.feature'] });",
    )
    .unwrap();
    fs::write(
        directory.path().join("cypress.config.js"),
        "export default defineConfig({ e2e: { specPattern: 'cy/**/*.feature' } });",
    )
    .unwrap();
    fs::write(
        directory.path().join("cucumber.json"),
        r#"{"paths":["cu/**"]}"#,
    )
    .unwrap();
    let package = serde_json::json!({
        "dependencies": {
            "@badeball/cypress-cucumber-preprocessor": "1",
            "@cucumber/cucumber": "1"
        }
    });
    let detected = detect(directory.path(), Some(&package)).unwrap().unwrap();
    assert_eq!(detected.framework, "Playwright-BDD");
    assert_eq!(detected.patterns, ["pw/**/*.feature"]);
    assert_eq!(
        detected.source,
        directory.path().join("playwright.config.ts")
    );
}

/// Checks ordinary Playwright configuration falls through to eligible Cypress or Cucumber discovery.
#[test]
fn unrecognized_playwright_config_falls_through_to_lower_precedence_frameworks() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("playwright.config.ts"),
        "export default defineConfig({ testDir: 'tests' });",
    )
    .unwrap();
    fs::write(
        directory.path().join("cypress.config.js"),
        "export default defineConfig({ e2e: { specPattern: 'cy/**/*.feature' } });",
    )
    .unwrap();
    fs::write(
        directory.path().join("cucumber.json"),
        r#"{"paths":["cu/**"]}"#,
    )
    .unwrap();

    let cypress = serde_json::json!({
        "dependencies": {"@badeball/cypress-cucumber-preprocessor": "1"}
    });
    let detected = detect(directory.path(), Some(&cypress)).unwrap().unwrap();
    assert_eq!(detected.framework, "Cypress Cucumber");
    assert_eq!(detected.patterns, ["cy/**/*.feature"]);

    let cucumber = serde_json::json!({"dependencies": {"@cucumber/cucumber": "1"}});
    let detected = detect(directory.path(), Some(&cucumber)).unwrap().unwrap();
    assert_eq!(detected.framework, "Cucumber.js");
    assert_eq!(detected.patterns, ["cu/**"]);
    assert_eq!(detected.source, directory.path().join("cucumber.json"));
}

/// Checks Cypress configuration requires a supported Cucumber preprocessor dependency.
#[test]
fn cypress_configuration_requires_preprocessor_dependency_evidence() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("cypress.config.js"),
        "export default defineConfig({ e2e: { specPattern: 'cy/**/*.feature' } });",
    )
    .unwrap();
    assert!(detect(directory.path(), None).unwrap().is_none());
    let unrelated = serde_json::json!({"dependencies": {"cypress": "10"}});
    assert!(detect(directory.path(), Some(&unrelated))
        .unwrap()
        .is_none());
}

/// Checks lookalike dependency names do not authorize framework discovery.
#[test]
fn unsupported_dependency_names_do_not_enable_framework_discovery() {
    let directory = tempfile::tempdir().unwrap();
    // The Cypress configuration would match if the lookalike preprocessor name were
    // accepted, so the None assertion actually distinguishes rejection from gating on
    // missing files.
    fs::write(
        directory.path().join("cypress.config.js"),
        "export default defineConfig({ e2e: { specPattern: 'cy/**/*.feature' } });",
    )
    .unwrap();
    let lookalike = serde_json::json!({
        "dependencies": {
            "cucumber-js": "1",
            "@badeball/cypress-cucumber-preprocessor-legacy": "1"
        }
    });
    assert!(detect(directory.path(), Some(&lookalike))
        .unwrap()
        .is_none());
}

/// Checks an unrecognized earlier Playwright filename does not hide a recognized later file.
#[test]
fn same_provider_filename_fallthrough_continues_after_a_non_match() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("playwright.config.ts"),
        "export default defineConfig({ testDir: 'tests' });",
    )
    .unwrap();
    fs::write(
        directory.path().join("playwright.config.js"),
        "defineBddConfig({ features: ['js/**/*.feature'] });",
    )
    .unwrap();
    let detected = detect(directory.path(), None).unwrap().unwrap();
    assert_eq!(
        detected.framework, "Playwright-BDD",
        "an unrecognized earlier filename must not stop the provider"
    );
    assert_eq!(detected.patterns, ["js/**/*.feature"]);
    assert_eq!(
        detected.source,
        directory.path().join("playwright.config.js")
    );
}

/// Checks a Cucumber filename supplies defaults without literal paths while malformed JSON surfaces an error.
#[test]
fn cucumber_files_match_regardless_of_contents_and_errors_propagate() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("cucumber.cjs"), "const answer = 42;").unwrap();
    let detected = detect(directory.path(), None).unwrap().unwrap();
    assert_eq!(detected.framework, "Cucumber.js");
    assert_eq!(detected.patterns, [CUCUMBER_DEFAULT]);
    assert!(detected.warnings.is_empty());

    fs::write(directory.path().join("cucumber.json"), "not json").unwrap();
    let error = detect(directory.path(), None).unwrap_err();
    assert!(
        error.to_string().contains("cucumber.json"),
        "a malformed existing config must surface, not fall through: {error:#}"
    );
}

/// Checks Playwright and Cypress distinguish absent configuration from dynamic paths and framework defaults.
#[test]
fn framework_files_cover_missing_dynamic_and_default_paths() {
    let directory = tempfile::tempdir().unwrap();

    let playwright = directory.path().join("playwright.config.js");
    fs::write(&playwright, "export default { testDir: 'tests' }").unwrap();
    assert!(playwright_config(&playwright).unwrap().is_none());
    fs::write(
        &playwright,
        "const root = dynamic(); defineBddConfig({ featuresRoot: root });",
    )
    .unwrap();
    let dynamic = playwright_config(&playwright).unwrap().unwrap();
    assert_eq!(dynamic.patterns, ["**/*.{feature,feature.md}"]);
    assert_eq!(dynamic.warnings.len(), 1);
    fs::write(&playwright, "defineBddConfig({});").unwrap();
    assert_eq!(
        playwright_config(&playwright).unwrap().unwrap().patterns,
        ["features/**/*.feature"]
    );
    fs::write(&playwright, "defineBddConfig({ features: dynamic() });").unwrap();
    assert_eq!(
        playwright_config(&playwright)
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

/// Checks JSON, YAML, and JavaScript Cucumber formats preserve literals and warn only for dynamic paths.
#[test]
fn cucumber_file_formats_cover_literal_dynamic_and_missing_paths() {
    let directory = tempfile::tempdir().unwrap();
    for (name, source, expected, warnings) in [
        (
            "cucumber.json",
            r#"{"default":{"paths":["features/a.feature","features/b.feature.md"]}}"#,
            vec!["features/a.feature", "features/b.feature.md"],
            0,
        ),
        (
            "cucumber.yaml",
            "default:\n  paths:\n    - specs\n",
            vec!["specs/**/*.{feature,feature.md}"],
            0,
        ),
        (
            "cucumber.js",
            "export default { paths: dynamic() };",
            vec![CUCUMBER_DEFAULT],
            1,
        ),
        (
            "cucumber.cjs",
            "module.exports = {};",
            vec![CUCUMBER_DEFAULT],
            0,
        ),
    ] {
        let path = directory.path().join(name);
        fs::write(&path, source).unwrap();
        let detected = cucumber_config(&path).unwrap();
        assert_eq!(detected.patterns, expected, "{name}");
        assert_eq!(detected.warnings.len(), warnings, "{name}");
        fs::remove_file(path).unwrap();
    }
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
            framework_pattern(input, FrameworkKind::Cucumber, &mut warnings).as_deref(),
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

/// Pins the exact discovery origin and warning texts each framework configuration form produces,
/// with literal-path controls that produce no warning. Rows pin today's behaviour, not a target.
#[test]
fn discovery_origins_and_warning_texts_are_exact() {
    let cypress = r#"{"devDependencies":{"@badeball/cypress-cucumber-preprocessor":"1"}}"#;
    let cucumber = r#"{"devDependencies":{"@cucumber/cucumber":"1"}}"#;
    let negated = concat!(
        "negated framework path `!skip/**` is not a discovery input; ",
        "use `exclude` in .cuke-dedup.json"
    );
    let deterministic = |what: &str| {
        format!(
            "{{path}} uses {what}; configure `features` in .cuke-dedup.json to make discovery deterministic"
        )
    };
    let cucumber_dynamic = concat!(
        "{path} uses dynamic Cucumber paths; ",
        "falling back to Cucumber's default `features/**/*.{feature,feature.md}`"
    );
    let malformed = concat!(
        "could not inspect framework configuration (failed to parse framework config {path}: ",
        "expected ident at line 1 column 2); using built-in feature patterns"
    );
    let cy = "Cypress Cucumber configuration cypress.config.ts";
    let pw = "Playwright-BDD configuration playwright.config.ts";
    for (file, source, package, origin, warning) in [
        (
            "cypress.config.ts",
            "defineConfig({ e2e: { specPattern: p } });",
            Some(cypress),
            cy,
            Some(deterministic("a dynamic Cypress e2e.specPattern")),
        ),
        (
            "cypress.config.ts",
            "defineConfig({ e2e: { specPattern: 'cy/*.feature' } });",
            Some(cypress),
            cy,
            None,
        ),
        (
            "playwright.config.ts",
            "defineBddConfig({ features: f() });",
            None,
            pw,
            Some(deterministic("dynamic Playwright-BDD feature paths")),
        ),
        (
            "playwright.config.ts",
            "defineBddConfig({ featuresRoot: r });",
            None,
            pw,
            Some(deterministic("a dynamic Playwright-BDD featuresRoot")),
        ),
        (
            "playwright.config.ts",
            "defineBddConfig({ features: ['pw', '!skip/**'] });",
            None,
            pw,
            Some(negated.to_owned()),
        ),
        (
            "playwright.config.ts",
            "defineBddConfig({ features: ['pw'] });",
            None,
            pw,
            None,
        ),
        (
            "cucumber.mjs",
            "export default { paths: p };",
            None,
            "Cucumber.js configuration cucumber.mjs",
            Some(cucumber_dynamic.to_owned()),
        ),
        (
            "cucumber.yml",
            "default:\n  paths: {a: 1}\n",
            None,
            "Cucumber.js configuration cucumber.yml",
            Some(cucumber_dynamic.to_owned()),
        ),
        (
            "cucumber.yml",
            "default:\n  paths: [specs]\n",
            None,
            "Cucumber.js configuration cucumber.yml",
            None,
        ),
        (
            "cucumber.json",
            r#"{"paths":["cu","!skip/**"]}"#,
            None,
            "Cucumber.js configuration cucumber.json",
            Some(negated.to_owned()),
        ),
        (
            "cucumber.json",
            "not json",
            None,
            "built-in defaults",
            Some(malformed.to_owned()),
        ),
        (
            "unrelated.txt",
            "",
            Some(cucumber),
            "Cucumber.js configuration package.json",
            None,
        ),
        ("unrelated.txt", "", None, "built-in defaults", None),
    ] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(file), source).unwrap();
        if let Some(package) = package {
            fs::write(directory.path().join("package.json"), package).unwrap();
        }
        let config = crate::config::Config::load(directory.path(), Default::default()).unwrap();
        let path = config.root.join(file).display().to_string();
        let expected: Vec<String> = warning
            .map(|text| text.replace("{path}", &path))
            .into_iter()
            .collect();
        assert_eq!(config.feature_pattern_origin, origin, "{file}: {source}");
        assert_eq!(config.config_warnings, expected, "{file}: {source}");
    }
}

/// Checks every filename of each framework in precedence order: with all present the first is
/// inspected and its own paths decide the patterns, and removing it promotes the next.
#[test]
fn every_config_filename_keeps_its_precedence() {
    let cypress = serde_json::json!({"devDependencies": {"cypress-cucumber-preprocessor": "1"}});
    let scripted = |stem: &str| {
        ["ts", "js", "mts", "mjs", "cts", "cjs"].map(|extension| format!("{stem}.{extension}"))
    };
    let cucumber = [
        "cucumber.json",
        "cucumber.yaml",
        "cucumber.yml",
        "cucumber.js",
        "cucumber.cjs",
        "cucumber.mjs",
        "cucumber.ts",
        "cucumber.cts",
        "cucumber.mts",
    ]
    .map(String::from);
    /// Writes configuration naming `dir` and returns the pattern discovery must report for it.
    type Case = fn(&str, &str) -> (String, String);
    let cucumber_case: Case = |name, dir| {
        let source = match name.rsplit('.').next() {
            Some("json") => format!(r#"{{"paths":["{dir}"]}}"#),
            Some("yaml" | "yml") => format!("paths: [{dir}]"),
            _ => format!("module.exports = {{ paths: ['{dir}'] }};"),
        };
        (source, format!("{dir}/**/*.{{feature,feature.md}}"))
    };
    let playwright_case: Case = |_, dir| {
        let source = format!("defineBddConfig({{ features: ['{dir}'] }});");
        (source, format!("{dir}/**/*.feature"))
    };
    let cypress_case: Case = |_, dir| {
        let source = format!("defineConfig({{ e2e: {{ specPattern: '{dir}/*.feature' }} }});");
        (source, format!("{dir}/*.feature"))
    };
    for (names, case, package) in [
        (cucumber.to_vec(), cucumber_case, None),
        (
            scripted("playwright.config").to_vec(),
            playwright_case,
            None,
        ),
        (
            scripted("cypress.config").to_vec(),
            cypress_case,
            Some(&cypress),
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let dir = |name: &str| name.replace('.', "_");
        for name in &names {
            fs::write(directory.path().join(name), case(name, &dir(name)).0).unwrap();
        }
        for name in &names {
            let detected = detect(directory.path(), package).unwrap().unwrap();
            assert_eq!(detected.source, directory.path().join(name));
            assert_eq!(detected.patterns, [case(name, &dir(name)).1]);
            fs::remove_file(directory.path().join(name)).unwrap();
        }
        assert!(detect(directory.path(), package).unwrap().is_none());
    }
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
