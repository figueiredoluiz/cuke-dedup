//! Source-local `cuke-dedup:ignore` directives on the comment lines above a registration.

use crate::model::{InlineSuppression, Rule, SourceLocation};
use crate::resource_limits::MAX_SUPPRESSION_REASON_CHARS;
use crate::source_adapter::{ExtractionDiagnostic, ExtractionDiagnosticLevel, SourceFile};

/// Reads the contiguous directive comments directly above the zero-based `registration_row`.
pub(crate) fn inline_suppressions(
    registration_row: usize,
    comment_prefix: &str,
    lines: &[&str],
    file: &SourceFile,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
) -> Vec<InlineSuppression> {
    let mut line_index = registration_row.checked_sub(1);
    let mut suppressions = Vec::new();

    while let Some(index) = line_index {
        let line = lines.get(index).copied().unwrap_or_default().trim();
        let Some(directive) = line
            .strip_prefix(comment_prefix)
            .map(str::trim)
            .and_then(|line| line.strip_prefix("cuke-dedup:ignore").map(str::trim))
        else {
            break;
        };
        let Some((rule, reason)) = directive.split_once("--") else {
            diagnostics.push(inline_suppression_diagnostic(
                file,
                index,
                &format!(
                    "inline suppression must use `{comment_prefix} cuke-dedup:ignore RULE -- REASON`"
                ),
            ));
            line_index = index.checked_sub(1);
            continue;
        };
        let rule = match rule.trim().parse::<Rule>() {
            Ok(rule) => rule,
            Err(error) => {
                diagnostics.push(inline_suppression_diagnostic(file, index, &error));
                line_index = index.checked_sub(1);
                continue;
            }
        };
        let reason = reason.trim();
        if reason.is_empty() {
            diagnostics.push(inline_suppression_diagnostic(
                file,
                index,
                "inline suppression reason must not be empty",
            ));
        } else if reason.chars().count() > MAX_SUPPRESSION_REASON_CHARS {
            diagnostics.push(inline_suppression_diagnostic(
                file,
                index,
                &format!(
                    "inline suppression reason exceeds the {MAX_SUPPRESSION_REASON_CHARS}-character limit"
                ),
            ));
        } else {
            suppressions.push(InlineSuppression {
                rule,
                reason: reason.to_owned(),
            });
        }
        line_index = index.checked_sub(1);
    }
    suppressions.reverse();
    suppressions
}

fn inline_suppression_diagnostic(
    file: &SourceFile,
    zero_based_line: usize,
    message: &str,
) -> ExtractionDiagnostic {
    ExtractionDiagnostic {
        kind: crate::source_adapter::ExtractionDiagnosticKind::Other,
        level: ExtractionDiagnosticLevel::Error,
        location: SourceLocation::new(&file.path, zero_based_line + 1, 1, zero_based_line + 1, 1),
        message: message.to_owned(),
    }
}
