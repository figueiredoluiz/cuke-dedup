//! Source-local `cuke-dedup:ignore` directives on the comment lines above a registration.

use crate::model::{InlineSuppression, Rule, SourceLocation};
use crate::resource_limits::MAX_SUPPRESSION_REASON_CHARS;
use crate::source_adapter::{ExtractionDiagnostic, ExtractionDiagnosticLevel, SourceFile};
use std::cell::RefCell;
use std::collections::HashMap;

/// One row's suppressions and the directive diagnostics its parse produced.
type ParsedRow = (Vec<InlineSuppression>, Vec<ExtractionDiagnostic>);

/// Directive comments of one file, parsed once per registration row.
///
/// Registrations sharing a row read the same lines above it; reparsing them per registration is
/// quadratic when many registrations share a row under a long line.
pub(crate) struct InlineSuppressions<'source> {
    comment_prefix: &'source str,
    lines: &'source [&'source str],
    file: &'source SourceFile,
    rows: RefCell<HashMap<usize, ParsedRow>>,
}

impl<'source> InlineSuppressions<'source> {
    /// Prepares lookups over `lines`, the file's lines split on `\n`.
    pub(crate) fn new(
        comment_prefix: &'source str,
        lines: &'source [&'source str],
        file: &'source SourceFile,
    ) -> Self {
        Self {
            comment_prefix,
            lines,
            file,
            rows: RefCell::default(),
        }
    }

    /// Returns the suppressions above the zero-based `registration_row` and appends that row's
    /// directive diagnostics, as each registration on the row did before the row was cached.
    pub(crate) fn at(
        &self,
        registration_row: usize,
        diagnostics: &mut Vec<ExtractionDiagnostic>,
    ) -> Vec<InlineSuppression> {
        let mut rows = self.rows.borrow_mut();
        let (suppressions, row_diagnostics) = rows.entry(registration_row).or_insert_with(|| {
            let mut row_diagnostics = Vec::new();
            let suppressions = inline_suppressions(
                registration_row,
                self.comment_prefix,
                self.lines,
                self.file,
                &mut row_diagnostics,
            );
            (suppressions, row_diagnostics)
        });
        diagnostics.extend(row_diagnostics.iter().cloned());
        suppressions.clone()
    }
}

/// Reads the contiguous directive comments directly above the zero-based `registration_row`.
fn inline_suppressions(
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

/// Builds an `Error`-level, `Other`-kind diagnostic as an empty span at column 1 of one-based line
/// `zero_based_line + 1`; it does not mark extraction incomplete.
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

#[cfg(test)]
mod tests;
