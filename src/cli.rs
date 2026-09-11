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

    /// Fail with exit code 2 when the corpus or comparison set is incomplete.
    #[arg(long, global = true)]
    fail_on_incomplete: bool,

    /// Fail with exit code 2 when no step definitions are extracted.
    #[arg(long, global = true)]
    require_definitions: bool,

    /// Maximum unique definition pairs retained, up to the hard safety ceiling.
    #[arg(long, global = true, value_name = "COUNT", value_parser = parse_positive_usize)]
    max_candidate_comparisons: Option<usize>,

    /// Maximum structural proposals per handler class, up to the hard safety ceiling.
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

struct Invocation {
    config: Config,
    options: CheckOptions,
}

#[derive(Default)]
struct Diagnostics {
    warnings: Vec<String>,
    errors: Vec<String>,
}

struct ExtractedCorpus {
    definitions: Vec<crate::model::StepDefinition>,
    feature_steps: Vec<crate::model::FeatureStep>,
    definition_files_with_definitions: usize,
    parsed_feature_files: usize,
    feature_files_without_steps: usize,
    incomplete: bool,
}

struct AnalyzedCorpus {
    result: crate::model::AnalysisResult,
    census: analysis::AnalysisCensus,
    definition_files_with_definitions: usize,
    parsed_feature_files: usize,
    feature_files_without_steps: usize,
    corpus_incomplete: bool,
    run_incomplete: bool,
}

struct PhaseTimings {
    discovery_ms: f64,
    parsing_ms: f64,
    analysis_ms: f64,
}

/// Parses the process arguments, runs the configured analysis, and returns its exit code.
pub fn run_cli() -> Result<i32> {
    execute(Cli::parse())
}

fn execute(cli: Cli) -> Result<i32> {
    let invocation = resolve_invocation(cli)?;
    let config = &invocation.config;
    let options = &invocation.options;
    if options.print_config {
        println!(
            "{}",
            serde_json::to_string_pretty(config)
                .context("failed to serialize effective configuration")?
        );
        return Ok(0);
    }
    let changed_files = resolve_changed_files(config, options.changed_since.as_deref())?;
    let discovery_started = Instant::now();
    let files = discovery::discover(config)?;
    let discovery_ms = elapsed_ms(discovery_started);

    let parsing_started = Instant::now();
    let mut diagnostics = discovery_diagnostics(config, &files, &changed_files);
    let extracted = extract_corpus(config, &files, &changed_files, &mut diagnostics);
    let parsing_ms = elapsed_ms(parsing_started);
    let analysis_started = Instant::now();
    let mut analyzed = analyze_corpus(config, extracted, &mut diagnostics)?;
    let analysis_ms = elapsed_ms(analysis_started);
    apply_finding_modes(
        &mut analyzed.result,
        &files,
        &changed_files,
        analyzed.parsed_feature_files,
        analyzed.feature_files_without_steps,
    );
    let baseline_outcome = apply_baseline_mode(
        config,
        options,
        &mut analyzed.result,
        analyzed.run_incomplete,
        &mut diagnostics,
    )?;
    write_reports(
        config,
        &files,
        &analyzed,
        &diagnostics,
        PhaseTimings {
            discovery_ms,
            parsing_ms,
            analysis_ms,
        },
    )?;
    write_diagnostics(config, options, &files, &diagnostics)?;
    determine_exit_code(
        config,
        options,
        &analyzed.result,
        baseline_outcome,
        &diagnostics,
    )
}

fn resolve_invocation(cli: Cli) -> Result<Invocation> {
    if cli.command.is_some() && cli.path.as_path() != Path::new(".") {
        bail!("do not place a path before `check`; use `cuke-dedup check <PATH>`");
    }
    let root = match cli.command {
        Some(Command::Check { path }) => path,
        None => cli.path,
    };
    let options = cli.options.clone();
    let config = Config::load_for_cli(
        &root,
        ConfigOverrides {
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
        },
        CliConfigOverrides {
            require_definitions: cli.options.require_definitions.then_some(true),
            fail_on_incomplete: cli.options.fail_on_incomplete.then_some(true),
            max_candidate_comparisons: cli.options.max_candidate_comparisons,
            max_structural_class_comparisons: cli.options.max_structural_class_comparisons,
        },
    )?;
    Ok(Invocation { config, options })
}

fn resolve_changed_files(
    config: &Config,
    changed_since: Option<&str>,
) -> Result<Option<BTreeSet<PathBuf>>> {
    let Some(base) = changed_since else {
        return Ok(None);
    };
    modes::ensure_changed_root_is_trackable(&config.root)?;
    Ok(Some(modes::git_changed_files(&config.root, base)?))
}

