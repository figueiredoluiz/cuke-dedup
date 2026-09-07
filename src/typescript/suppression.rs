use crate::model::{InlineSuppression, Rule, SourceLocation};
use crate::source_adapter::{ExtractionDiagnostic, ExtractionDiagnosticLevel, SourceFile};
use tree_sitter::Node;

pub(super) fn inline_suppressions(
    call: Node<'_>,
    lines: &[&str],
    file: &SourceFile,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
) -> Vec<InlineSuppression> {
    let mut line_index = call.start_position().row.checked_sub(1);
    let mut suppressions = Vec::new();

    while let Some(index) = line_index {
        let line = lines.get(index).copied().unwrap_or_default().trim();
        let Some(directive) = line
            .strip_prefix("//")
            .map(str::trim)
            .and_then(|line| line.strip_prefix("cuke-dedup:ignore").map(str::trim))
        else {
            break;
        };
        let Some((rule, reason)) = directive.split_once("--") else {
            diagnostics.push(inline_suppression_diagnostic(
                file,
                index,
                "inline suppression must use `// cuke-dedup:ignore RULE -- REASON`",
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
        level: ExtractionDiagnosticLevel::Error,
        location: SourceLocation::new(&file.path, zero_based_line + 1, 1, zero_based_line + 1, 1),
        message: message.to_owned(),
    }
}
