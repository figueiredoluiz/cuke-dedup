use crate::model::{
    stable_fingerprint_parts, DefinitionComparison, Finding, MatcherDiff, MatcherKind,
    SourceLocation, StepDefinition,
};
use crate::resource_limits::{sample_text, LimitedWriter};
use serde::{ser::SerializeStruct, Serialize, Serializer};
use std::borrow::Cow;

pub(super) const MAX_EVIDENCE_TEXT_BYTES: usize = 8 * 1024;
pub(super) const MAX_FINDING_EVIDENCE_BYTES: usize = 16 * 1024 * 1024;

/// Samples presentation text before it is copied into a finding. A digest of the full input
/// distinguishes long values that share a prefix in the display; baseline identity is separate.
pub(super) fn evidence_text(value: &str) -> Cow<'_, str> {
    if value.len() <= MAX_EVIDENCE_TEXT_BYTES {
        return Cow::Borrowed(value);
    }
    Cow::Owned(format!(
        "{} [digest: {}]",
        sample_text(value, MAX_EVIDENCE_TEXT_BYTES - 48),
        stable_fingerprint_parts([value])
    ))
}

/// Preserves the legacy fallback identity from original message/evidence fragments before
/// sampling. The original fields are length-prefixed exactly as in `finding_fingerprint`.
pub(super) fn original_fallback_fingerprint(
    rule: crate::model::Rule,
    message: &[&str],
    matcher_difference: &[&str],
    handler_evidence: &[&str],
) -> [u8; 16] {
    crate::model::fingerprint_fragments(
        [
            &["finding-fallback"][..],
            &[rule.as_str()],
            message,
            matcher_difference,
            handler_evidence,
        ]
        .into_iter()
        .map(|part| part.iter().copied()),
    )
}

/// Borrows finding contents for byte accounting while accepting every filesystem path.
/// The stored finding and its raw path bytes remain untouched.
struct FindingForBudget<'a>(&'a Finding);

impl Serialize for FindingForBudget<'_> {
    /// Serializes every finding field, replacing only location views. Exhaustive destructuring
    /// forces new model fields to be included in the accounting view.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Finding {
            rule,
            severity,
            message,
            original_fingerprint,
            primary,
            related,
            evidence,
            suggested_action,
            suppression,
        } = self.0;
        let mut state = serializer
            .serialize_struct("Finding", 8 + usize::from(original_fingerprint.is_some()))?;
        state.serialize_field("rule", rule)?;
        state.serialize_field("severity", severity)?;
        state.serialize_field("message", message)?;
        if original_fingerprint.is_some() {
            state.serialize_field("original_fingerprint", original_fingerprint)?;
        }
        state.serialize_field("primary", &LocationForBudget(primary))?;
        state.serialize_field("related", &LocationsForBudget(related))?;
        state.serialize_field("evidence", evidence)?;
        state.serialize_field("suggested_action", suggested_action)?;
        state.serialize_field("suppression", suppression)?;
        state.end()
    }
}

/// A location's accounting view uses the same lossy path conversion as displayed reports.
struct LocationForBudget<'a>(&'a SourceLocation);

impl Serialize for LocationForBudget<'_> {
    /// Counts displayable path bytes and the complete span without changing the source location.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let SourceLocation {
            path,
            line,
            column,
            end_line,
            end_column,
        } = self.0;
        let mut state = serializer.serialize_struct("SourceLocation", 5)?;
        state.serialize_field("path", &path.to_string_lossy())?;
        state.serialize_field("line", line)?;
        state.serialize_field("column", column)?;
        state.serialize_field("end_line", end_line)?;
        state.serialize_field("end_column", end_column)?;
        state.end()
    }
}

/// Streams related location views without copying their paths or allocating a second list.
struct LocationsForBudget<'a>(&'a [SourceLocation]);

impl Serialize for LocationsForBudget<'_> {
    /// Visits every related location in its original order.
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(LocationForBudget))
    }
}

