//! Safe, static feature-path discovery from supported framework configuration files.

use crate::resource_limits::{read_utf8, MAX_CONFIG_INPUT_BYTES};
use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser};
use yaml_serde::Value as YamlValue;

const CUCUMBER_CONFIGS: [&str; 9] = [
    "cucumber.json",
    "cucumber.yaml",
    "cucumber.yml",
    "cucumber.js",
    "cucumber.cjs",
    "cucumber.mjs",
    "cucumber.ts",
    "cucumber.cts",
    "cucumber.mts",
];
const PLAYWRIGHT_CONFIGS: [&str; 6] = [
    "playwright.config.ts",
    "playwright.config.js",
    "playwright.config.mts",
    "playwright.config.mjs",
    "playwright.config.cts",
    "playwright.config.cjs",
];
const CYPRESS_CONFIGS: [&str; 6] = [
    "cypress.config.ts",
    "cypress.config.js",
    "cypress.config.mts",
    "cypress.config.mjs",
    "cypress.config.cts",
    "cypress.config.cjs",
];
const CUCUMBER_DEFAULT: &str = "features/**/*.{feature,feature.md}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameworkFeatures {
    pub(crate) patterns: Vec<String>,
    pub(crate) source: PathBuf,
    pub(crate) framework: &'static str,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn detect(
    root: &Path,
    package: Option<&JsonValue>,
) -> Result<Option<FrameworkFeatures>> {
    for name in PLAYWRIGHT_CONFIGS {
        let path = root.join(name);
        if !path.is_file() {
            continue;
        }
        if let Some(config) = playwright_config(&path)? {
            return Ok(Some(config));
        }
    }

    if package_has_dependency(package, "@badeball/cypress-cucumber-preprocessor") {
        for name in CYPRESS_CONFIGS {
            let path = root.join(name);
            if path.is_file() {
                if let Some(config) = cypress_config(&path)? {
                    return Ok(Some(config));
                }
            }
        }
    }

    for name in CUCUMBER_CONFIGS {
        let path = root.join(name);
        if path.is_file() {
            return cucumber_config(&path).map(Some);
        }
    }

    if package_has_dependency(package, "@cucumber/cucumber") {
        return Ok(Some(FrameworkFeatures {
            patterns: vec![CUCUMBER_DEFAULT.to_owned()],
            source: root.join("package.json"),
            framework: "Cucumber.js",
            warnings: Vec::new(),
        }));
    }
    Ok(None)
}

fn cypress_config(path: &Path) -> Result<Option<FrameworkFeatures>> {
    let text = read_utf8(path, "framework configuration", MAX_CONFIG_INPUT_BYTES)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let spec_pattern = static_object_property_path(
        &text,
        extension,
        Some("defineConfig"),
        &["e2e", "specPattern"],
    )?;
    let mut warnings = Vec::new();
    let patterns = match spec_pattern {
        LiteralProperty::Strings(patterns) => patterns,
        LiteralProperty::Dynamic => {
            warnings.push(format!(
                "{} uses a dynamic Cypress e2e.specPattern; configure `features` in .cuke-dedup.json to make discovery deterministic",
                path.display()
            ));
            vec!["**/*.{feature,feature.md}".to_owned()]
        }
        LiteralProperty::Missing => return Ok(None),
    };
    Ok(Some(FrameworkFeatures {
        patterns,
        source: path.to_path_buf(),
        framework: "Cypress Cucumber",
        warnings,
    }))
}

