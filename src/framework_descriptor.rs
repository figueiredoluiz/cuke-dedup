//! Framework descriptor contract: static feature-path discovery from framework configuration.
//!
//! A descriptor names a framework and discovers its feature patterns; adapters attach descriptors
//! to their framework contracts, whose order decides precedence. Cucumber configuration reading
//! lives here because its JSON and YAML forms are language-neutral; the adapter whose contract
//! uses it passes the [`ScriptConfigReader`]s for its scripted forms.

use crate::resource_limits::{read_utf8, MAX_CONFIG_INPUT_BYTES};
use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use std::path::{Path, PathBuf};
use yaml_serde::Value as YamlValue;

const CUCUMBER_STEM: &str = "cucumber";
type StructuredPaths = fn(&str) -> Result<LiteralProperty>;
const CUCUMBER_STRUCTURED_CONFIGS: [(&str, StructuredPaths); 3] = [
    ("cucumber.json", cucumber_json_paths),
    ("cucumber.yaml", cucumber_yaml_paths),
    ("cucumber.yml", cucumber_yaml_paths),
];
const CUCUMBER_DEFAULT: &str = "features/**/*.{feature,feature.md}";
const CUCUMBER_EXTENSIONS: &str = "{feature,feature.md}";
const CUCUMBER_DEPENDENCIES: [&str; 2] = ["@cucumber/cucumber", "cucumber"];

/// The discovery match `detect` returns: a descriptor's name with the features it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrameworkFeatures {
    pub(crate) patterns: Vec<String>,
    pub(crate) source: PathBuf,
    pub(crate) framework: &'static str,
    pub(crate) warnings: Vec<String>,
}

/// Feature patterns one configuration file, or other project evidence, yields: the glob patterns
/// relative to the project root, the file they came from, and warnings shown with the
/// configuration. Discovery normalizes each pattern as it does Cucumber `paths`: `\` becomes
/// `/`, a leading `./` and a trailing `:<line>` are dropped, a directory expands to a glob over
/// feature files, and a negated (`!`) pattern is dropped with a warning. Empty patterns select no
/// feature files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFeatures {
    pub(crate) patterns: Vec<String>,
    pub(crate) source: PathBuf,
    pub(crate) warnings: Vec<String>,
}

impl DiscoveredFeatures {
    /// Feature `patterns` read from `source`, with no warnings.
    pub fn new(patterns: Vec<String>, source: PathBuf) -> Self {
        Self {
            patterns,
            source,
            warnings: Vec::new(),
        }
    }

    /// These features with `warnings` added, such as a dynamic value replaced by a default.
    pub fn with_warnings(mut self, warnings: Vec<String>) -> Self {
        self.warnings.extend(warnings);
        self
    }

    /// The feature patterns, as the descriptor returned them.
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// The file the patterns came from.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// The warnings shown with the configuration.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// What a descriptor may inspect: the project root and its parsed `package.json`.
#[derive(Clone, Copy)]
pub struct DiscoveryInput<'a> {
    pub(crate) root: &'a Path,
    pub(crate) package: Option<&'a JsonValue>,
}

impl<'a> DiscoveryInput<'a> {
    /// The input discovery passes for `root` and its parsed `package.json`.
    pub fn new(root: &'a Path, package: Option<&'a JsonValue>) -> Self {
        Self { root, package }
    }

    /// The project root.
    pub fn root(&self) -> &'a Path {
        self.root
    }

    /// The root `package.json`, or `None` when it is absent.
    pub fn package(&self) -> Option<&'a JsonValue> {
        self.package
    }

    /// Whether `package.json` lists `name` under `dependencies`, `devDependencies` or
    /// `peerDependencies`.
    pub fn has_dependency(&self, name: &str) -> bool {
        package_has_dependency(self.package, name)
    }

    /// Reads a configuration file under the shared configuration input limit. An oversized file
    /// fails configuration loading instead of becoming a warning.
    pub fn read(&self, path: &Path) -> Result<String> {
        read_utf8(path, "framework configuration", MAX_CONFIG_INPUT_BYTES)
    }
}

/// Static discovery contract for one framework. `discover` returns the feature patterns of the
/// first matching configuration file, or of other project evidence such as a `package.json`
/// dependency (with that file as `source`), `Ok(None)` when the framework does not apply to the
/// project, or an error when an existing configuration file could not be inspected. Errors
/// propagate; they are never treated as a non-match. Configuration is only ever parsed
/// statically, never imported or executed.
pub struct FrameworkDescriptor {
    pub(crate) name: &'static str,
    pub(crate) discover: FeatureDiscovery,
}

/// A descriptor's discovery function.
pub type FeatureDiscovery = fn(&DiscoveryInput<'_>) -> Result<Option<DiscoveredFeatures>>;

impl FrameworkDescriptor {
    /// A descriptor reported as `name` in the feature-pattern origin, discovering with `discover`.
    pub const fn new(name: &'static str, discover: FeatureDiscovery) -> Self {
        Self { name, discover }
    }
}

/// A language adapter's static reader for scripted configuration files.
pub(crate) struct ScriptConfigReader {
    /// File extensions the reader parses, in discovery order.
    pub(crate) extensions: &'static [&'static str],
    /// Reads `property` from the default-profile object, else the first object defining it.
    pub(crate) read_property: fn(&str, &str, &str) -> Result<LiteralProperty>,
}

/// Classification of a configuration value: absent, not statically known, or literal strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LiteralProperty {
    Missing,
    Dynamic,
    Strings(Vec<String>),
}

