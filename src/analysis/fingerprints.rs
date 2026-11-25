//! Per-definition comparison evidence written by `--emit-fingerprints` for test oracles.

use super::evidence::definition_semantic_fingerprint;
use crate::model::{stable_fingerprint, Framework, MatcherKind, StepDefinition};
use crate::resource_limits::{LimitedWriter, MAX_ORACLE_OUTPUT_BYTES, MAX_ORACLE_RECORDS};
use anyhow::bail;
use serde::Serialize;
use std::io::Write;
use std::path::Path;

/// Schema version written into every fingerprint record.
const FINGERPRINT_SCHEMA_VERSION: &str = "1";

// Field order is the serialized order and part of the schema contract.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintRecord<'a> {
    schema_version: &'static str,
    path: String,
    line: usize,
    column: usize,
    domain: &'static str,
    framework: &'a Framework,
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

/// The ordering shared by definition and pair evidence.
pub(super) fn definition_position(definition: &StepDefinition) -> (&Path, usize, usize) {
    let location = &definition.location;
    (&location.path, location.line, location.column)
}

/// Writes one compact JSON line per definition in source order.
pub(crate) fn write_fingerprints(
    definitions: &[StepDefinition],
    root: &Path,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    write_fingerprints_with_limits(
        definitions,
        root,
        out,
        (MAX_ORACLE_RECORDS, MAX_ORACLE_OUTPUT_BYTES),
    )
}

/// Rejects excess records before serialization and charges each written byte.
pub(super) fn write_fingerprints_with_limits(
    definitions: &[StepDefinition],
    root: &Path,
    out: &mut dyn Write,
    (max_records, max_bytes): (usize, usize),
) -> anyhow::Result<()> {
    if definitions.len() > max_records {
        bail!("fingerprint record limit exceeded ({max_records})");
    }
    let mut writer = LimitedWriter {
        inner: out,
        remaining: max_bytes,
    };
    let mut ordered: Vec<_> = definitions.iter().collect();
    ordered.sort_by_key(|definition| definition_position(definition));
    for definition in ordered {
        serde_json::to_writer(&mut writer, &record(definition, root))?;
        writer.write_all(b"\n")?;
    }
    Ok(())
}
