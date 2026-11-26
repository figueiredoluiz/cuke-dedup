//! Terminal, JSON, JSON Lines, self-contained HTML, and SARIF reporters.

mod context;
mod html;
mod json;
mod jsonl;
mod sarif;
mod shared;
mod terminal;

pub(crate) use context::{CliReportMetadata, CorpusCensus};
pub use context::{ExecutionMetrics, ReportContext, Summary};
pub use html::render_html;
pub use json::render_json;
pub use jsonl::render_jsonl;
pub use sarif::render_sarif;
pub use terminal::write_terminal;

use crate::config::{unique_reporters, Config, ReporterKind};
use crate::model::AnalysisResult;
use crate::output_file::OutputFile;
use anyhow::{bail, Context, Result};
use shared::terminal_safe;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

/// Schema version written into JSON and embedded HTML report data.
pub const JSON_SCHEMA_VERSION: &str = "3";

/// Schema version written into each JSON Lines record.
pub const JSONL_SCHEMA_VERSION: &str = "3";

const JSON_REPORT_FILE: &str = "cuke-dedup.json";
const HTML_REPORT_FILE: &str = "cuke-dedup.html";
const SARIF_REPORT_FILE: &str = "cuke-dedup.sarif";

/// File names of the reports written inside the output directory.
pub(crate) const REPORT_FILE_NAMES: [&str; 3] =
    [JSON_REPORT_FILE, HTML_REPORT_FILE, SARIF_REPORT_FILE];

/// File report destinations bound before a file baseline can be updated.
#[derive(Default)]
pub(crate) struct ReportFiles(BTreeMap<&'static str, OutputFile>);

impl ReportFiles {
    /// Binds the selected reports and refuses any that name the baseline, including aliases.
    /// An empty reservation is removed on error unless another process replaced it.
    pub(crate) fn bind_baseline(config: &Config, baseline: Option<&Path>) -> Result<Self> {
        let mut files = Self::default();
        let Some(baseline) = baseline else {
            return Ok(files);
        };
        let baseline = config.root.join(baseline);
        for reporter in unique_reporters(&config.reporters) {
            let name = match reporter {
                ReporterKind::Json => JSON_REPORT_FILE,
                ReporterKind::Html => HTML_REPORT_FILE,
                ReporterKind::Sarif => SARIF_REPORT_FILE,
                ReporterKind::Terminal | ReporterKind::Jsonl => continue,
            };
            let path = config.output_path(name);
            let reserved = OutputFile::reserve(&path)?;
            let file = reserved.previous().expect("a reserved output has a file");
            match crate::contained_read::same_file_as(file, &baseline) {
                Ok(false) => {}
                Ok(true) => bail!(
                    "baseline {} would collide with report output {}; choose a different file",
                    baseline.display(),
                    path.display()
                ),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) => {}
                // fail-closed: an unreadable identity cannot establish that the paths are distinct.
                Err(error) => bail!(
                    "cannot verify that baseline {} is distinct from report output {}: {error}",
                    baseline.display(),
                    path.display()
                ),
            }
            files.0.insert(name, reserved);
        }
        Ok(files)
    }

    /// Uses the already verified binding, or opens the destination for a run without a baseline.
    fn take_or_open(&mut self, name: &'static str, path: &Path) -> Result<OutputFile> {
        self.0
            .remove(name)
            .map_or_else(|| OutputFile::open(path, false), Ok)
    }
}

/// Writes every configured report and returns the paths of file-based reports.
pub fn write_reports(
    context: &ReportContext<'_>,
    config: &Config,
    terminal: &mut dyn Write,
) -> Result<Vec<std::path::PathBuf>> {
    write_reports_context(
        context,
        config,
        None,
        terminal,
        false,
        ReportFiles::default(),
    )
}

/// Builds a report context from `result`, with `metrics` when given, and writes the configured
/// reports with CLI metadata and optional terminal color; returns the file-report paths.
pub(crate) fn write_cli_reports(
    result: &AnalysisResult,
    config: &Config,
    metrics: Option<&ExecutionMetrics>,
    metadata: &CliReportMetadata<'_>,
    terminal: &mut dyn Write,
    terminal_color: bool,
    report_files: ReportFiles,
) -> Result<Vec<std::path::PathBuf>> {
    let context = metrics.map_or_else(
        || ReportContext::new(result, &config.root, config.threshold),
        |metrics| ReportContext::with_metrics(result, &config.root, config.threshold, metrics),
    );
    write_reports_context(
        &context,
        config,
        Some(metadata),
        terminal,
        terminal_color,
        report_files,
    )
}

/// Writes each distinct configured reporter in first-selected order and returns the file-report paths.
///
/// Terminal and JSONL output go to `terminal`; JSON, HTML and SARIF go to `config.output_path`.
/// File paths are listed on `terminal` only when no terminal or JSONL reporter is configured.
/// Stops at the first error; reports already written stay on disk.
fn write_reports_context(
    context: &ReportContext<'_>,
    config: &Config,
    metadata: Option<&CliReportMetadata<'_>>,
    terminal: &mut dyn Write,
    terminal_color: bool,
    mut report_files: ReportFiles,
) -> Result<Vec<std::path::PathBuf>> {
    let mut written = Vec::new();
    let has_stdout_reporter = config
        .reporters
        .iter()
        .any(|reporter| matches!(reporter, ReporterKind::Terminal | ReporterKind::Jsonl));
    for reporter in unique_reporters(&config.reporters) {
        match reporter {
            ReporterKind::Terminal => {
                terminal::write_terminal_with_color(context, terminal, terminal_color)?
            }
            ReporterKind::Json => {
                let path = config.output_path(JSON_REPORT_FILE);
                let bytes = json::render_json_context_with_metadata(context, metadata)?;
                report_files
                    .take_or_open(JSON_REPORT_FILE, &path)?
                    .replace(bytes.as_bytes())
                    .with_context(|| format!("failed to write JSON report {}", path.display()))?;
                written.push(path);
            }
            ReporterKind::Jsonl => jsonl::write_jsonl_context(context, metadata, terminal)?,
            ReporterKind::Html => {
                let path = config.output_path(HTML_REPORT_FILE);
                let bytes = html::render_html_context_with_metadata(context, metadata)?;
                report_files
                    .take_or_open(HTML_REPORT_FILE, &path)?
                    .replace(bytes.as_bytes())
                    .with_context(|| format!("failed to write HTML report {}", path.display()))?;
                written.push(path);
            }
            ReporterKind::Sarif => {
                let path = config.output_path(SARIF_REPORT_FILE);
                let bytes = sarif::render_sarif_context_with_metadata(context, metadata)?;
                report_files
                    .take_or_open(SARIF_REPORT_FILE, &path)?
                    .replace(bytes.as_bytes())
                    .with_context(|| format!("failed to write SARIF report {}", path.display()))?;
                written.push(path);
            }
        }
    }
    if !has_stdout_reporter && !written.is_empty() {
        writeln!(terminal, "Reports written to:")?;
        for path in &written {
            let display = path.to_string_lossy().replace('\\', "/");
            writeln!(terminal, "  {}", terminal_safe(&display))?;
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests;
