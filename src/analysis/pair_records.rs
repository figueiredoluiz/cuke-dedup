//! Per-pair verification evidence written by `--emit-pairs` for test oracles.

use super::evidence::definition_semantic_fingerprint;
use super::fingerprints::relative_path;
use super::pairs::PairRelationships;
use crate::model::{Rule, StepDefinition};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

/// Schema version written into every pair record.
const PAIR_SCHEMA_VERSION: &str = "1";

/// What the pair verifier decided for one candidate pair that it evaluated.
pub(crate) struct PairRecord {
    pub(super) left: usize,
    pub(super) right: usize,
    pub(super) sources: Vec<&'static str>,
    pub(super) owner: &'static str,
    pub(super) positional: bool,
    pub(super) relationships: PairRelationships,
    pub(super) meaningful_handlers: bool,
    pub(super) matcher_similarity: Option<f64>,
    pub(super) handler_similarity: Option<f64>,
    pub(super) rules: Vec<Rule>,
}

// Field order is the serialized order and part of the schema contract.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairLine<'a> {
    schema_version: &'static str,
    left: &'a Side,
    right: &'a Side,
    candidate_sources: &'a [&'static str],
    owner: &'static str,
    positional: bool,
    relationships: PairRelationships,
    meaningful_handlers: bool,
    matcher_similarity: Option<f64>,
    handler_similarity: Option<f64>,
    rules: &'a [Rule],
}

/// Identifies one definition; `definitionFingerprint` joins to `--emit-fingerprints`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Side {
    path: String,
    line: usize,
    column: usize,
    definition_fingerprint: String,
}

/// Names `definition` by root-relative path, position and comparison fingerprint.
fn side(definition: &StepDefinition, root: &Path) -> Side {
    let location = &definition.location;
    Side {
        path: relative_path(definition, root),
        line: location.line,
        column: location.column,
        definition_fingerprint: definition_semantic_fingerprint(definition),
    }
}

/// Writes one compact JSON line per evaluated pair, in verification order. Each definition is
/// named once, however many pairs it takes part in.
pub(crate) fn write_pairs(
    definitions: &[StepDefinition],
    records: &[PairRecord],
    root: &Path,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    let mut sides: HashMap<usize, Side> = HashMap::new();
    for record in records {
        for index in [record.left, record.right] {
            sides
                .entry(index)
                .or_insert_with(|| side(&definitions[index], root));
        }
        let line = PairLine {
            schema_version: PAIR_SCHEMA_VERSION,
            left: &sides[&record.left],
            right: &sides[&record.right],
            candidate_sources: &record.sources,
            owner: record.owner,
            positional: record.positional,
            relationships: record.relationships,
            meaningful_handlers: record.meaningful_handlers,
            matcher_similarity: record.matcher_similarity,
            handler_similarity: record.handler_similarity,
            rules: &record.rules,
        };
        serde_json::to_writer(&mut *out, &line)?;
        out.write_all(b"\n")?;
    }
    Ok(())
}
