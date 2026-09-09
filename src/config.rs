//! Configuration loading and precedence.

use crate::framework_config;
use crate::model::{Rule, Severity};
use crate::resource_limits::{is_input_limit_error, read_utf8, MAX_CONFIG_INPUT_BYTES};
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
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn standalone_config_overrides_package_and_cli_overrides_both() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("package.json"),
            r#"{"cukeDedup":{"reporters":["html"],"threshold":10,"rules":{"duplicate-matcher":"warning"}}}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join("cuke-dedup.config.json"),
            r#"{"reporters":["json"],"threshold":20,"rules":{"duplicate-matcher":"off"}}"#,
        )
        .unwrap();
        let mut overrides = ConfigOverrides {
            reporters: Some(vec![ReporterKind::Terminal]),
            threshold: Some(30.0),
            ..ConfigOverrides::default()
        };
        overrides
            .rules
            .insert(Rule::DuplicateMatcher, Severity::Error);

        let config = Config::load(directory.path(), overrides).unwrap();
        assert_eq!(config.reporters, vec![ReporterKind::Terminal]);
        assert_eq!(config.threshold, 30.0);
        assert_eq!(config.severity(Rule::DuplicateMatcher), Severity::Error);
    }

    #[test]
    fn threshold_defaults_to_zero_and_rejects_out_of_range_values() {
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(config.threshold, 0.0);

        for threshold in [-0.1, 100.1, f64::NAN, f64::INFINITY] {
            let error = Config::load(
                directory.path(),
                ConfigOverrides {
                    threshold: Some(threshold),
                    ..ConfigOverrides::default()
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains("finite percentage"));
        }
    }

    #[test]
    fn no_metrics_is_available_from_project_config_and_cli_override() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".cuke-dedup.json"),
            r#"{"noMetrics":true}"#,
        )
        .unwrap();
        let configured = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert!(configured.no_metrics);

        let overridden = Config::load(
            directory.path(),
            ConfigOverrides {
                no_metrics: Some(false),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        assert!(!overridden.no_metrics);
    }

    #[test]
    fn require_definitions_is_available_from_project_config() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".cuke-dedup.json"),
            r#"{"requireDefinitions":true}"#,
        )
        .unwrap();
        let configured = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert!(configured.require_definitions);
    }

    #[test]
    fn project_configuration_output_cannot_escape_the_analysis_root() {
        let directory = tempfile::tempdir().unwrap();
        for output in ["../outside", "nested/../../outside"] {
            fs::write(
                directory.path().join(".cuke-dedup.json"),
                format!(r#"{{"output":"{output}"}}"#),
            )
            .unwrap();
            let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
            assert!(error.to_string().contains("must stay beneath"), "{error:#}");
        }

        fs::write(
            directory.path().join(".cuke-dedup.json"),
            r#"{"output":"/tmp/cuke-dedup-project-controlled"}"#,
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("must stay beneath"), "{error:#}");
    }

    #[test]
    fn command_line_output_safely_overrides_an_invalid_project_value() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".cuke-dedup.json"),
            r#"{"output":"../../project-controlled"}"#,
        )
        .unwrap();
        let operator_output = directory.path().join("../operator-controlled");
        let config = Config::load(
            directory.path(),
            ConfigOverrides {
                output: Some(operator_output.clone()),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(config.output, operator_output);
    }

    #[cfg(unix)]
    #[test]
    fn project_configuration_output_cannot_follow_a_symlink_outside_the_root() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), directory.path().join("reports-link")).unwrap();
        fs::write(
            directory.path().join(".cuke-dedup.json"),
            r#"{"output":"reports-link"}"#,
        )
        .unwrap();

        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("resolves outside"), "{error:#}");
    }

    #[test]
    fn suppression_requires_a_reason() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cuke-dedup.config.json"),
            r#"{"suppressions":[{"rule":"duplicate-handler","path":"steps.ts","reason":""}]}"#,
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("non-empty reason"));
    }

    #[test]
    fn configured_excludes_extend_safety_defaults() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cuke-dedup.config.json"),
            r#"{"exclude":["dist/**"]}"#,
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert!(config.exclude.contains(&"**/node_modules/**".to_owned()));
        assert!(config.exclude.contains(&"dist/**".to_owned()));
    }

    #[test]
    fn exclude_layers_replace_custom_patterns_but_preserve_default_policy() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("package.json"),
            r#"{"cukeDedup":{"exclude":["package-only/**"]}}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join("cuke-dedup.config.json"),
            r#"{"exclude":["standalone-only/**"]}"#,
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert!(config.exclude.contains(&"**/node_modules/**".to_owned()));
        assert!(config.exclude.contains(&"standalone-only/**".to_owned()));
        assert!(!config.exclude.contains(&"package-only/**".to_owned()));

        let config = Config::load(
            directory.path(),
            ConfigOverrides {
                exclude: Some(vec!["cli-only/**".to_owned()]),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        assert!(config.exclude.contains(&"cli-only/**".to_owned()));
        assert!(!config.exclude.contains(&"standalone-only/**".to_owned()));
    }

    #[test]
    fn default_excludes_and_hidden_directories_have_explicit_escape_hatches() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cuke-dedup.config.json"),
            r#"{"excludeDefaults":false,"includeHidden":true}"#,
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert!(!config.exclude_defaults);
        assert!(config.exclude.is_empty());
        assert!(config.include_hidden);
    }

    #[test]
    fn jsonl_is_configurable_but_cannot_be_mixed_with_terminal_output() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".cuke-dedup.json"),
            r#"{"reporters":["jsonl","json"]}"#,
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(
            config.reporters,
            vec![ReporterKind::Jsonl, ReporterKind::Json]
        );

        fs::write(
            directory.path().join(".cuke-dedup.json"),
            r#"{"reporters":["terminal","jsonl"]}"#,
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error
            .to_string()
            .contains("terminal and jsonl reporters cannot share stdout"));
    }

    #[test]
    fn invalid_suppression_glob_is_rejected_during_config_load() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cuke-dedup.config.json"),
            r#"{"suppressions":[{"rule":"duplicate-handler","path":"[","reason":"test"}]}"#,
        )
        .unwrap();
        let error = Config::load(directory.path(), ConfigOverrides::default()).unwrap_err();
        assert!(error.to_string().contains("invalid suppression path glob"));
    }

    #[test]
    fn cucumber_config_supplies_defaults_below_cuke_dedup_configuration() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cucumber.json"),
            r#"{"default":{"paths":["acceptance"]}}"#,
        )
        .unwrap();
        let framework = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(
            framework.features,
            vec!["acceptance/**/*.{feature,feature.md}"]
        );
        assert!(framework.feature_pattern_origin.starts_with("Cucumber.js"));

        fs::write(
            directory.path().join("cuke-dedup.config.json"),
            r#"{"features":["custom/**/*.spec"]}"#,
        )
        .unwrap();
        let own_config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(own_config.features, vec!["custom/**/*.spec"]);
        assert_eq!(own_config.feature_pattern_origin, "cuke-dedup.config.json");

        let cli = Config::load(
            directory.path(),
            ConfigOverrides {
                features: Some(vec!["cli/**/*.feature".to_owned()]),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(cli.features, vec!["cli/**/*.feature"]);
        assert_eq!(cli.feature_pattern_origin, "--features");
    }

    #[test]
    fn playwright_bdd_literal_features_are_detected_without_executing_config() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("playwright.config.ts"),
            "const testDir = defineBddConfig({ features: ['specs/**/*.spec'] });",
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(config.features, vec!["specs/**/*.spec"]);
        assert!(config.feature_pattern_origin.starts_with("Playwright-BDD"));
    }

    #[test]
    fn cucumber_yaml_paths_are_detected() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cucumber.yml"),
            "default:\n  paths:\n    - acceptance\n    - docs/**/*.feature.md\n",
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(
            config.features,
            vec![
                "acceptance/**/*.{feature,feature.md}",
                "docs/**/*.feature.md"
            ]
        );
    }

    #[test]
    fn explicit_features_skip_invalid_lower_precedence_framework_config() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("cucumber.json"), "not json").unwrap();
        fs::write(
            directory.path().join("cuke-dedup.config.json"),
            r#"{"features":["custom/**/*.spec"]}"#,
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(config.features, vec!["custom/**/*.spec"]);
        assert!(config.config_warnings.is_empty());
    }

    #[test]
    fn jscpd_style_config_discovery_uses_one_source_in_documented_order() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join(".config")).unwrap();
        fs::write(
            directory.path().join("package.json"),
            r#"{"cukeDedup":{"threshold":10}}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join(".config/cuke-dedup.json"),
            r#"{"threshold":20}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join(".cuke-dedup.json"),
            r#"{"threshold":30}"#,
        )
        .unwrap();

        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(config.threshold, 30.0);
    }

    #[test]
    fn explicit_config_is_fatal_and_bypasses_auto_discovery() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".cuke-dedup.json"),
            r#"{"threshold":10}"#,
        )
        .unwrap();
        let explicit = directory.path().join("team-policy.json");
        fs::write(&explicit, r#"{"threshold":40}"#).unwrap();
        let config = Config::load(
            directory.path(),
            ConfigOverrides {
                config_file: Some(explicit),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(config.threshold, 40.0);

        let relative = Config::load(
            directory.path(),
            ConfigOverrides {
                config_file: Some(PathBuf::from("team-policy.json")),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(relative.threshold, 40.0);

        let error = Config::load(
            directory.path(),
            ConfigOverrides {
                config_file: Some(directory.path().join("missing.json")),
                ..ConfigOverrides::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("failed to read"));
    }

    #[test]
    fn malformed_auto_config_warns_and_falls_through_to_package_config() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(".cuke-dedup.json"), "not json").unwrap();
        fs::write(
            directory.path().join("package.json"),
            r#"{"cukeDedup":{"threshold":25}}"#,
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert_eq!(config.threshold, 25.0);
        assert_eq!(config.config_warnings.len(), 1);
    }
}