fn discovery_diagnostics(
    config: &Config,
    files: &discovery::DiscoveredFiles,
    changed_files: &Option<BTreeSet<PathBuf>>,
) -> Diagnostics {
    let mut diagnostics = Diagnostics {
        warnings: config.config_warnings.clone(),
        errors: files.errors.clone(),
    };
    for pattern in &files.unmatched_feature_patterns {
        diagnostics.warnings.push(format!(
            "feature pattern `{pattern}` from {} matched no files",
            config.feature_pattern_origin
        ));
    }
    for pattern in &files.unmatched_definition_patterns {
        diagnostics
            .warnings
            .push(format!("definition pattern `{pattern}` matched no files"));
    }
    if files.definitions.is_empty() && config.definitions.is_empty() {
        diagnostics
            .warnings
            .push("automatic discovery found no supported step-definition source files".to_owned());
    }
    if files.features.is_empty() {
        let message = format!(
            "no feature files matched patterns from {}; unused-definition findings are disabled for this incomplete corpus",
            config.feature_pattern_origin
        );
        if config.require_features {
            diagnostics.errors.push(message);
        } else if !files.definitions.is_empty() {
            diagnostics.warnings.push(message);
        }
    }
    if changed_files.as_ref().is_some_and(BTreeSet::is_empty) {
        diagnostics.warnings.push(
            "--changed-since found no tracked or untracked files; no findings will be reported"
                .to_owned(),
        );
    }
    diagnostics
}

fn extract_corpus(
    config: &Config,
    files: &discovery::DiscoveredFiles,
    changed_files: &Option<BTreeSet<PathBuf>>,
    diagnostics: &mut Diagnostics,
) -> ExtractedCorpus {
    let mut corpus = ExtractedCorpus {
        definitions: Vec::new(),
        feature_steps: Vec::new(),
        definition_files_with_definitions: 0,
        parsed_feature_files: 0,
        feature_files_without_steps: 0,
        incomplete: false,
    };
    extract_definitions(config, files, changed_files, diagnostics, &mut corpus);
    extract_feature_steps(config, files, diagnostics, &mut corpus);
    corpus
}

fn extract_definitions(
    config: &Config,
    files: &discovery::DiscoveredFiles,
    changed_files: &Option<BTreeSet<PathBuf>>,
    diagnostics: &mut Diagnostics,
    corpus: &mut ExtractedCorpus,
) {
    let mut extraction_session = source_adapter::SourceExtractionSession::with_registrations(
        &config.root,
        &config.registrations,
    );
    for file in &files.definitions {
        let adapter = source_adapter::adapter_for_language(file.language);
        match adapter
            .extract_file_with_session(file, &mut extraction_session)
            .with_context(|| format!("failed to analyze {}", file.path.display()))
        {
            Ok(extracted) => {
                if !extracted.definitions.is_empty() {
                    corpus.definition_files_with_definitions += 1;
                }
                corpus.definitions.extend(extracted.definitions);
                collect_extraction_diagnostics(
                    config,
                    file,
                    changed_files,
                    extracted.diagnostics,
                    diagnostics,
                    &mut corpus.incomplete,
                );
            }
            // Every discovered source contributes to cross-file definition comparisons, even
            // when changed mode later filters findings. Skipping an unreadable source could turn
            // an incomplete run into a false pass.
            Err(error) => diagnostics.errors.push(format!("{error:#}")),
        }
    }
    corpus.definitions.sort_by(|left, right| {
        left.location
            .path
            .cmp(&right.location.path)
            .then(left.location.line.cmp(&right.location.line))
            .then(left.location.column.cmp(&right.location.column))
    });
    if corpus.definitions.is_empty() {
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
            diagnostics.errors.push(message);
        } else if !files.definitions.is_empty() {
            diagnostics.warnings.push(message);
        }
    }
}

fn collect_extraction_diagnostics(
    config: &Config,
    file: &source_adapter::SourceFile,
    changed_files: &Option<BTreeSet<PathBuf>>,
    extracted: Vec<source_adapter::ExtractionDiagnostic>,
    diagnostics: &mut Diagnostics,
    corpus_incomplete: &mut bool,
) {
    let changed_or_full_run = changed_files
        .as_ref()
        .is_none_or(|changed| changed.contains(&file.path));
    for diagnostic in extracted {
        let message = format!(
            "{}: {}",
            diagnostic.location.display(&config.root),
            diagnostic.message
        );
        let completeness = source_adapter::is_completeness_diagnostic(&diagnostic);
        // An unresolved registration import hides every definition it would have introduced, so
        // the corpus is incomplete regardless of which files changed.
        *corpus_incomplete |= completeness;
        match diagnostic.level {
            ExtractionDiagnosticLevel::Warning if changed_or_full_run || completeness => {
                diagnostics.warnings.push(message);
            }
            ExtractionDiagnosticLevel::Warning => {}
            // Extraction errors can mean definitions were omitted. They remain fatal for
            // unchanged files because those definitions still participate in changed-file pairs.
            ExtractionDiagnosticLevel::Error => diagnostics.errors.push(message),
        }
    }
}

