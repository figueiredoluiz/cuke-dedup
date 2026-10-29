//! JavaScript/TypeScript framework descriptors and the scripted-configuration reader.

use crate::framework_descriptor::{
    cucumber_discovery, directory_pattern, first_matching_config, framework_pattern,
    package_has_any_dependency, read_config, DiscoveredFeatures, DiscoveryInput,
    FrameworkDescriptor, LiteralProperty, ScriptConfigReader,
};
use anyhow::{Context, Result};
use std::path::Path;
use tree_sitter::{Node, Parser};

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
const CYPRESS_PREPROCESSORS: [&str; 2] = [
    "@badeball/cypress-cucumber-preprocessor",
    "cypress-cucumber-preprocessor",
];
const BROAD_FEATURES: &str = "**/*.{feature,feature.md}";

/// Recognized Playwright-BDD configuration.
pub(crate) const PLAYWRIGHT_BDD_DISCOVERY: FrameworkDescriptor =
    FrameworkDescriptor::new("Playwright-BDD", playwright_discovery);

/// Cypress configuration, when a supported Cucumber preprocessor dependency is present.
pub(crate) const CYPRESS_CUCUMBER_DISCOVERY: FrameworkDescriptor =
    FrameworkDescriptor::new("Cypress Cucumber", cypress_discovery);

/// Cucumber configuration files, scripted ones in the JavaScript and TypeScript extensions, else
/// default paths when a Cucumber dependency is present.
pub(crate) const CUCUMBER_JS_DISCOVERY: FrameworkDescriptor =
    FrameworkDescriptor::new("Cucumber.js", |input| {
        cucumber_discovery(input, &[SCRIPT_CONFIG_READER])
    });

/// Reads scripted configuration in the JavaScript and TypeScript module extensions.
const SCRIPT_CONFIG_READER: ScriptConfigReader = ScriptConfigReader {
    extensions: &["js", "cjs", "mjs", "ts", "cts", "mts"],
    read_property: |source, extension, property| {
        static_object_property(source, extension, None, property)
    },
};

/// Inspects supported Playwright configuration filenames in precedence order for a BDD configuration.
fn playwright_discovery(input: &DiscoveryInput<'_>) -> Result<Option<DiscoveredFeatures>> {
    first_matching_config(input.root, &PLAYWRIGHT_CONFIGS, playwright_config)
}

/// Inspects Cypress configurations only when a supported Cucumber preprocessor dependency is present.
fn cypress_discovery(input: &DiscoveryInput<'_>) -> Result<Option<DiscoveredFeatures>> {
    if !package_has_any_dependency(input.package, &CYPRESS_PREPROCESSORS) {
        return Ok(None);
    }
    first_matching_config(input.root, &CYPRESS_CONFIGS, cypress_config)
}

/// Reads static e2e.specPattern values, using broad feature discovery with a warning for dynamic values and returning no match when absent.
fn cypress_config(path: &Path) -> Result<Option<DiscoveredFeatures>> {
    let (text, extension) = read_config(path)?;
    let property = ["e2e", "specPattern"];
    let spec_pattern =
        static_object_property_path(&text, extension, Some("defineConfig"), &property)?;
    let mut warnings = Vec::new();
    let patterns = match spec_pattern {
        LiteralProperty::Strings(patterns) => patterns,
        LiteralProperty::Dynamic => {
            warnings.push(format!(
                "{} uses a dynamic Cypress e2e.specPattern; configure `features` in .cuke-dedup.json to make discovery deterministic",
                path.display()
            ));
            vec![BROAD_FEATURES.to_owned()]
        }
        LiteralProperty::Missing => return Ok(None),
    };
    Ok(Some(
        DiscoveredFeatures::new(patterns, path.to_path_buf()).with_warnings(warnings),
    ))
}

/// Reads defineBddConfig feature paths or roots statically, supplying defaults for missing values and warnings for dynamic values.
fn playwright_config(path: &Path) -> Result<Option<DiscoveredFeatures>> {
    let (text, extension) = read_config(path)?;
    if !text.contains("defineBddConfig") {
        return Ok(None);
    }
    let features = static_object_property(&text, extension, Some("defineBddConfig"), "features")?;
    let features_root =
        static_object_property(&text, extension, Some("defineBddConfig"), "featuresRoot")?;
    let mut warnings = Vec::new();
    let patterns = match features {
        LiteralProperty::Strings(paths) => paths
            .into_iter()
            .filter_map(|path| framework_pattern(&path, "feature", &mut warnings))
            .collect(),
        LiteralProperty::Dynamic => {
            warnings.push(format!(
                "{} uses dynamic Playwright-BDD feature paths; configure `features` in .cuke-dedup.json to make discovery deterministic",
                path.display()
            ));
            vec![BROAD_FEATURES.to_owned()]
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
                vec![BROAD_FEATURES.to_owned()]
            }
            LiteralProperty::Missing => vec!["features/**/*.feature".to_owned()],
        },
    };
    Ok(Some(
        DiscoveredFeatures::new(patterns, path.to_path_buf()).with_warnings(warnings),
    ))
}

/// Reads a static property from a matching call or object, preferring an object-valued default profile when no call is specified.
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

/// Follows a property path from the first matching call object or syntax root, distinguishing absent properties from nonliteral or non-object values.
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

/// Parses configuration text with the TypeScript or JavaScript grammar selected by extension, propagating parser setup or missing-tree errors.
fn parse_config_source(source: &str, extension: &str) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    let grammar: tree_sitter::Language = if matches!(extension, "ts" | "mts" | "cts") {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    } else {
        tree_sitter_javascript::LANGUAGE.into()
    };
    parser
        .set_language(&grammar)
        .context("failed to initialize framework config parser")?;
    let tree = parser
        .parse(source, None)
        .context("framework config parser returned no syntax tree")?;
    Ok(tree)
}

/// Returns the first object argument of a call whose final dotted function-name segment matches, traversing in source order.
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
            let object = node.child_by_field_name("arguments").and_then(|arguments| {
                let mut cursor = arguments.walk();
                let mut objects = arguments.named_children(&mut cursor);
                objects.find(|child| child.kind() == "object")
            });
            if object.is_some() {
                return object;
            }
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

/// Collects object nodes in depth-first source order, including nested objects.
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

/// Classifies an object property as missing, a literal string list, or dynamic.
fn read_object_property(object: Node<'_>, source: &str, name: &str) -> LiteralProperty {
    match property_node(object, source, name) {
        None => LiteralProperty::Missing,
        Some(value) => literal_strings(value, source).unwrap_or(LiteralProperty::Dynamic),
    }
}

/// Returns the value of the first pair whose key text matches after trimming quotes; shorthand and spread entries are skipped.
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

/// Reads a quoted string or an array containing only quoted strings, returning None for other syntax.
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
        _ => None, // fail-closed: callers classify other syntax as dynamic and warn.
    }
}

/// Removes leading and trailing quote characters and unescapes forward slashes, returning None when either quote is absent.
fn unquote(value: &str) -> Option<String> {
    let value = value.strip_prefix(['\'', '"'])?.strip_suffix(['\'', '"'])?;
    Some(value.replace("\\/", "/"))
}

/// Returns the source slice covered by the syntax node byte range.
fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

#[cfg(test)]
mod tests;