fn cucumber_config(path: &Path) -> Result<FrameworkFeatures> {
    let text = read_utf8(path, "framework configuration", MAX_CONFIG_INPUT_BYTES)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let paths = match extension {
        "json" => cucumber_json_paths(&text)
            .with_context(|| format!("failed to parse framework config {}", path.display()))?,
        "yaml" | "yml" => cucumber_yaml_paths(&text)
            .with_context(|| format!("failed to parse framework config {}", path.display()))?,
        _ => static_object_property(&text, extension, None, "paths")?,
    };
    let mut warnings = Vec::new();
    let patterns = match paths {
        LiteralProperty::Missing => vec![CUCUMBER_DEFAULT.to_owned()],
        LiteralProperty::Strings(paths) => paths
            .into_iter()
            .filter_map(|path| framework_pattern(&path, FrameworkKind::Cucumber, &mut warnings))
            .collect(),
        LiteralProperty::Dynamic => {
            warnings.push(format!(
                "{} uses dynamic Cucumber paths; falling back to Cucumber's default `{CUCUMBER_DEFAULT}`",
                path.display()
            ));
            vec![CUCUMBER_DEFAULT.to_owned()]
        }
    };
    Ok(FrameworkFeatures {
        patterns,
        source: path.to_path_buf(),
        framework: "Cucumber.js",
        warnings,
    })
}

fn playwright_config(path: &Path) -> Result<Option<FrameworkFeatures>> {
    let text = read_utf8(path, "framework configuration", MAX_CONFIG_INPUT_BYTES)?;
    if !text.contains("defineBddConfig") {
        return Ok(None);
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let features = static_object_property(&text, extension, Some("defineBddConfig"), "features")?;
    let features_root =
        static_object_property(&text, extension, Some("defineBddConfig"), "featuresRoot")?;
    let mut warnings = Vec::new();
    let patterns = match features {
        LiteralProperty::Strings(paths) => paths
            .into_iter()
            .filter_map(|path| framework_pattern(&path, FrameworkKind::Playwright, &mut warnings))
            .collect(),
        LiteralProperty::Dynamic => {
            warnings.push(format!(
                "{} uses dynamic Playwright-BDD feature paths; configure `features` in .cuke-dedup.json to make discovery deterministic",
                path.display()
            ));
            vec!["**/*.{feature,feature.md}".to_owned()]
        }
        LiteralProperty::Missing => match features_root {
            LiteralProperty::Strings(roots) => roots
                .into_iter()
                .map(|root| directory_pattern(&root, "feature"))
                .collect(),
            LiteralProperty::Dynamic => {
                warnings.push(format!(
                    "{} uses a dynamic Playwright-BDD featuresRoot; configure `features` in .cuke-dedup.json to make discovery deterministic",
                    path.display()
                ));
                vec!["**/*.{feature,feature.md}".to_owned()]
            }
            LiteralProperty::Missing => vec!["features/**/*.feature".to_owned()],
        },
    };
    Ok(Some(FrameworkFeatures {
        patterns,
        source: path.to_path_buf(),
        framework: "Playwright-BDD",
        warnings,
    }))
}

#[derive(Debug, Clone, Copy)]
enum FrameworkKind {
    Cucumber,
    Playwright,
}

fn framework_pattern(
    value: &str,
    framework: FrameworkKind,
    warnings: &mut Vec<String>,
) -> Option<String> {
    let mut value = value.trim().replace('\\', "/");
    if value.is_empty() {
        return None;
    }
    while let Some(stripped) = value.strip_prefix("./") {
        value = stripped.to_owned();
    }
    if value.is_empty() {
        value = ".".to_owned();
    }
    if value.starts_with('!') {
        warnings.push(format!(
            "negated framework path `{value}` is not a discovery input; use `exclude` in .cuke-dedup.json"
        ));
        return None;
    }
    if let Some((path, line)) = value.rsplit_once(':') {
        if line.chars().all(|character| character.is_ascii_digit()) {
            value = path.to_owned();
        }
    }
    if is_glob(&value) || has_extension(&value) {
        return Some(value);
    }
    let extension = match framework {
        FrameworkKind::Cucumber => "{feature,feature.md}",
        FrameworkKind::Playwright => "feature",
    };
    Some(directory_pattern(&value, extension))
}

fn directory_pattern(path: &str, extension: &str) -> String {
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "." {
        format!("**/*.{extension}")
    } else {
        format!("{path}/**/*.{extension}")
    }
}

fn is_glob(value: &str) -> bool {
    value
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b'{'))
}