fn extract_feature_steps(
    config: &Config,
    files: &discovery::DiscoveredFiles,
    diagnostics: &mut Diagnostics,
    corpus: &mut ExtractedCorpus,
) {
    for file in &files.features {
        match gherkin::extract_file_with_format(&file.path, file.format) {
            Ok(extracted) => {
                corpus.parsed_feature_files += 1;
                if extracted.is_empty()
                    && file.format == crate::gherkin::FeatureFormat::GherkinMarkdown
                {
                    corpus.feature_files_without_steps += 1;
                    corpus.incomplete = true;
                    let display = file
                        .path
                        .strip_prefix(&config.root)
                        .unwrap_or(&file.path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    diagnostics.warnings.push(format!(
                        "Gherkin Markdown file {display} parsed successfully but produced 0 feature steps; unused-definition findings are disabled for this incomplete corpus"
                    ));
                } else {
                    corpus.feature_steps.extend(extracted);
                }
            }
            // Changed definitions still depend on the full feature corpus for usage and ambiguity.
            Err(error) => diagnostics.errors.push(format!("{error:#}")),
        }
    }
    if !files.features.is_empty() && corpus.parsed_feature_files < files.features.len() {
        let message = if corpus.parsed_feature_files == 0 {
            "no discovered feature file was parsed successfully; unused-definition findings are disabled for this incomplete corpus".to_owned()
        } else {
            format!(
                "{} discovered feature file(s) could not be parsed; unused-definition findings are disabled for this incomplete corpus",
                files.features.len() - corpus.parsed_feature_files
            )
        };
        if config.require_features {
            diagnostics.errors.push(message);
        } else if !files.definitions.is_empty() {
            diagnostics.warnings.push(message);
        }
    }
    corpus.feature_steps.sort_by(|left, right| {
        left.location
            .path
            .cmp(&right.location.path)
            .then(left.location.line.cmp(&right.location.line))
            .then(left.location.column.cmp(&right.location.column))
    });
}

fn analyze_corpus(
    config: &Config,
    extracted: ExtractedCorpus,
    diagnostics: &mut Diagnostics,
) -> Result<AnalyzedCorpus> {
    let ExtractedCorpus {
        definitions,
        feature_steps,
        definition_files_with_definitions,
        parsed_feature_files,
        feature_files_without_steps,
        incomplete: corpus_incomplete,
    } = extracted;
    let (analysis, census, unmatched_suppressions) =
        analysis::analyze_for_cli(definitions, feature_steps, config)?;
    for index in unmatched_suppressions.indices {
        let suppression = &config.suppressions[index];
        diagnostics.warnings.push(format!(
            "suppression {} for {} matched no step definitions",
            index + 1,
            suppression.rule
        ));
    }
    if unmatched_suppressions.truncated {
        diagnostics.warnings.push(
            "unmatched suppression validation stopped after its safety limit; analysis findings are unaffected"
                .to_owned(),
        );
    }

    // Bounded work that could not finish makes findings a subset of a complete run, but every
    // reported finding remains valid. `--fail-on-incomplete` restores strict CI gating.
    let run_incomplete = corpus_incomplete || !analysis.incomplete.is_empty();
    if config.fail_on_incomplete {
        diagnostics.errors.extend(analysis.incomplete);
        if corpus_incomplete {
            diagnostics.errors.push(
                "input extraction did not fully represent every discovered definition or feature file, so the analyzed corpus is incomplete".to_owned(),
            );
        }
    } else {
        diagnostics.warnings.extend(analysis.incomplete);
    }
    Ok(AnalyzedCorpus {
        result: analysis.result,
        census,
        definition_files_with_definitions,
        parsed_feature_files,
        feature_files_without_steps,
        corpus_incomplete,
        run_incomplete,
    })
}

fn apply_finding_modes(
    result: &mut crate::model::AnalysisResult,
    files: &discovery::DiscoveredFiles,
    changed_files: &Option<BTreeSet<PathBuf>>,
    parsed_feature_files: usize,
    feature_files_without_steps: usize,
) {
    // A converter can produce a syntactically valid but empty document when it fails to
    // recognize markup or a dialect. Such an input cannot prove that a definition is unused,
    // so use the same conservative suppression as a feature parse failure.
    if parsed_feature_files < files.features.len()
        || feature_files_without_steps > 0
        || files.features.is_empty()
    {
        result
            .findings
            .retain(|finding| finding.rule != Rule::UnusedDefinition);
    }
    if let Some(changed) = changed_files {
        modes::retain_changed_findings(&mut result.findings, changed);
    }
}

fn apply_baseline_mode(
    config: &Config,
    options: &CheckOptions,
    result: &mut crate::model::AnalysisResult,
    run_incomplete: bool,
    diagnostics: &mut Diagnostics,
) -> Result<Option<modes::BaselineOutcome>> {
    let Some(path) = &options.baseline else {
        return Ok(None);
    };
    let path = if path.is_absolute() {
        path.clone()
    } else {
        config.root.join(path)
    };
    if options.update_baseline && (run_incomplete || !diagnostics.errors.is_empty()) {
        diagnostics.warnings.push(format!(
            "baseline {} was not updated because analysis is incomplete",
            path.display()
        ));
        return Ok(None);
    }
    let outcome = if options.update_baseline {
        modes::update_baseline(&mut result.findings, &path)?
    } else {
        modes::apply_baseline(&mut result.findings, &path)?
    };
    if options.update_baseline {
        diagnostics.warnings.push(format!(
            "updated baseline {}: {} added, {} removed, {} total",
            path.display(),
            outcome.added,
            outcome.removed,
            outcome.suppressed
        ));
    }
    Ok(Some(outcome))
}

fn write_reports(
    config: &Config,
    files: &discovery::DiscoveredFiles,
    analyzed: &AnalyzedCorpus,
    diagnostics: &Diagnostics,
    timings: PhaseTimings,
) -> Result<()> {
    let corpus = reporters::CorpusCensus {
        definition_files: files.definitions.len(),
        definition_files_with_definitions: analyzed.definition_files_with_definitions,
        definitions_extracted: analyzed.result.definitions.len(),
        feature_files: files.features.len(),
        feature_files_parsed: analyzed.parsed_feature_files,
        feature_files_without_steps: analyzed.feature_files_without_steps,
        incomplete: analyzed.corpus_incomplete,
    };
    let metrics = reporters::ExecutionMetrics::new(
        files.definitions.len(),
        files.features.len(),
        timings.discovery_ms,
        timings.parsing_ms,
        timings.analysis_ms,
    );
    let report_metadata = reporters::CliReportMetadata {
        corpus: &corpus,
        analysis: &analyzed.census,
        // SARIF consumers treat a successful invocation as proof the tool covered its input.
        execution_successful: diagnostics.errors.is_empty() && !analyzed.run_incomplete,
    };
    let mut stdout = io::stdout().lock();
    reporters::write_cli_reports(
        &analyzed.result,
        config,
        (!config.no_metrics).then_some(&metrics),
        &report_metadata,
        &mut stdout,
    )?;
    Ok(())
}

fn write_diagnostics(
    config: &Config,
    options: &CheckOptions,
    files: &discovery::DiscoveredFiles,
    diagnostics: &Diagnostics,
) -> Result<()> {
    if !options.explain_discovery
        && diagnostics.warnings.is_empty()
        && diagnostics.errors.is_empty()
    {
        return Ok(());
    }
    let mut stderr = io::stderr().lock();
    if options.explain_discovery {
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
    for warning in &diagnostics.warnings {
        writeln!(stderr, "cuke-dedup: warning: {warning}")?;
    }
    for error in &diagnostics.errors {
        writeln!(stderr, "cuke-dedup: {error}")?;
    }
    Ok(())
}

fn determine_exit_code(
    config: &Config,
    options: &CheckOptions,
    result: &crate::model::AnalysisResult,
    baseline: Option<modes::BaselineOutcome>,
    diagnostics: &Diagnostics,
) -> Result<i32> {
    if !diagnostics.errors.is_empty() {
        return Ok(2);
    }
    let duplication = DuplicationThreshold::from_result(result, config.threshold);
    let has_non_duplication_errors = result.findings.iter().any(|finding| {
        finding.is_active()
            && finding.severity == Severity::Error
            && !finding.rule.contributes_to_duplication_threshold()
    });
    let new_findings_exceeded = options
        .fail_on_new
        .is_some_and(|limit| baseline.is_some_and(|outcome| outcome.new_findings > limit));
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
