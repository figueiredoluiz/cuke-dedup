use super::*;
use std::fs;

const CUCUMBER_DEFAULT: &str = "features/**/*.{feature,feature.md}";

/// Detects through the built-in adapters' framework contracts.
fn detect(root: &Path, package: Option<&JsonValue>) -> Result<Option<FrameworkFeatures>> {
    super::detect(AdapterRegistry::builtin(), root, package)
}

/// Checks Playwright-BDD and Cypress patterns lose only a leading `./`: Cucumber `paths`
/// normalization would expand the existing Cypress `specs` directory, and an unstripped `./`
/// would match no root-relative file.
#[test]
fn playwright_and_cypress_patterns_are_not_renormalized() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("specs")).unwrap();
    let config = directory.path().join("playwright.config.ts");
    fs::write(&config, "defineBddConfig({ featuresRoot: './specs' });").unwrap();
    let detected = detect(directory.path(), None).unwrap().unwrap();
    assert_eq!(detected.patterns, ["specs/**/*.feature"]);

    fs::remove_file(config).unwrap();
    fs::write(
        directory.path().join("cypress.config.js"),
        "export default defineConfig({ e2e: { specPattern: './specs' } });",
    )
    .unwrap();
    let package = serde_json::json!({
        "devDependencies": {"@badeball/cypress-cucumber-preprocessor": "1"}
    });
    let detected = detect(directory.path(), Some(&package)).unwrap().unwrap();
    assert_eq!(detected.framework, "Cypress Cucumber");
    assert_eq!(detected.patterns, ["specs"]);
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
        let detected = detect(directory.path(), None).unwrap().unwrap();
        assert_eq!(detected.patterns, expected, "{name}");
        assert_eq!(detected.warnings.len(), warnings, "{name}");
        fs::remove_file(path).unwrap();
    }
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

/// Checks every descriptor's configuration read is bounded: one byte over the limit is a fatal
/// input-limit error for structured, scripted and adapter files, and a file at the limit loads.
#[test]
fn framework_configuration_reads_are_input_limited() {
    let cypress = r#"{"devDependencies":{"cypress-cucumber-preprocessor":"1"}}"#;
    for (file, package) in [
        ("cucumber.yml", None),
        ("cucumber.js", None),
        ("cypress.config.ts", Some(cypress)),
    ] {
        for (size, fatal) in [(1024 * 1024, false), (1024 * 1024 + 1, true)] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(directory.path().join(file), " ".repeat(size)).unwrap();
            if let Some(package) = package {
                fs::write(directory.path().join("package.json"), package).unwrap();
            }
            let loaded = crate::config::Config::load(directory.path(), Default::default());
            assert_eq!(loaded.is_err(), fatal, "{file} at {size} bytes");
            // The root prints in its platform form (no verbatim `\\?\` prefix on Windows),
            // so only the kind, the file name and the limit are compared.
            if let Err(error) = loaded {
                let message = error.to_string();
                let tail = format!("{file} exceeds the 1048576-byte input limit");
                assert!(
                    message.starts_with("framework configuration ") && message.contains(&tail),
                    "{file}: {error:#}"
                );
            }
        }
    }
}
