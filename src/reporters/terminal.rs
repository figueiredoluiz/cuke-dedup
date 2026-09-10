use super::shared::{bounded_findings, count_label, terminal_safe};
use super::ReportContext;
use anyhow::Result;
use std::collections::BTreeMap;
use std::io::Write;

/// Writes grouped, terminal-safe findings and totals to `writer`.
pub fn write_terminal(context: &ReportContext<'_>, writer: &mut dyn Write) -> Result<()> {
    let result = context.result;
    let root = context.root;
    let summary = &context.summary;
    writeln!(writer, "CukeDedup")?;
    let mut wrote_rule = false;
    let mut findings_by_rule: BTreeMap<_, Vec<_>> = BTreeMap::new();
    let selected = bounded_findings(result, true);
    let findings_truncated = selected.truncated;
    for finding in selected.findings {
        findings_by_rule
            .entry(finding.rule)
            .or_default()
            .push(finding);
    }
    for (rule, findings) in findings_by_rule {
        if wrote_rule {
            writeln!(writer)?;
        }
        writeln!(writer, "{rule}")?;
        wrote_rule = true;
        for finding in findings {
            writeln!(
                writer,
                "  [{}] {}",
                finding.severity,
                terminal_safe(&finding.message)
            )?;
            writeln!(
                writer,
                "    --> {}",
                terminal_safe(&finding.primary.display(root))
            )?;
            for related in &finding.related {
                writeln!(
                    writer,
                    "    related: {}",
                    terminal_safe(&related.display(root))
                )?;
            }
            if !finding.evidence.matcher_difference.is_empty() {
                writeln!(
                    writer,
                    "    evidence: {}",
                    terminal_safe(&finding.evidence.matcher_difference)
                )?;
            }
            if !finding.evidence.handler_evidence.is_empty() {
                writeln!(
                    writer,
                    "    handler: {}",
                    terminal_safe(&finding.evidence.handler_evidence)
                )?;
            }
            writeln!(
                writer,
                "    action: {}",
                terminal_safe(&finding.suggested_action)
            )?;
        }
    }
    if summary.findings > 0 {
        writeln!(writer)?;
    }
    if findings_truncated > 0 {
        writeln!(
            writer,
            "Showing the highest-severity findings; {} additional active findings omitted",
            findings_truncated
        )?;
    }
    writeln!(
        writer,
        "Analyzed {} and {}: {}, {}, {} suppressed",
        count_label(summary.definitions_analyzed, "definition"),
        count_label(summary.feature_steps_analyzed, "feature step"),
        count_label(summary.errors, "error"),
        count_label(summary.warnings, "warning"),
        summary.suppressed
    )?;
    writeln!(
        writer,
        "Duplication: {} of {} definitions ({:.2}%), threshold {:.2}% — {}",
        summary.duplication.duplicated_definitions,
        summary.duplication.total_definitions,
        summary.duplication.percentage,
        summary.duplication.threshold,
        if summary.duplication.passed {
            "PASS"
        } else {
            "FAIL"
        }
    )?;
    if !summary.duplication.rules.is_empty() {
        writeln!(
            writer,
            "Contributing rules: {}",
            summary
                .duplication
                .rules
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )?;
    }
    Ok(())
}
