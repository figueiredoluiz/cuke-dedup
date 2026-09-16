//! Safe, static feature-path discovery from supported framework configuration files.
//!
//! Frameworks register in [`DISCOVERY_REGISTRY`], whose order is the discovery precedence.

use crate::resource_limits::{read_utf8, MAX_CONFIG_INPUT_BYTES};
use crate::source_adapter::{grammar_for_language, SourceLanguage};
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
const CYPRESS_PREPROCESSORS: [&str; 2] = [
    "@badeball/cypress-cucumber-preprocessor",
    "cypress-cucumber-preprocessor",
];
const CUCUMBER_DEPENDENCIES: [&str; 2] = ["@cucumber/cucumber", "cucumber"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameworkFeatures {
    pub(crate) patterns: Vec<String>,
    pub(crate) source: PathBuf,
    pub(crate) framework: &'static str,
    pub(crate) warnings: Vec<String>,
}

/// Static discovery contract for one framework's configuration. A discovery function returns the
/// feature patterns of the first matching configuration file, `Ok(None)` when the framework does
/// not apply to the project, or an error when an existing configuration file could not be
/// inspected. Errors propagate; they are never treated as a non-match. Configuration is only
/// ever parsed statically, never imported or executed.
type FrameworkDiscovery = fn(&Path, Option<&JsonValue>) -> Result<Option<FrameworkFeatures>>;

/// Ordered discovery registry: the first provider to return a match decides, so the array order
/// is the framework precedence — recognized Playwright-BDD configuration, then Cypress
/// configuration when a supported Cucumber preprocessor dependency is present, then Cucumber
/// configuration files, then Cucumber dependency-based default paths.
const DISCOVERY_REGISTRY: [FrameworkDiscovery; 3] =
    [playwright_discovery, cypress_discovery, cucumber_discovery];

pub(crate) fn detect(
    root: &Path,
    package: Option<&JsonValue>,
) -> Result<Option<FrameworkFeatures>> {
    for discovery in DISCOVERY_REGISTRY {
        if let Some(features) = discovery(root, package)? {
            return Ok(Some(features));
        }
    }
    Ok(None)
}

/// Inspects a provider's configuration files in listed order and returns the first recognized
/// one. A file that yields no recognized configuration is a non-match, so the next filename is
/// tried; an existing file that fails to parse aborts discovery with the error.
fn first_matching_config(
    root: &Path,
    configs: &[&str],
    probe: fn(&Path) -> Result<Option<FrameworkFeatures>>,
) -> Result<Option<FrameworkFeatures>> {
    for name in configs {
        let path = root.join(name);
        if !path.is_file() {
            continue;
        }
        if let Some(features) = probe(&path)? {
            return Ok(Some(features));
        }
    }
    Ok(None)
}

fn playwright_discovery(
    root: &Path,
    _package: Option<&JsonValue>,
) -> Result<Option<FrameworkFeatures>> {
    first_matching_config(root, &PLAYWRIGHT_CONFIGS, playwright_config)
}

fn cypress_discovery(
    root: &Path,
    package: Option<&JsonValue>,
) -> Result<Option<FrameworkFeatures>> {
    if !package_has_any_dependency(package, &CYPRESS_PREPROCESSORS) {
        return Ok(None);
    }
    first_matching_config(root, &CYPRESS_CONFIGS, cypress_config)
}

fn cucumber_discovery(
    root: &Path,
    package: Option<&JsonValue>,
) -> Result<Option<FrameworkFeatures>> {
    if let Some(features) = first_matching_config(root, &CUCUMBER_CONFIGS, cucumber_file_config)? {
        return Ok(Some(features));
    }
    if package_has_any_dependency(package, &CUCUMBER_DEPENDENCIES) {
        return Ok(Some(cucumber_default_features(root)));
    }
    Ok(None)
}

/// Unlike the scripted frameworks, any existing Cucumber configuration file matches, whatever its
/// contents: missing paths fall back to the default.
fn cucumber_file_config(path: &Path) -> Result<Option<FrameworkFeatures>> {
    cucumber_config(path).map(Some)
}

fn cucumber_default_features(root: &Path) -> FrameworkFeatures {
    FrameworkFeatures {
        patterns: vec![CUCUMBER_DEFAULT.to_owned()],
        source: root.join("package.json"),
        framework: "Cucumber.js",
        warnings: Vec::new(),
    }
}

fn package_has_any_dependency(package: Option<&JsonValue>, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| package_has_dependency(package, name))
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
    if call_name.is_some() {
        return static_object_property_path(source, extension, call_name, &[property]);
    }

    let tree = parse_config_source(source, extension)?;
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
        SourceLanguage::TypeScript
    } else {
        SourceLanguage::JavaScript
    };
    parser
        .set_language(&grammar_for_language(language))
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
mod tests;