fn has_extension(value: &str) -> bool {
    Path::new(value).extension().is_some()
}

fn package_has_dependency(package: Option<&JsonValue>, dependency: &str) -> bool {
    package.is_some_and(|package| {
        ["dependencies", "devDependencies", "peerDependencies"]
            .into_iter()
            .any(|key| {
                package
                    .get(key)
                    .and_then(JsonValue::as_object)
                    .is_some_and(|map| map.contains_key(dependency))
            })
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LiteralProperty {
    Missing,
    Dynamic,
    Strings(Vec<String>),
}

fn cucumber_json_paths(text: &str) -> Result<LiteralProperty> {
    let root: JsonValue = serde_json::from_str(text)?;
    let profile = root.get("default").unwrap_or(&root);
    Ok(json_strings(profile.get("paths")))
}

fn json_strings(value: Option<&JsonValue>) -> LiteralProperty {
    match value {
        None => LiteralProperty::Missing,
        Some(JsonValue::String(value)) => LiteralProperty::Strings(vec![value.clone()]),
        Some(JsonValue::Array(values)) if values.iter().all(JsonValue::is_string) => {
            LiteralProperty::Strings(
                values
                    .iter()
                    .filter_map(JsonValue::as_str)
                    .map(ToOwned::to_owned)
                    .collect(),
            )
        }
        Some(_) => LiteralProperty::Dynamic,
    }
}

fn cucumber_yaml_paths(text: &str) -> Result<LiteralProperty> {
    let root: YamlValue = yaml_serde::from_str(text)?;
    let default_key = YamlValue::String("default".to_owned());
    let paths_key = YamlValue::String("paths".to_owned());
    let profile = root.get(&default_key).unwrap_or(&root);
    Ok(match profile.get(&paths_key) {
        None => LiteralProperty::Missing,
        Some(YamlValue::String(value)) => LiteralProperty::Strings(vec![value.clone()]),
        Some(YamlValue::Sequence(values))
            if values
                .iter()
                .all(|value| matches!(value, YamlValue::String(_))) =>
        {
            LiteralProperty::Strings(
                values
                    .iter()
                    .filter_map(YamlValue::as_str)
                    .map(ToOwned::to_owned)
                    .collect(),
            )
        }
        Some(_) => LiteralProperty::Dynamic,
    })
}

fn static_object_property(
    source: &str,
    extension: &str,
    call_name: Option<&str>,
    property: &str,
) -> Result<LiteralProperty> {
    let tree = parse_config_source(source, extension)?;

    if let Some(call_name) = call_name {
        let Some(object) = find_call_object(tree.root_node(), source, call_name) else {
            return Ok(LiteralProperty::Missing);
        };
        return Ok(read_object_property(object, source, property));
    }

    let objects = collect_objects(tree.root_node());
    for object in &objects {
        if let Some(default) = property_node(*object, source, "default") {
            if default.kind() == "object" {
                return Ok(read_object_property(default, source, property));
            }
        }
    }
    for object in objects {
        let value = read_object_property(object, source, property);
        if value != LiteralProperty::Missing {
            return Ok(value);
        }
    }
    Ok(LiteralProperty::Missing)
}

fn static_object_property_path(
    source: &str,
    extension: &str,
    call_name: Option<&str>,
    path: &[&str],
) -> Result<LiteralProperty> {
    let tree = parse_config_source(source, extension)?;

    let mut object = if let Some(call_name) = call_name {
        match find_call_object(tree.root_node(), source, call_name) {
            Some(object) => object,
            None => return Ok(LiteralProperty::Missing),
        }
    } else {
        tree.root_node()
    };
    for (index, name) in path.iter().enumerate() {
        let Some(value) = property_node(object, source, name) else {
            return Ok(LiteralProperty::Missing);
        };
        if index + 1 == path.len() {
            return Ok(literal_strings(value, source).unwrap_or(LiteralProperty::Dynamic));
        }
        if value.kind() != "object" {
            return Ok(LiteralProperty::Dynamic);
        }
        object = value;
    }
    Ok(LiteralProperty::Missing)
}

fn parse_config_source(source: &str, extension: &str) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    let language = if matches!(extension, "ts" | "mts" | "cts") {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT
    } else {
        tree_sitter_javascript::LANGUAGE
    };
    parser
        .set_language(&language.into())
        .context("failed to initialize framework config parser")?;
    let tree = parser
        .parse(source, None)
        .context("framework config parser returned no syntax tree")?;
    Ok(tree)
}

fn find_call_object<'a>(node: Node<'a>, source: &str, call_name: &str) -> Option<Node<'a>> {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if node.kind() == "call_expression"
            && node
                .child_by_field_name("function")
                .is_some_and(|function| {
                    node_text(function, source).rsplit('.').next() == Some(call_name)
                })
        {
            if let Some(arguments) = node.child_by_field_name("arguments") {
                let mut cursor = arguments.walk();
                if let Some(object) = arguments
                    .named_children(&mut cursor)
                    .find(|child| child.kind() == "object")
                {
                    return Some(object);
                };
            }
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

fn collect_objects(node: Node<'_>) -> Vec<Node<'_>> {
    let mut objects = Vec::new();
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if node.kind() == "object" {
            objects.push(node);
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    objects
}

fn read_object_property(object: Node<'_>, source: &str, name: &str) -> LiteralProperty {
    match property_node(object, source, name) {
        None => LiteralProperty::Missing,
        Some(value) => literal_strings(value, source).unwrap_or(LiteralProperty::Dynamic),
    }
}

fn property_node<'a>(object: Node<'a>, source: &str, name: &str) -> Option<Node<'a>> {
    let mut cursor = object.walk();
    for child in object.named_children(&mut cursor) {
        if child.kind() != "pair" {
            continue;
        }
        let key = child.child_by_field_name("key")?;
        if node_text(key, source).trim_matches(['\'', '"']) == name {
            return child.child_by_field_name("value");
        }
    }
    None
}

fn literal_strings(node: Node<'_>, source: &str) -> Option<LiteralProperty> {
    match node.kind() {
        "string" => Some(LiteralProperty::Strings(vec![unquote(node_text(
            node, source,
        ))?])),
        "array" => {
            let mut values = Vec::new();
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() != "string" {
                    return None;
                }
                values.push(unquote(node_text(child, source))?);
            }
            Some(LiteralProperty::Strings(values))
        }
        _ => None,
    }
}

fn unquote(value: &str) -> Option<String> {
    let value = value.strip_prefix(['\'', '"'])?.strip_suffix(['\'', '"'])?;
    Some(value.replace("\\/", "/"))
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    #[test]
    fn extracts_only_define_bdd_config_features() {
        let source = "const unrelated = { features: ['wrong'] }; const testDir = defineBddConfig({ features: ['tests/**/*.spec'] });";
        assert_eq!(
            static_object_property(source, "ts", Some("defineBddConfig"), "features").unwrap(),
            LiteralProperty::Strings(vec!["tests/**/*.spec".to_owned()])
        );
    }

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

    #[test]
    fn extracts_cypress_e2e_spec_pattern_from_project_config() {
        let source = r#"
            export default defineConfig({
                component: { specPattern: '**/*.cy.ts' },
                e2e: { specPattern: ['cypress/e2e/**/*.feature', 'acceptance/**/*.feature.md'] }
            });
        "#;
        assert_eq!(
            static_object_property_path(
                source,
                "ts",
                Some("defineConfig"),
                &["e2e", "specPattern"]
            )
            .unwrap(),
            LiteralProperty::Strings(vec![
                "cypress/e2e/**/*.feature".to_owned(),
                "acceptance/**/*.feature.md".to_owned()
            ])
        );
    }

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
        assert!(detect(tempfile::tempdir().unwrap().path(), None)
            .unwrap()
            .is_none());
    }

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
}
