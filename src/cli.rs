//! Command-line parsing and analysis orchestration.

use crate::analysis;
use crate::config::{CliConfigOverrides, Config, ConfigOverrides, ReporterKind};
use crate::discovery;
use crate::model::{DuplicationThreshold, Rule, Severity};
use crate::source_adapter::ExtractionDiagnosticLevel;
use crate::{gherkin, modes, reporters, source_adapter};
use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Debug, Parser)]
#[command(
    name = "cuke-dedup",
    version,
    about = "Detect duplicate and reusable Cucumber step definitions",
    after_help = "Examples:\n  cuke-dedup .\n  cuke-dedup check .\n  cuke-dedup . --reporters terminal,json,html\n  cuke-dedup . --reporters jsonl"
)]
struct Cli {
    /// Directory to analyze when no explicit command is used.
    #[arg(value_name = "PATH", default_value = ".")]
    path: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    options: CheckOptions,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Analyze a repository for duplicate and reusable step definitions.
    Check {
        /// Directory to analyze.
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
}

#[derive(Debug, Clone, Args, Default)]
struct CheckOptions {
    /// Explicit CukeDedup JSON configuration file.
    #[arg(long, short = 'c', global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Reporters to run (terminal, json, jsonl, html, sarif).
    #[arg(long, global = true, value_delimiter = ',')]
    reporters: Option<Vec<ReporterKind>>,

    /// Directory in which file-based reports are written.
    #[arg(long, global = true, value_name = "DIR")]
    output: Option<PathBuf>,

    /// Maximum percentage of definitions allowed in active duplication errors.
    #[arg(
        long,
        global = true,
        value_name = "PERCENT",
        allow_hyphen_values = true
    )]
    threshold: Option<f64>,

    /// Fail with exit code 2 when no feature files are discovered.
    #[arg(long, global = true)]
    require_features: bool,

    /// Fail with exit code 2 when no step definitions are extracted.
    #[arg(long, global = true)]
    require_definitions: bool,

    /// Maximum unique definition pairs retained for analysis.
    #[arg(long, global = true, value_name = "COUNT", value_parser = parse_positive_usize)]
    max_candidate_comparisons: Option<usize>,

    /// Maximum structural candidate proposals considered per handler class.
    #[arg(long, global = true, value_name = "COUNT", value_parser = parse_positive_usize)]
    max_structural_class_comparisons: Option<usize>,

    /// Glob patterns selecting step-definition source files.
    #[arg(long, global = true, value_delimiter = ',')]
    definitions: Option<Vec<String>>,

    /// Glob patterns selecting Gherkin feature files.
    #[arg(long, global = true, value_delimiter = ',')]
    features: Option<Vec<String>>,

    /// Print the effective feature source and every discovered input.
    #[arg(long, global = true)]
    explain_discovery: bool,

    /// Print the merged effective configuration as JSON and exit.
    #[arg(long, global = true)]
    print_config: bool,

    /// Omit runtime timing metrics from machine reports for reproducible output.
    #[arg(long, global = true)]
    no_metrics: bool,

    /// Glob patterns excluded from discovery.
    #[arg(long, global = true, value_delimiter = ',')]
    exclude: Option<Vec<String>>,

    /// Disable built-in exclusions such as node_modules and target.
    #[arg(long, global = true)]
    no_default_excludes: bool,

    /// Descend into dot-directories such as .steps.
    #[arg(long, global = true)]
    include_hidden: bool,

    /// Override a rule severity, for example duplicate-matcher=warning.
    #[arg(long = "rule", global = true, value_name = "RULE=SEVERITY", value_parser = parse_rule_override)]
    rules: Vec<(Rule, Severity)>,

    /// Report only findings involving files changed since this Git revision.
    #[arg(long, global = true, value_name = "GIT_REF")]
    changed_since: Option<String>,

    /// Suppress findings already present in a versioned semantic baseline.
    #[arg(long, global = true, value_name = "BASELINE.json")]
    baseline: Option<PathBuf>,

    /// Rewrite the configured baseline from the current complete analysis.
    #[arg(
        long,
        global = true,
        requires = "baseline",
        conflicts_with = "changed_since"
    )]
    update_baseline: bool,

    /// Fail when more than COUNT findings are new relative to the baseline.
    #[arg(
        long,
        global = true,
        value_name = "COUNT",
        num_args = 0..=1,
        default_missing_value = "0",
        requires = "baseline",
        conflicts_with = "update_baseline"
    )]
    fail_on_new: Option<usize>,
}

