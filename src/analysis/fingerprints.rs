//! Per-definition comparison evidence written by `--emit-fingerprints` for test oracles.

use super::evidence::definition_semantic_fingerprint;
use crate::model::{stable_fingerprint, MatcherKind, StepDefinition};
use serde::Serialize;
use std::io::Write;
use std::path::Path;

/// Schema version written into every fingerprint record.
const FINGERPRINT_SCHEMA_VERSION: &str = "1";

// Field order is the serialized order and part of the schema contract. `F` passes the framework
// through without naming its type, which the language-boundary ratchet keeps out of analysis.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintRecord<'a, F> {
    schema_version: &'static str,
    path: String,
    line: usize,
    column: usize,
    domain: &'static str,
    framework: &'a F,
    registration: &'a str,
    matcher: &'a str,
    matcher_kind: MatcherKind,
    matcher_flags: &'a str,
    normalized_matcher: &'a str,
    definition_fingerprint: String,
    handler: HandlerRecord,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HandlerRecord {
    exact: String,
    normalized: String,
    alpha_normalized: String,
    structural: String,
    behavior: String,
    comparable: bool,
    trivial: bool,
}

/// Root-relative, `/`-separated path of a definition, shared with the pair oracle so both join.
pub(super) fn relative_path(definition: &StepDefinition, root: &Path) -> String {
    let path = &definition.location.path;
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Builds the record for one definition. Handler representations are digested so Ruby's raw keys
/// and TypeScript's hashed keys have one bounded form; equal keys give equal digests.
fn record<'a>(definition: &'a StepDefinition, root: &Path) -> impl Serialize + 'a {
    let location = &definition.location;
    let handler = &definition.handler;
    FingerprintRecord {
        schema_version: FINGERPRINT_SCHEMA_VERSION,
        path: relative_path(definition, root),
        line: location.line,
        column: location.column,
        domain: definition.analysis_profile().domain.label,
        framework: &definition.framework,
        registration: &definition.registration,
        matcher: &definition.matcher,
        matcher_kind: definition.matcher_kind,
        matcher_flags: &definition.matcher_flags,
        normalized_matcher: &definition.normalized_matcher,
        definition_fingerprint: definition_semantic_fingerprint(definition),
        handler: HandlerRecord {
            exact: stable_fingerprint(&handler.exact),
            normalized: stable_fingerprint(&handler.normalized),
            alpha_normalized: stable_fingerprint(&handler.alpha_normalized),
            structural: stable_fingerprint(&handler.structural),
            behavior: handler.behavior_digest(),
            comparable: handler.comparable,
            trivial: handler.trivial,
        },
    }
}

/// Writes one compact JSON line per definition, in the order given.
pub(crate) fn write_fingerprints(
    definitions: &[StepDefinition],
    root: &Path,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    for definition in definitions {
        serde_json::to_writer(&mut *out, &record(definition, root))?;
        out.write_all(b"\n")?;
    }
    Ok(())
}
