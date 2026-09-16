//! Configuration loading and precedence.

use crate::framework_config;
use crate::model::{Rule, Severity};
use crate::resource_limits::{
    is_input_limit_error, read_utf8, MAX_CANDIDATE_COMPARISONS, MAX_CONFIG_INPUT_BYTES,
    MAX_STRUCTURAL_CLASS_COMPARISONS, MAX_SUPPRESSION_REASON_CHARS,
};
use anyhow::{bail, Context, Result};
use globset::Glob;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

const DEFAULT_EXCLUDES: [&str; 5] = [
    "**/node_modules/**",
    "**/target/**",
    "**/.features-gen/**",
    "**/test-results/**",
    "**/playwright-report/**",
];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Destination format selected for analysis output.
#[non_exhaustive]
pub enum ReporterKind {
    /// Human-readable output written to the supplied terminal stream.
    Terminal,
    /// Versioned machine-readable JSON output.
    Json,
    /// Compact JSON Lines output written to stdout for streaming consumers.
    Jsonl,
    /// Self-contained interactive HTML output.
    Html,
    /// SARIF 2.1.0 output for GitHub code scanning and other compatible tools.
    Sarif,
}

impl FromStr for ReporterKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "terminal" => Ok(Self::Terminal),
            "json" => Ok(Self::Json),
            "jsonl" => Ok(Self::Jsonl),
            "html" => Ok(Self::Html),
            "sarif" => Ok(Self::Sarif),
            _ => Err(format!("unknown reporter `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    definitions: Option<Vec<String>>,
    #[serde(default)]
    features: Option<Vec<String>>,
    #[serde(default)]
    exclude: Option<Vec<String>>,
    #[serde(default)]
    exclude_defaults: Option<bool>,
    #[serde(default)]
    include_hidden: Option<bool>,
    #[serde(default)]
    reporters: Option<Vec<ReporterKind>>,
    #[serde(default)]
    output: Option<PathBuf>,
    #[serde(default)]
    threshold: Option<f64>,
    #[serde(default)]
    require_features: Option<bool>,
    #[serde(default)]
    require_definitions: Option<bool>,
    #[serde(default)]
    fail_on_incomplete: Option<bool>,
    #[serde(default)]
    registrations: Option<Vec<String>>,
    #[serde(default)]
    parameter_types: Option<BTreeMap<String, String>>,
    #[serde(default)]
    max_candidate_comparisons: Option<usize>,
    #[serde(default)]
    max_structural_class_comparisons: Option<usize>,
    #[serde(default)]
    no_metrics: Option<bool>,
    #[serde(default)]
    rules: Option<BTreeMap<String, Severity>>,
    #[serde(default)]
    suppressions: Option<Vec<SuppressionConfig>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PackageJson {
    #[serde(default)]
    cuke_dedup: Option<RawConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// A configuration entry that suppresses matching findings for a documented reason.
pub struct SuppressionConfig {
    /// Rule to suppress.
    pub rule: Rule,
    /// Human-readable justification recorded with suppressed findings.
    pub reason: String,
    #[serde(default)]
    /// Optional glob selecting source paths.
    pub path: Option<String>,
    #[serde(default)]
    /// Optional exact matcher selecting step definitions.
    pub matcher: Option<String>,
}

#[derive(Debug, Clone, Default)]
/// Command-line or embedding overrides applied after file-based configuration.
pub struct ConfigOverrides {
    /// Explicit JSON configuration file, bypassing automatic config-file discovery.
    pub config_file: Option<PathBuf>,
    /// Replacement definition-source globs.
    pub definitions: Option<Vec<String>>,
    /// Replacement feature-file globs.
    pub features: Option<Vec<String>>,
    /// Replacement custom exclusion globs.
    pub exclude: Option<Vec<String>>,
    /// Whether built-in generated-directory exclusions remain enabled.
    pub exclude_defaults: Option<bool>,
    /// Whether discovery descends into dot-directories.
    pub include_hidden: Option<bool>,
    /// Replacement reporter selection.
    pub reporters: Option<Vec<ReporterKind>>,
    /// Replacement report output directory.
    pub output: Option<PathBuf>,
    /// Maximum percentage of definitions allowed in active duplication errors.
    pub threshold: Option<f64>,
    /// Whether a run with no discovered feature files is an operational failure.
    pub require_features: Option<bool>,
    /// Whether machine reports omit runtime measurements.
    pub no_metrics: Option<bool>,
    /// Per-rule severity overrides.
    pub rules: BTreeMap<Rule, Severity>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CliConfigOverrides {
    pub(crate) require_definitions: Option<bool>,
    /// Whether bounded or incomplete analysis is an operational failure.
    pub(crate) fail_on_incomplete: Option<bool>,
    pub(crate) max_candidate_comparisons: Option<usize>,
    pub(crate) max_structural_class_comparisons: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
/// Fully resolved and validated analyzer configuration.
pub struct Config {
    /// Canonical repository root being analyzed.
    pub root: PathBuf,
    /// Globs selecting definition sources, or an empty list for automatic discovery.
    pub definitions: Vec<String>,
    /// Globs selecting classic Gherkin or Gherkin Markdown feature files.
    pub features: Vec<String>,
    /// Human-readable origin of the effective feature globs.
    pub feature_pattern_origin: String,
    /// Non-fatal diagnostics produced while inspecting framework configuration.
    pub config_warnings: Vec<String>,
    /// Standalone or package configuration source selected below CLI overrides.
    pub config_source: String,
    /// Globs excluded from discovery.
    pub exclude: Vec<String>,
    /// Whether built-in generated-directory exclusions are active.
    pub exclude_defaults: bool,
    /// Whether discovery descends into dot-directories.
    pub include_hidden: bool,
    /// Report formats emitted by the analyzer.
    pub reporters: Vec<ReporterKind>,
    /// Directory for file-based reports.
    pub output: PathBuf,
    /// Maximum percentage of definitions allowed in active duplication errors.
    pub threshold: f64,
    /// Whether finding no feature files is an operational failure.
    pub require_features: bool,
    /// Whether extracting no step definitions is an operational failure.
    pub require_definitions: bool,
    /// Whether an incomplete corpus or truncated analysis is an operational failure.
    pub fail_on_incomplete: bool,
    /// Additional local function names that register step definitions.
    pub registrations: Vec<String>,
    /// Project-defined Cucumber Expression parameter types and their regular expressions.
    pub parameter_types: BTreeMap<String, String>,
    /// Maximum unique definition pairs retained for analysis.
    pub max_candidate_comparisons: usize,
    /// Maximum structural candidate proposals considered from one handler class.
    pub max_structural_class_comparisons: usize,
    /// Whether machine reports omit runtime measurements for reproducibility.
    pub no_metrics: bool,
    /// Effective severity for each analysis rule.
    pub rules: BTreeMap<Rule, Severity>,
    /// Explicit, reason-bearing finding suppressions.
    pub suppressions: Vec<SuppressionConfig>,
    #[serde(skip)]
    custom_exclude: Vec<String>,
    #[serde(skip)]
    output_from_project_config: bool,
}

impl Config {
    /// Loads framework, package, and standalone configuration beneath `root`, then applies
    /// `overrides` in precedence order.
    pub fn load(root: &Path, overrides: ConfigOverrides) -> Result<Self> {
        Self::load_internal(root, overrides, CliConfigOverrides::default())
    }

    pub(crate) fn load_for_cli(
        root: &Path,
        overrides: ConfigOverrides,
        cli_overrides: CliConfigOverrides,
    ) -> Result<Self> {
        Self::load_internal(root, overrides, cli_overrides)
    }

    fn load_internal(
        root: &Path,
        overrides: ConfigOverrides,
        cli_overrides: CliConfigOverrides,
    ) -> Result<Self> {
        let root = normalize_platform_path(
            root.canonicalize()
                .with_context(|| format!("cannot access target directory {}", root.display()))?,
        );
        if !root.is_dir() {
            bail!("target {} is not a directory", root.display());
        }

        let package_path = root.join("package.json");
        let (package_config, package_json) = if package_path.is_file() {
            let text = read_utf8(
                &package_path,
                "package configuration",
                MAX_CONFIG_INPUT_BYTES,
            )?;
            // serde_json's default recursion limit remains enabled for untrusted config input.
            let value = serde_json::from_str::<serde_json::Value>(&text)
                .with_context(|| format!("failed to parse {}", package_path.display()))?;
            let package = serde_json::from_value::<PackageJson>(value.clone())
                .with_context(|| format!("failed to parse {}", package_path.display()))?;
            (package.cuke_dedup, Some(value))
        } else {
            (None, None)
        };

        let explicit_config = overrides.config_file.as_ref().map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                root.join(path)
            }
        });
        let mut config_warnings = Vec::new();
        let (standalone_config, standalone_path) = if let Some(path) = explicit_config {
            (Some(read_raw_config(&path)?), Some(path))
        } else {
            discover_config_file(&root, &mut config_warnings)?
        };

        let package_config = if standalone_config.is_none() {
            package_config
        } else {
            None
        };

        let has_explicit_features = overrides.features.is_some()
            || standalone_config
                .as_ref()
                .is_some_and(|config| config.features.is_some())
            || package_config
                .as_ref()
                .is_some_and(|config| config.features.is_some());
        let mut config = Self::defaults(root);
        config.config_warnings = config_warnings;
        if !has_explicit_features {
            match framework_config::detect(&config.root, package_json.as_ref()) {
                Ok(Some(framework)) => {
                    config.features = framework.patterns;
                    config.feature_pattern_origin = format!(
                        "{} configuration {}",
                        framework.framework,
                        framework
                            .source
                            .strip_prefix(&config.root)
                            .unwrap_or(&framework.source)
                            .display()
                    );
                    config.config_warnings.extend(framework.warnings);
                }
                Ok(None) => {}
                Err(error) if is_input_limit_error(&error) => return Err(error),
                Err(error) => config.config_warnings.push(format!(
                    "could not inspect framework configuration ({error:#}); using built-in feature patterns"
                )),
            }
        }
        if let Some(raw) = package_config {
            let replaces_features = raw.features.is_some();
            config.apply_raw(raw)?;
            config.config_source = "package.json#cukeDedup".to_owned();
            if replaces_features {
                config.feature_pattern_origin = "package.json#cukeDedup".to_owned();
            }
        }
        if let Some(raw) = standalone_config {
            let replaces_features = raw.features.is_some();
            config.apply_raw(raw)?;
            config.config_source = standalone_path
                .as_ref()
                .map(|path| config_source_name(path, &config.root))
                .unwrap_or_else(|| ".cuke-dedup.json".to_owned());
            if replaces_features {
                config.feature_pattern_origin = config.config_source.clone();
            }
        }
        let cli_replaces_features = overrides.features.is_some();
        config.apply_overrides(overrides);
        if let Some(value) = cli_overrides.require_definitions {
            config.require_definitions = value;
        }
        if let Some(value) = cli_overrides.fail_on_incomplete {
            config.fail_on_incomplete = value;
        }
        if let Some(value) = cli_overrides.max_candidate_comparisons {
            config.max_candidate_comparisons = value;
        }
        if let Some(value) = cli_overrides.max_structural_class_comparisons {
            config.max_structural_class_comparisons = value;
        }
        if cli_replaces_features {
            config.feature_pattern_origin = "--features".to_owned();
        }
        config.validate()?;
        Ok(config)
    }

    fn defaults(root: PathBuf) -> Self {
        let rules = [
            (Rule::DuplicateMatcher, Severity::Error),
            (Rule::NormalizedMatcher, Severity::Error),
            (Rule::AmbiguousStep, Severity::Error),
            (Rule::OverlappingMatcher, Severity::Warning),
            (Rule::DuplicateHandler, Severity::Error),
            (Rule::NearDuplicateStep, Severity::Warning),
            (Rule::ParameterizationCandidate, Severity::Warning),
            (Rule::UnusedDefinition, Severity::Warning),
        ]
        .into_iter()
        .collect();
        Self {
            root,
            definitions: Vec::new(),
            features: vec!["**/*.{feature,feature.md}".to_owned()],
            feature_pattern_origin: "built-in defaults".to_owned(),
            config_warnings: Vec::new(),
            config_source: "built-in defaults".to_owned(),
            exclude: DEFAULT_EXCLUDES.iter().map(ToString::to_string).collect(),
            exclude_defaults: true,
            include_hidden: false,
            reporters: vec![ReporterKind::Terminal],
            output: PathBuf::from("reports/cuke-dedup"),
            threshold: 0.0,
            require_features: false,
            require_definitions: false,
            fail_on_incomplete: false,
            registrations: Vec::new(),
            parameter_types: BTreeMap::new(),
            max_candidate_comparisons: MAX_CANDIDATE_COMPARISONS,
            max_structural_class_comparisons: MAX_STRUCTURAL_CLASS_COMPARISONS,
            no_metrics: false,
            rules,
            suppressions: Vec::new(),
            custom_exclude: Vec::new(),
            output_from_project_config: false,
        }
    }

    fn apply_raw(&mut self, raw: RawConfig) -> Result<()> {
        if let Some(value) = raw.definitions {
            self.definitions = value;
        }
        if let Some(value) = raw.features {
            self.features = value;
        }
        if let Some(value) = raw.exclude_defaults {
            self.exclude_defaults = value;
        }
        if let Some(value) = raw.include_hidden {
            self.include_hidden = value;
        }
        if let Some(value) = raw.exclude {
            self.custom_exclude = value;
        }
        self.rebuild_excludes();
        if let Some(value) = raw.reporters {
            self.reporters = value;
        }
        if let Some(value) = raw.output {
            self.output = value;
            self.output_from_project_config = true;
        }
        if let Some(value) = raw.threshold {
            self.threshold = value;
        }
        if let Some(value) = raw.require_features {
            self.require_features = value;
        }
        if let Some(value) = raw.require_definitions {
            self.require_definitions = value;
        }
        if let Some(value) = raw.fail_on_incomplete {
            self.fail_on_incomplete = value;
        }
        if let Some(value) = raw.registrations {
            self.registrations = value;
        }
        if let Some(value) = raw.parameter_types {
            self.parameter_types = value;
        }
        if let Some(value) = raw.max_candidate_comparisons {
            self.max_candidate_comparisons = value;
        }
        if let Some(value) = raw.max_structural_class_comparisons {
            self.max_structural_class_comparisons = value;
        }
        if let Some(value) = raw.no_metrics {
            self.no_metrics = value;
        }
        if let Some(value) = raw.rules {
            for (name, severity) in value {
                let rule = name.parse::<Rule>().map_err(anyhow::Error::msg)?;
                self.rules.insert(rule, severity);
            }
        }
        if let Some(value) = raw.suppressions {
            self.suppressions = value;
        }
        Ok(())
    }

    fn apply_overrides(&mut self, overrides: ConfigOverrides) {
        if let Some(value) = overrides.definitions {
            self.definitions = value;
        }
        if let Some(value) = overrides.features {
            self.features = value;
        }
        if let Some(value) = overrides.exclude_defaults {
            self.exclude_defaults = value;
        }
        if let Some(value) = overrides.include_hidden {
            self.include_hidden = value;
        }
        if let Some(value) = overrides.exclude {
            self.custom_exclude = value;
        }
        self.rebuild_excludes();
        if let Some(value) = overrides.reporters {
            self.reporters = value;
        }
        if let Some(value) = overrides.output {
            self.output = value;
            self.output_from_project_config = false;
        }
        if let Some(value) = overrides.threshold {
            self.threshold = value;
        }
        if let Some(value) = overrides.require_features {
            self.require_features = value;
        }
        if let Some(value) = overrides.no_metrics {
            self.no_metrics = value;
        }
        self.rules.extend(overrides.rules);
    }

    fn rebuild_excludes(&mut self) {
        self.exclude.clear();
        if self.exclude_defaults {
            self.exclude
                .extend(DEFAULT_EXCLUDES.iter().map(ToString::to_string));
        }
        self.exclude.extend(self.custom_exclude.iter().cloned());
    }

    fn validate(&self) -> Result<()> {
        if self.reporters.is_empty() {
            bail!("at least one reporter must be configured");
        }
        if self.reporters.contains(&ReporterKind::Terminal)
            && self.reporters.contains(&ReporterKind::Jsonl)
        {
            bail!("terminal and jsonl reporters cannot share stdout; select only one of them");
        }
        if !self.threshold.is_finite() || !(0.0..=100.0).contains(&self.threshold) {
            bail!("threshold must be a finite percentage from 0 through 100");
        }
        for (name, pattern) in &self.parameter_types {
            if name.is_empty() || name.contains(['{', '}']) {
                bail!("parameter type `{name}` must not be empty or contain braces");
            }
            crate::resource_limits::compile_regex(pattern).with_context(|| {
                format!("invalid regular expression for parameter type `{name}`")
            })?;
        }
        for name in &self.registrations {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
                || name.starts_with(|character: char| character.is_ascii_digit())
            {
                bail!("registration `{name}` must be a JavaScript identifier");
            }
        }
        self.validate_analysis_limits()?;
        if self.output_from_project_config {
            validate_project_output(&self.root, &self.output)?;
        }
        for suppression in &self.suppressions {
            if suppression.reason.trim().is_empty() {
                bail!(
                    "suppression for {} must include a non-empty reason",
                    suppression.rule
                );
            }
            if suppression.reason.chars().count() > MAX_SUPPRESSION_REASON_CHARS {
                bail!(
                    "suppression reason for {} exceeds the {}-character limit",
                    suppression.rule,
                    MAX_SUPPRESSION_REASON_CHARS
                );
            }
            if suppression.path.is_none() && suppression.matcher.is_none() {
                bail!(
                    "suppression for {} must select a path or matcher",
                    suppression.rule
                );
            }
            if let Some(pattern) = &suppression.path {
                Glob::new(pattern).with_context(|| {
                    format!(
                        "invalid suppression path glob `{pattern}` for {}",
                        suppression.rule
                    )
                })?;
            }
        }
        Ok(())
    }

    pub(crate) fn validate_analysis_limits(&self) -> Result<()> {
        if self.max_candidate_comparisons == 0 {
            bail!("maxCandidateComparisons must be greater than zero");
        }
        if self.max_candidate_comparisons > MAX_CANDIDATE_COMPARISONS {
            bail!(
                "maxCandidateComparisons must not exceed the hard safety limit of {MAX_CANDIDATE_COMPARISONS}"
            );
        }
        if self.max_structural_class_comparisons == 0 {
            bail!("maxStructuralClassComparisons must be greater than zero");
        }
        if self.max_structural_class_comparisons > MAX_STRUCTURAL_CLASS_COMPARISONS {
            bail!(
                "maxStructuralClassComparisons must not exceed the hard safety limit of {MAX_STRUCTURAL_CLASS_COMPARISONS}"
            );
        }
        Ok(())
    }

    /// Returns the configured severity for `rule`, defaulting to [`Severity::Off`].
    pub fn severity(&self, rule: Rule) -> Severity {
        self.rules.get(&rule).copied().unwrap_or(Severity::Off)
    }

    /// Resolves a report filename against the configured output directory.
    pub fn output_path(&self, filename: &str) -> PathBuf {
        if self.output.is_absolute() {
            self.output.join(filename)
        } else {
            self.root.join(&self.output).join(filename)
        }
    }
}

fn validate_project_output(root: &Path, output: &Path) -> Result<()> {
    if output.is_absolute()
        || output.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!(
            "project configuration output `{}` must stay beneath the analyzed root; use --output for an operator-controlled path",
            output.display()
        );
    }

    let candidate = root.join(output);
    let mut existing_ancestor = candidate.as_path();
    while !existing_ancestor.exists() {
        existing_ancestor = existing_ancestor.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "could not resolve project configuration output `{}`",
                output.display()
            )
        })?;
    }
    let canonical_ancestor =
        normalize_platform_path(existing_ancestor.canonicalize().with_context(|| {
            format!(
                "failed to validate project configuration output {}",
                candidate.display()
            )
        })?);
    if !canonical_ancestor.starts_with(root) {
        bail!(
            "project configuration output `{}` resolves outside the analyzed root; use --output for an operator-controlled path",
            output.display()
        );
    }
    Ok(())
}