/// Parses the process arguments, runs the configured analysis, and returns its exit code.
pub fn run_cli() -> Result<i32> {
    execute(Cli::parse())
}

fn execute(cli: Cli) -> Result<i32> {
    if cli.command.is_some() && cli.path.as_path() != Path::new(".") {
        bail!("do not place a path before `check`; use `cuke-dedup check <PATH>`");
    }
    let root = match cli.command {
        Some(Command::Check { path }) => path,
        None => cli.path,
    };
    let overrides = ConfigOverrides {
        config_file: cli.options.config,
        definitions: cli.options.definitions,
        features: cli.options.features,
        exclude: cli.options.exclude,
        exclude_defaults: cli.options.no_default_excludes.then_some(false),
        include_hidden: cli.options.include_hidden.then_some(true),
        reporters: cli.options.reporters,
        output: cli.options.output,
        threshold: cli.options.threshold,
        require_features: cli.options.require_features.then_some(true),
        no_metrics: cli.options.no_metrics.then_some(true),
        rules: cli.options.rules.into_iter().collect::<BTreeMap<_, _>>(),
    };
    let config = Config::load_for_cli(
        &root,
        overrides,
        CliConfigOverrides {
            require_definitions: cli.options.require_definitions.then_some(true),
            max_candidate_comparisons: cli.options.max_candidate_comparisons,
            max_structural_class_comparisons: cli.options.max_structural_class_comparisons,
        },
    )?;
    if cli.options.print_config {
        println!(
            "{}",
            serde_json::to_string_pretty(&config)
                .context("failed to serialize effective configuration")?
        );
        return Ok(0);
    }
    let changed_files = if let Some(base) = cli.options.changed_since.as_deref() {
        modes::ensure_changed_root_is_trackable(&config.root)?;
        Some(modes::git_changed_files(&config.root, base)?)
    } else {
        None
    };
    let discovery_started = Instant::now();
    let files = discovery::discover(&config)?;
    let discovery_ms = elapsed_ms(discovery_started);

    let parsing_started = Instant::now();
    let mut definitions = Vec::new();
    let mut definition_files_with_definitions = 0_usize;
    let mut operational_errors = files.errors.clone();
    let mut operational_warnings = config.config_warnings.clone();
    for pattern in &files.unmatched_feature_patterns {
        operational_warnings.push(format!(
            "feature pattern `{pattern}` from {} matched no files",
            config.feature_pattern_origin
        ));
    }
    for pattern in &files.unmatched_definition_patterns {
        operational_warnings.push(format!("definition pattern `{pattern}` matched no files"));
    }
    if files.definitions.is_empty() && config.definitions.is_empty() {
        operational_warnings
            .push("automatic discovery found no supported step-definition source files".to_owned());
    }
    if files.features.is_empty() {
        let message = format!(
            "no feature files matched patterns from {}; unused-definition findings are disabled for this incomplete corpus",
            config.feature_pattern_origin
        );
        if config.require_features {
            operational_errors.push(message);
        } else if !files.definitions.is_empty() {
            operational_warnings.push(message);
        }
    }
    if changed_files.as_ref().is_some_and(BTreeSet::is_empty) {
        operational_warnings.push(
            "--changed-since found no tracked or untracked files; no findings will be reported"
                .to_owned(),
        );
    }
    let mut extraction_session = source_adapter::SourceExtractionSession::new(&config.root);
    for file in &files.definitions {
        let adapter = source_adapter::adapter_for_language(file.language);
        match adapter
            .extract_file_with_session(file, &mut extraction_session)
            .with_context(|| format!("failed to analyze {}", file.path.display()))
        {
            Ok(extracted) => {
                if !extracted.definitions.is_empty() {
                    definition_files_with_definitions += 1;
                }
                definitions.extend(extracted.definitions);
                let changed_or_full_run = changed_files
                    .as_ref()
                    .is_none_or(|changed| changed.contains(&file.path));
                for diagnostic in extracted.diagnostics {
                    let message = format!(
                        "{}: {}",
                        diagnostic.location.display(&config.root),
                        diagnostic.message
                    );
                    match diagnostic.level {
                        ExtractionDiagnosticLevel::Warning
                            if changed_or_full_run
                                || source_adapter::is_completeness_diagnostic(&diagnostic) =>
                        {
                            operational_warnings.push(message)
                        }
                        ExtractionDiagnosticLevel::Warning => {}
                        // Extraction errors can mean definitions were omitted. They remain fatal
                        // for unchanged files because those definitions still participate in
                        // comparisons involving changed files.
                        ExtractionDiagnosticLevel::Error => operational_errors.push(message),
                    }
                }
            }
            // Every discovered source contributes to cross-file definition comparisons, even
            // when changed mode later filters the findings. Skipping any unreadable source could
            // therefore turn an incomplete run into a false pass.
            Err(error) => {
                operational_errors.push(format!("{error:#}"));
            }
        }
    }
    definitions.sort_by(|left, right| {
        left.location
            .path
            .cmp(&right.location.path)
            .then(left.location.line.cmp(&right.location.line))
            .then(left.location.column.cmp(&right.location.column))
    });
    if definitions.is_empty() {
        let message = if files.definitions.is_empty() {
            "no step definitions were extracted because no definition source files were discovered"
                .to_owned()
        } else {
            format!(
                "definition extraction produced 0 definitions from {} discovered definition source file(s)",
                files.definitions.len()
            )
        };
        if config.require_definitions {
            operational_errors.push(message);
        } else if !files.definitions.is_empty() {
            operational_warnings.push(message);
        }
    }
    for index in analysis::unmatched_suppressions(&config, &definitions) {
        let suppression = &config.suppressions[index];
        operational_warnings.push(format!(
            "suppression {} for {} matched no step definitions",
            index + 1,
            suppression.rule
        ));
    }

    let mut feature_steps = Vec::new();
    let mut parsed_feature_files = 0_usize;
    for file in &files.features {
        match gherkin::extract_file_with_format(&file.path, file.format) {
            Ok(extracted) => {
                parsed_feature_files += 1;
                feature_steps.extend(extracted);
            }
            // Changed definitions are still compared against the complete feature corpus. Any
            // skipped feature can hide usage or ambiguity involving those definitions.
            Err(error) => {
                operational_errors.push(format!("{error:#}"));
            }
        }
    }
    if !files.features.is_empty() && parsed_feature_files < files.features.len() {
        let message = if parsed_feature_files == 0 {
            "no discovered feature file was parsed successfully; unused-definition findings are disabled for this incomplete corpus".to_owned()
        } else {
            format!(
                "{} discovered feature file(s) could not be parsed; unused-definition findings are disabled for this incomplete corpus",
                files.features.len() - parsed_feature_files
            )
        };
        if config.require_features {
            operational_errors.push(message);
        } else if !files.definitions.is_empty() {
            operational_warnings.push(message);
        }
    }
    feature_steps.sort_by(|left, right| {
        left.location
            .path
            .cmp(&right.location.path)
            .then(left.location.line.cmp(&right.location.line))
            .then(left.location.column.cmp(&right.location.column))
    });
    let parsing_ms = elapsed_ms(parsing_started);

    let analysis_started = Instant::now();
    let (analysis, analysis_census) =
        analysis::analyze_for_cli(definitions, feature_steps, &config)?;
    operational_errors.extend(analysis.operational_errors);
    let mut result = analysis.result;
    let analysis_ms = elapsed_ms(analysis_started);
    if parsed_feature_files < files.features.len() || files.features.is_empty() {
        result
            .findings
            .retain(|finding| finding.rule != Rule::UnusedDefinition);
    }
    if let Some(changed) = &changed_files {
        modes::retain_changed_findings(&mut result.findings, changed);
    }
    let mut baseline_outcome = None;
    if let Some(path) = &cli.options.baseline {
        let path = if path.is_absolute() {
            path.clone()
        } else {
            config.root.join(path)
        };
        if cli.options.update_baseline && !operational_errors.is_empty() {
            operational_warnings.push(format!(
                "baseline {} was not updated because analysis is incomplete",
                path.display()
            ));
        } else {
            let outcome = if cli.options.update_baseline {
                modes::update_baseline(&mut result.findings, &path)?
            } else {
                modes::apply_baseline(&mut result.findings, &path)?
            };
            if cli.options.update_baseline {
                operational_warnings.push(format!(
                    "updated baseline {}: {} added, {} removed, {} total",
                    path.display(),
                    outcome.added,
                    outcome.removed,
                    outcome.suppressed
                ));
            }
            baseline_outcome = Some(outcome);
        }
    }
    let corpus = reporters::CorpusCensus {
        definition_files: files.definitions.len(),
        definition_files_with_definitions,
        definitions_extracted: result.definitions.len(),
        feature_files: files.features.len(),
        feature_files_parsed: parsed_feature_files,
    };
    let metrics = reporters::ExecutionMetrics {
        definition_files: files.definitions.len(),
        feature_files: files.features.len(),
        files_discovered: files.definitions.len() + files.features.len(),
        discovery_ms,
        parsing_ms,
        analysis_ms,
    };
    let report_metadata = reporters::CliReportMetadata {
        corpus: &corpus,
        analysis: &analysis_census,
        execution_successful: operational_errors.is_empty(),
    };
    let mut stdout = io::stdout().lock();
    reporters::write_cli_reports(
        &result,
        &config,
        (!config.no_metrics).then_some(&metrics),
        &report_metadata,
        &mut stdout,
    )?;
    if cli.options.explain_discovery
        || !operational_warnings.is_empty()
        || !operational_errors.is_empty()
    {
        let mut stderr = io::stderr().lock();
        if cli.options.explain_discovery {
            writeln!(
                stderr,
                "cuke-dedup: feature patterns: {} ({})",
                config.features.join(", "),
                config.feature_pattern_origin
            )?;
            for file in &files.features {
                let relative = file.path.strip_prefix(&config.root).unwrap_or(&file.path);
                writeln!(
                    stderr,
                    "cuke-dedup: feature {} [{}] via {}",
                    relative.display(),
                    file.format.as_str(),
                    file.pattern
                )?;
            }
            for file in &files.definitions {
                let relative = file.path.strip_prefix(&config.root).unwrap_or(&file.path);
                writeln!(stderr, "cuke-dedup: definition {}", relative.display())?;
            }
        }
        for warning in operational_warnings {
            writeln!(stderr, "cuke-dedup: warning: {warning}")?;
        }
        for error in &operational_errors {
            writeln!(stderr, "cuke-dedup: {error}")?;
        }
    }
    if !operational_errors.is_empty() {
        return Ok(2);
    }
    let duplication = DuplicationThreshold::from_result(&result, config.threshold);
    let has_non_duplication_errors = result.findings.iter().any(|finding| {
        finding.is_active()
            && finding.severity == Severity::Error
            && !finding.rule.contributes_to_duplication_threshold()
    });
    let new_findings_exceeded = cli
        .options
        .fail_on_new
        .is_some_and(|limit| baseline_outcome.is_some_and(|outcome| outcome.new_findings > limit));
    Ok(i32::from(
        has_non_duplication_errors || !duplication.passed || new_findings_exceeded,
    ))
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

fn parse_rule_override(value: &str) -> std::result::Result<(Rule, Severity), String> {
    let (rule, severity) = value
        .split_once('=')
        .ok_or_else(|| "expected RULE=SEVERITY".to_owned())?;
    Ok((rule.parse()?, severity.parse()?))
}

fn parse_positive_usize(value: &str) -> std::result::Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| "expected an integer greater than zero".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rule_override() {
        assert_eq!(
            parse_rule_override("duplicate-matcher=warning"),
            Ok((Rule::DuplicateMatcher, Severity::Warning))
        );
        assert!(parse_rule_override("unknown=error").is_err());
    }

    #[test]
    fn candidate_limit_cli_values_must_be_positive() {
        assert_eq!(parse_positive_usize("7"), Ok(7));
        assert!(parse_positive_usize("0").is_err());
        assert!(parse_positive_usize("-1").is_err());
        assert!(parse_positive_usize("many").is_err());
    }

    #[test]
    fn supports_implicit_and_explicit_check_forms() {
        let implicit = Cli::try_parse_from(["cuke-dedup", ".", "--reporters", "json"])
            .expect("implicit invocation");
        assert!(implicit.command.is_none());
        let explicit = Cli::try_parse_from(["cuke-dedup", "check", ".", "--reporters", "json"])
            .expect("explicit invocation");
        assert!(matches!(explicit.command, Some(Command::Check { .. })));
        assert!(Cli::try_parse_from(["cuke-dedup", "check"]).is_err());
    }
}
