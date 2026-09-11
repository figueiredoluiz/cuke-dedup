use super::shared::{
    bounded_findings, count_label, report_safe, report_value_with_metadata,
    MAX_REPORTED_CLUSTER_MEMBERS,
};
use super::{CliReportMetadata, ReportContext};
use crate::model::{DefinitionComparison, MatcherDiff};
use anyhow::{Context, Result};

const HTML_TEMPLATE: &str = include_str!("templates/report.html");

/// Renders the self-contained interactive HTML report without writing it to disk.
pub fn render_html(context: &ReportContext<'_>) -> Result<String> {
    render_html_context_with_metadata(context, None)
}

pub(super) fn render_html_context_with_metadata(
    context: &ReportContext<'_>,
    metadata: Option<&CliReportMetadata<'_>>,
) -> Result<String> {
    let result = context.result;
    let root = context.root;
    let summary = &context.summary;
    let rule_summary = if summary.by_rule.is_empty() {
        String::new()
    } else {
        let counts = summary
            .by_rule
            .iter()
            .map(|(rule, count)| {
                format!(
                    r#"<li><code>{}</code><strong>{count}</strong></li>"#,
                    html_escape(rule)
                )
            })
            .collect::<String>();
        format!(r#"<ul class="rule-summary" aria-label="Findings by rule">{counts}</ul>"#)
    };
    let contributing_rules = if summary.duplication.rules.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="threshold-rules">Contributing rules: {}</p>"#,
            summary
                .duplication
                .rules
                .iter()
                .map(|rule| format!("<code>{}</code>", html_escape(rule.as_str())))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let report_json = json_for_html(
        &serde_json::to_string(&report_value_with_metadata(context, metadata))
            .context("failed to serialize HTML report data")?,
    );
    let mut cards = String::new();
    let selected = bounded_findings(result, true);
    let truncated = selected.truncated;
    for finding in selected.findings {
        let location = finding.primary.display(root);
        let retained_related = MAX_REPORTED_CLUSTER_MEMBERS.saturating_sub(1);
        let mut related_locations = finding
            .related
            .iter()
            .take(retained_related)
            .map(|related| {
                format!(
                    r#"<li><code>{}</code></li>"#,
                    html_escape(&related.display(root))
                )
            })
            .collect::<String>();
        if finding.related.len() > retained_related {
            related_locations.push_str(&format!(
                "<li>... {} more locations omitted</li>",
                finding.related.len() - retained_related
            ));
        }
        let related = if related_locations.is_empty() {
            String::new()
        } else {
            format!(r#"<ul class="related">{related_locations}</ul>"#)
        };
        let comparison = finding
            .evidence
            .comparison
            .as_ref()
            .map(render_comparison)
            .unwrap_or_default();
        let handler_evidence = if finding.evidence.handler_evidence.is_empty() {
            String::new()
        } else {
            format!(
                r#"<p><strong>Handler evidence:</strong> {}</p>"#,
                html_escape(&finding.evidence.handler_evidence)
            )
        };
        cards.push_str(&format!(
            r#"<article class="finding" data-rule="{}" data-severity="{}"><header><span class="severity {}">{}</span><code>{}</code></header><h2>{}</h2><p class="location">{}</p>{related}<p>{}</p>{handler_evidence}{comparison}<p><strong>Action:</strong> {}</p></article>"#,
            finding.rule,
            finding.severity,
            finding.severity,
            finding.severity,
            finding.rule,
            html_escape(&finding.message),
            html_escape(&location),
            html_escape(&finding.evidence.matcher_difference),
            html_escape(&finding.suggested_action),
        ));
    }
    if cards.is_empty() {
        cards.push_str("<p class=empty>No active findings.</p>");
    }
    let mut truncation_notice = String::new();
    if let Some(metadata) = metadata.filter(|metadata| metadata.analysis.truncated) {
        let analysis = metadata.analysis;
        truncation_notice.push_str(&format!(
            r#"<p class="truncation-notice" role="alert">{}</p>"#,
            html_escape(&format!(
                "Analysis is incomplete: {} candidate comparisons were evaluated and {} were skipped after configured limits.",
                analysis.candidate_comparisons_evaluated,
                analysis.skipped_candidate_comparisons
            ))
        ));
    }
    if metadata.is_some_and(|metadata| metadata.corpus.incomplete) {
        truncation_notice.push_str(&format!(
            r#"<p class="truncation-notice" role="alert">{}</p>"#,
            html_escape(
                "Corpus is incomplete: one or more definition or feature inputs could not be represented fully, so some analysis was disabled or omitted."
            )
        ));
    }
    if truncated > 0 {
        truncation_notice.push_str(&format!(
            r#"<p class="truncation-notice" role="status">{}</p>"#,
            html_escape(&format!(
                "Showing the highest-severity active findings; {truncated} additional findings are available in the summary and streaming JSONL report."
            ))
        ));
    }

    let replacements = [
        (
            "{{DEFINITIONS}}",
            count_label(summary.definitions_analyzed, "definition"),
        ),
        (
            "{{FEATURE_STEPS}}",
            count_label(summary.feature_steps_analyzed, "feature step"),
        ),
        ("{{ERRORS}}", count_label(summary.errors, "error")),
        ("{{WARNINGS}}", count_label(summary.warnings, "warning")),
        ("{{SUPPRESSED}}", summary.suppressed.to_string()),
        (
            "{{THRESHOLD_CLASS}}",
            if summary.duplication.passed {
                "pass".to_owned()
            } else {
                "fail".to_owned()
            },
        ),
        (
            "{{DUPLICATED_DEFINITIONS}}",
            summary.duplication.duplicated_definitions.to_string(),
        ),
        (
            "{{TOTAL_DEFINITIONS}}",
            summary.duplication.total_definitions.to_string(),
        ),
        (
            "{{DUPLICATION_PERCENTAGE}}",
            format!("{:.2}", summary.duplication.percentage),
        ),
        (
            "{{DUPLICATION_THRESHOLD}}",
            format!("{:.2}", summary.duplication.threshold),
        ),
        (
            "{{THRESHOLD_STATUS}}",
            if summary.duplication.passed {
                "PASS".to_owned()
            } else {
                "FAIL".to_owned()
            },
        ),
        ("{{RULE_SUMMARY}}", rule_summary),
        ("{{CONTRIBUTING_RULES}}", contributing_rules),
        ("{{TRUNCATION_NOTICE}}", truncation_notice),
        ("{{CARDS}}", cards),
        ("{{REPORT_JSON}}", report_json),
    ];
    Ok(render_template(
        HTML_TEMPLATE.strip_suffix('\n').unwrap_or(HTML_TEMPLATE),
        &replacements,
    ))
}

fn render_template(template: &str, replacements: &[(&str, String)]) -> String {
    let mut rendered = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some((index, placeholder, value)) = replacements
        .iter()
        .filter_map(|(placeholder, value)| {
            remaining
                .find(placeholder)
                .map(|index| (index, *placeholder, value))
        })
        .min_by_key(|(index, _, _)| *index)
    {
        rendered.push_str(&remaining[..index]);
        rendered.push_str(value);
        remaining = &remaining[index + placeholder.len()..];
    }
    rendered.push_str(remaining);
    rendered
}

fn render_comparison(comparison: &DefinitionComparison) -> String {
    format!(
        r#"<div class="comparison"><section><h3>Primary definition</h3><code class="matcher">{}</code><pre><code>{}</code></pre></section><section><h3>Related definition</h3><code class="matcher">{}</code><pre><code>{}</code></pre></section></div>"#,
        render_matcher_side(&comparison.matcher_diff, true),
        html_escape(&comparison.left_handler),
        render_matcher_side(&comparison.matcher_diff, false),
        html_escape(&comparison.right_handler),
    )
}

fn render_matcher_side(diff: &MatcherDiff, left: bool) -> String {
    let change = if left {
        &diff.left_change
    } else {
        &diff.right_change
    };
    let change = if change.is_empty() {
        String::new()
    } else {
        format!("<mark>{}</mark>", html_escape(change))
    };
    format!(
        "{}{}{}",
        html_escape(&diff.prefix),
        change,
        html_escape(&diff.suffix)
    )
}

fn html_escape(value: &str) -> String {
    report_safe(value)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn json_for_html(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '<' | '>' | '&'
                | '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        ) {
            output.push_str(&format!("\\u{:04x}", character as u32));
        } else {
            output.push(character);
        }
    }
    output
}