pub(crate) fn normalize_platform_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(local) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(local);
        }
    }
    path
}

fn read_raw_config(path: &Path) -> Result<RawConfig> {
    let text = read_utf8(path, "CukeDedup configuration", MAX_CONFIG_INPUT_BYTES)?;
    // serde_json's default recursion limit remains enabled for untrusted config input.
    serde_json::from_str::<RawConfig>(&text)
        .with_context(|| format!("failed to parse {}", path.display()))
}

fn discover_config_file(
    root: &Path,
    warnings: &mut Vec<String>,
) -> Result<(Option<RawConfig>, Option<PathBuf>)> {
    const CANDIDATES: [&str; 4] = [
        ".cuke-dedup.json",
        ".config/cuke-dedup.json",
        ".config/.cuke-dedup.json",
        "cuke-dedup.config.json",
    ];
    for candidate in CANDIDATES {
        let path = root.join(candidate);
        if !path.is_file() {
            continue;
        }
        match read_raw_config(&path) {
            Ok(config) => return Ok((Some(config), Some(path))),
            Err(error) if is_input_limit_error(&error) => return Err(error),
            Err(error) => warnings.push(format!(
                "could not load auto-discovered config {} ({error:#}); trying the next configuration source",
                path.display()
            )),
        }
    }
    Ok((None, None))
}

fn config_source_name(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests;