/// Charges serialized finding bytes without an output buffer, using displayable path views.
/// A refusal leaves the finding out;
/// the caller must mark its analysis incomplete and stop producing findings for this pass.
pub(super) fn retain_finding(
    findings: &mut Vec<crate::model::Finding>,
    remaining: &mut usize,
    finding: crate::model::Finding,
) -> bool {
    let mut counter = LimitedWriter {
        inner: std::io::sink(),
        remaining: *remaining,
    };
    if serde_json::to_writer(&mut counter, &FindingForBudget(&finding)).is_err() {
        return false;
    }
    *remaining = counter.remaining;
    findings.push(finding);
    true
}

/// Renders bounded samples of the two matchers as `` `left` ↔ `right` `` for finding evidence.
pub(super) fn matcher_difference(left: &StepDefinition, right: &StepDefinition) -> String {
    format!(
        "`{}` ↔ `{}`",
        evidence_text(&left.matcher),
        evidence_text(&right.matcher)
    )
}

/// Builds the pair comparison: sampled matchers, handler source snippets, semantic fingerprints and
/// the character-level matcher diff.
pub(super) fn definition_comparison(
    left: &StepDefinition,
    right: &StepDefinition,
) -> DefinitionComparison {
    DefinitionComparison {
        left_fingerprint: definition_semantic_fingerprint(left),
        right_fingerprint: definition_semantic_fingerprint(right),
        left_matcher: evidence_text(&left.matcher).into_owned(),
        right_matcher: evidence_text(&right.matcher).into_owned(),
        left_handler: evidence_text(&left.handler.source_snippet).into_owned(),
        right_handler: evidence_text(&right.handler.source_snippet).into_owned(),
        matcher_diff: matcher_diff(&left.matcher, &right.matcher),
    }
}

/// Hashes matcher kind, normalized matcher, matcher flags and alpha-normalized handler into a
/// 32-hex-digit fingerprint; parts are length-prefixed, so bytes cannot shift between components.
pub(super) fn definition_semantic_fingerprint(definition: &StepDefinition) -> String {
    let matcher_kind = match definition.matcher_kind {
        MatcherKind::Literal => "literal",
        MatcherKind::CucumberExpression => "cucumber-expression",
        MatcherKind::RegularExpression => "regular-expression",
    };
    stable_fingerprint_parts([
        "definition-semantic",
        matcher_kind,
        &definition.normalized_matcher,
        &definition.matcher_flags,
        &definition.handler.alpha_normalized,
    ])
}

/// Splits two matchers by Unicode scalar into a common prefix, the two differing middles and a
/// common suffix; the suffix never overlaps the prefix, so identical inputs yield an empty suffix.
fn matcher_diff(left: &str, right: &str) -> MatcherDiff {
    let prefix = left
        .chars()
        .zip(right.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let suffix = left[prefix..]
        .chars()
        .rev()
        .zip(right[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    MatcherDiff {
        prefix: evidence_text(&left[..prefix]).into_owned(),
        left_change: evidence_text(&left[prefix..left.len() - suffix]).into_owned(),
        right_change: evidence_text(&right[prefix..right.len() - suffix]).into_owned(),
        suffix: evidence_text(&left[left.len() - suffix..]).into_owned(),
    }
}

/// Names the strictest handler fingerprint the pair shares, checked in order exact, normalized,
/// alpha-normalized, structural; when none match, reports that behaviour signatures differ.
pub(super) fn handler_evidence(left: &StepDefinition, right: &StepDefinition) -> String {
    if left.handler.exact == right.handler.exact {
        "Handlers have the same exact syntax fingerprint".to_owned()
    } else if left.handler.normalized == right.handler.normalized {
        "Handlers differ only in comments or formatting".to_owned()
    } else if left.handler.alpha_normalized == right.handler.alpha_normalized {
        "Handlers differ only in parameter or local-variable names".to_owned()
    } else if left.handler.structural == right.handler.structural {
        "Handlers share the same structure after literal normalization".to_owned()
    } else {
        "Handler behavior signatures differ".to_owned()
    }
}

#[cfg(test)]
mod tests;