/// Returns the first descriptor match in the given order, propagating configuration errors.
pub(crate) fn detect<'d>(
    descriptors: impl IntoIterator<Item = &'d FrameworkDescriptor>,
    input: &DiscoveryInput<'_>,
) -> Result<Option<FrameworkFeatures>> {
    for descriptor in descriptors {
        if let Some(found) = (descriptor.discover)(input)? {
            let mut warnings = found.warnings;
            let patterns = found
                .patterns
                .iter()
                .filter_map(|pattern| {
                    framework_pattern(pattern, CUCUMBER_EXTENSIONS, &mut warnings)
                })
                .collect();
            return Ok(Some(FrameworkFeatures {
                patterns,
                source: found.source,
                framework: descriptor.name,
                warnings,
            }));
        }
    }
    Ok(None)
}

/// Uses the first Cucumber configuration: structured files, then each extension of `script_readers`
/// in order; else default feature paths when a Cucumber dependency is present.
pub(crate) fn cucumber_discovery(
    input: &DiscoveryInput<'_>,
    script_readers: &[ScriptConfigReader],
) -> Result<Option<DiscoveredFeatures>> {
    for (name, parse) in CUCUMBER_STRUCTURED_CONFIGS {
        let path = input.root.join(name);
        if path.is_file() {
            let context = || format!("failed to parse framework config {}", path.display());
            return cucumber_config(&path, |text, _| parse(text).with_context(context)).map(Some);
        }
    }
    for reader in script_readers {
        for extension in reader.extensions {
            let path = input.root.join(format!("{CUCUMBER_STEM}.{extension}"));
            if path.is_file() {
                let read =
                    |text: &str, extension: &str| (reader.read_property)(text, extension, "paths");
                return cucumber_config(&path, read).map(Some);
            }
        }
    }
    if package_has_any_dependency(input.package, &CUCUMBER_DEPENDENCIES) {
        return Ok(Some(DiscoveredFeatures::new(
            vec![CUCUMBER_DEFAULT.to_owned()],
            input.root.join("package.json"),
        )));
    }
    Ok(None)
}

/// Inspects a provider's configuration files in listed order and returns the first recognized
/// one. A file that yields no recognized configuration is a non-match, so the next filename is
/// tried; an existing file that fails to parse aborts discovery with the error.
pub(crate) fn first_matching_config(
    root: &Path,
    configs: &[&str],
    probe: fn(&Path) -> Result<Option<DiscoveredFeatures>>,
) -> Result<Option<DiscoveredFeatures>> {
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

/// Reads a configuration file under the shared input limit, returning its text and extension.
pub(crate) fn read_config(path: &Path) -> Result<(String, &str)> {
    let text = read_utf8(path, "framework configuration", MAX_CONFIG_INPUT_BYTES)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    Ok((text, extension))
}

/// Reports whether the package declares any supplied name in a supported dependency section.
pub(crate) fn package_has_any_dependency(package: Option<&JsonValue>, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| package_has_dependency(package, name))
}

/// Reads Cucumber paths with the file's format reader, normalizing literal paths and warning before using defaults for dynamic values.
fn cucumber_config(
    path: &Path,
    read_paths: impl FnOnce(&str, &str) -> Result<LiteralProperty>,
) -> Result<DiscoveredFeatures> {
    let (text, extension) = read_config(path)?;
    let paths = read_paths(&text, extension)?;
    let mut warnings = Vec::new();
    let patterns = match paths {
        LiteralProperty::Missing => vec![CUCUMBER_DEFAULT.to_owned()],
        LiteralProperty::Strings(paths) => paths
            .into_iter()
            .filter_map(|path| framework_pattern(&path, CUCUMBER_EXTENSIONS, &mut warnings))
            .collect(),
        LiteralProperty::Dynamic => {
            warnings.push(format!(
                "{} uses dynamic Cucumber paths; falling back to Cucumber's default `{CUCUMBER_DEFAULT}`",
                path.display()
            ));
            vec![CUCUMBER_DEFAULT.to_owned()]
        }
    };
    Ok(DiscoveredFeatures::new(patterns, path.to_path_buf()).with_warnings(warnings))
}

/// Drops initially blank inputs and negated paths, normalizes prefixes and numeric line suffixes, and expands directory or dot paths into globs over `extension`.
pub(crate) fn framework_pattern(
    value: &str,
    extension: &str,
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
    Some(directory_pattern(&value, extension))
}

/// Appends a recursive glob for the requested extension, treating empty and dot paths as the current directory.
pub(crate) fn directory_pattern(path: &str, extension: &str) -> String {
    let path = path.trim_end_matches('/');
    if path.is_empty() || path == "." {
        format!("**/*.{extension}")
    } else {
        format!("{path}/**/*.{extension}")
    }
}

/// Reports whether a path contains any of the supported glob-opening characters.
fn is_glob(value: &str) -> bool {
    value
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b'{'))
}

/// Reports whether the final path component has an extension.
fn has_extension(value: &str) -> bool {
    Path::new(value).extension().is_some()
}

/// Checks dependencies, devDependencies, and peerDependencies for the requested package name.
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

/// Parses JSON and classifies paths from the default profile when present, otherwise from the root.
fn cucumber_json_paths(text: &str) -> Result<LiteralProperty> {
    let root: JsonValue = serde_json::from_str(text)?;
    let profile = root.get("default").unwrap_or(&root);
    Ok(json_strings(profile.get("paths")))
}

/// Classifies an absent JSON value, a literal string or all-string array, or an unsupported dynamic value.
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

/// Parses YAML and classifies paths from the default profile when present, otherwise from the root.
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

#[cfg(test)]
mod tests;
