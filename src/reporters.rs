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

use crate::config::{Config, ReporterKind};
use crate::model::AnalysisResult;
use anyhow::{Context, Result};
use shared::{ensure_parent, terminal_safe};
use std::fs;
use std::io::Write;

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

/// Writes every configured report and returns the paths of file-based reports.
pub fn write_reports(
    context: &ReportContext<'_>,
    config: &Config,
    terminal: &mut dyn Write,
) -> Result<Vec<std::path::PathBuf>> {
    write_reports_context(context, config, None, terminal, false)
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
) -> Result<Vec<std::path::PathBuf>> {
    let context = metrics.map_or_else(
        || ReportContext::new(result, &config.root, config.threshold),
        |metrics| ReportContext::with_metrics(result, &config.root, config.threshold, metrics),
    );
    write_reports_context(&context, config, Some(metadata), terminal, terminal_color)
}

/// Writes each configured reporter in order and returns the file-report paths.
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
) -> Result<Vec<std::path::PathBuf>> {
    let mut written = Vec::new();
    let has_stdout_reporter = config
        .reporters
        .iter()
        .any(|reporter| matches!(reporter, ReporterKind::Terminal | ReporterKind::Jsonl));
    for reporter in &config.reporters {
        match reporter {
            ReporterKind::Terminal => {
                terminal::write_terminal_with_color(context, terminal, terminal_color)?
            }
            ReporterKind::Json => {
                let path = config.output_path(JSON_REPORT_FILE);
                ensure_parent(&path)?;
                fs::write(
                    &path,
                    json::render_json_context_with_metadata(context, metadata)?,
                )
                .with_context(|| format!("failed to write JSON report {}", path.display()))?;
                written.push(path);
            }
            ReporterKind::Jsonl => jsonl::write_jsonl_context(context, metadata, terminal)?,
            ReporterKind::Html => {
                let path = config.output_path(HTML_REPORT_FILE);
                ensure_parent(&path)?;
                fs::write(
                    &path,
                    html::render_html_context_with_metadata(context, metadata)?,
                )
                .with_context(|| format!("failed to write HTML report {}", path.display()))?;
                written.push(path);
            }
            ReporterKind::Sarif => {
                let path = config.output_path(SARIF_REPORT_FILE);
                ensure_parent(&path)?;
                fs::write(
                    &path,
                    sarif::render_sarif_context_with_metadata(context, metadata)?,
                )
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
