//! Terminal, JSON, JSON Lines, self-contained HTML, and SARIF reporters.

mod context;
mod html;
mod json;
mod jsonl;
mod sarif;
mod shared;
mod terminal;

pub use context::{ExecutionMetrics, ReportContext, Summary};
pub use html::{render_html, render_html_with_threshold};
pub use json::{render_json, render_json_with_threshold};
pub use jsonl::{render_jsonl, render_jsonl_with_threshold};
pub use sarif::{render_sarif, render_sarif_with_threshold};
pub use terminal::{write_terminal, write_terminal_with_threshold};

use crate::config::{Config, ReporterKind};
use crate::model::AnalysisResult;
use anyhow::{Context, Result};
use shared::{ensure_parent, terminal_safe};
use std::fs;
use std::io::Write;

/// Schema version written into JSON and embedded HTML report data.
pub const JSON_SCHEMA_VERSION: &str = "1";

/// Schema version written into each JSON Lines record.
pub const JSONL_SCHEMA_VERSION: &str = "1";

/// Writes every configured report and returns the paths of file-based reports.
pub fn write_reports(
    result: &AnalysisResult,
    config: &Config,
    terminal: &mut dyn Write,
) -> Result<Vec<std::path::PathBuf>> {
    write_reports_context(
        &ReportContext::new(result, &config.root, config.threshold),
        config,
        terminal,
    )
}

/// Writes every configured report with measured CLI execution phases.
pub fn write_reports_with_metrics(
    result: &AnalysisResult,
    config: &Config,
    metrics: &ExecutionMetrics,
    terminal: &mut dyn Write,
) -> Result<Vec<std::path::PathBuf>> {
    write_reports_context(
        &ReportContext::with_metrics(result, &config.root, config.threshold, metrics),
        config,
        terminal,
    )
}

fn write_reports_context(
    context: &ReportContext<'_>,
    config: &Config,
    terminal: &mut dyn Write,
) -> Result<Vec<std::path::PathBuf>> {
    let mut written = Vec::new();
    let has_stdout_reporter = config
        .reporters
        .iter()
        .any(|reporter| matches!(reporter, ReporterKind::Terminal | ReporterKind::Jsonl));
    for reporter in &config.reporters {
        match reporter {
            ReporterKind::Terminal => terminal::write_terminal_context(context, terminal)?,
            ReporterKind::Json => {
                let path = config.output_path("cuke-dedup.json");
                ensure_parent(&path)?;
                fs::write(&path, json::render_json_context(context)?)
                    .with_context(|| format!("failed to write JSON report {}", path.display()))?;
                written.push(path);
            }
            ReporterKind::Jsonl => jsonl::write_jsonl_context(context, terminal)?,
            ReporterKind::Html => {
                let path = config.output_path("cuke-dedup.html");
                ensure_parent(&path)?;
                fs::write(&path, html::render_html_context(context)?)
                    .with_context(|| format!("failed to write HTML report {}", path.display()))?;
                written.push(path);
            }
            ReporterKind::Sarif => {
                let path = config.output_path("cuke-dedup.sarif");
                ensure_parent(&path)?;
                fs::write(&path, sarif::render_sarif_context(context)?)
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
