//! Resource limits applied while reading untrusted repository inputs.

use anyhow::{Context, Result};
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const MAX_PROJECT_INPUT_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_CONFIG_INPUT_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_SUPPRESSION_REASON_CHARS: usize = 512;
pub(crate) const MAX_BASELINE_INPUT_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_REGEX_PATTERN_BYTES: usize = 1024 * 1024;
pub(crate) const REGEX_SIZE_LIMIT_BYTES: usize = 1024 * 1024;
pub(crate) const REGEX_DFA_SIZE_LIMIT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_STATIC_OVERLAP_FINDINGS: usize = 10_000;
pub(crate) const MAX_REGISTRATION_MODULES: usize = 1_024;
pub(crate) const MAX_REGISTRATION_MODULE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_REGISTRATION_RESOLUTION_STATES: usize = 16_384;
pub(crate) const MAX_PROJECT_CONFIG_EXTENDS_DEPTH: usize = 16;
pub(crate) const MAX_PROJECT_METADATA_FILES: usize = 1_024;
pub(crate) const MAX_PROJECT_METADATA_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_PROJECT_MODULE_MAPPINGS: usize = 16_384;
pub(crate) const MAX_WORKSPACE_SCAN_ENTRIES: usize = 100_000;
pub(crate) const MAX_GHERKIN_MARKDOWN_CANDIDATES: usize = 10_000;
pub(crate) const MAX_GHERKIN_MARKDOWN_PROBE_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const MAX_GHERKIN_MARKDOWN_PROBES: usize = 128;
pub(crate) const GHERKIN_MARKDOWN_RESTORE_BATCH_SIZE: usize = 32;
pub(crate) const MAX_CANDIDATE_COMPARISONS: usize = 2_000_000;
pub(crate) const MAX_STRUCTURAL_CLASS_COMPARISONS: usize = 250_000;

#[derive(Debug)]
struct InputLimitExceeded {
    path: PathBuf,
    kind: &'static str,
    limit: u64,
}

impl fmt::Display for InputLimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} {} exceeds the {}-byte input limit; reduce the file before retrying",
            self.kind,
            self.path.display(),
            self.limit
        )
    }
}

impl Error for InputLimitExceeded {}

pub(crate) fn read_utf8(path: &Path, kind: &'static str, limit: u64) -> Result<String> {
    let file =
        File::open(path).with_context(|| format!("failed to read {kind} {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to inspect {kind} {}", path.display()))?;
    if metadata.len() > limit {
        return Err(InputLimitExceeded {
            path: path.to_owned(),
            kind,
            limit,
        }
        .into());
    }

    // The metadata check rejects known-large regular files without allocating for them. The
    // bounded reader also handles files that grow between metadata and read, and special files
    // whose metadata does not expose their eventual length.
    let mut bytes = Vec::with_capacity(metadata.len().min(limit) as usize);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read {kind} {}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(InputLimitExceeded {
            path: path.to_owned(),
            kind,
            limit,
        }
        .into());
    }
    String::from_utf8(bytes)
        .with_context(|| format!("{kind} {} is not valid UTF-8", path.display()))
}

pub(crate) fn is_input_limit_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<InputLimitExceeded>().is_some())
}

pub(crate) fn compile_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    compile_regex_with_limits(pattern, REGEX_SIZE_LIMIT_BYTES, REGEX_DFA_SIZE_LIMIT_BYTES)
}

fn compile_regex_with_limits(
    pattern: &str,
    size_limit: usize,
    dfa_size_limit: usize,
) -> Result<regex::Regex, regex::Error> {
    regex::RegexBuilder::new(pattern)
        .size_limit(size_limit)
        .dfa_size_limit(dfa_size_limit)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn bounded_reader_accepts_exact_limit_and_rejects_next_byte() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.ts");
        fs::write(&path, "1234").unwrap();
        assert_eq!(read_utf8(&path, "source", 4).unwrap(), "1234");

        fs::write(&path, "12345").unwrap();
        let error = read_utf8(&path, "source", 4).unwrap_err();
        assert!(is_input_limit_error(&error));
        assert!(error.to_string().contains("4-byte input limit"));
    }

    #[test]
    fn bounded_reader_rejects_invalid_utf8_without_misclassifying_it_as_a_limit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input.ts");
        fs::write(&path, [0xff]).unwrap();

        let error = read_utf8(&path, "source", 4).unwrap_err();
        assert!(!is_input_limit_error(&error));
        assert!(error.to_string().contains("not valid UTF-8"));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_limits_special_files_without_reliable_metadata_lengths() {
        let error = read_utf8(Path::new("/dev/zero"), "special input", 4).unwrap_err();
        assert!(is_input_limit_error(&error));
        assert!(error.to_string().contains("4-byte input limit"));
    }

    #[test]
    fn regex_builder_enforces_the_configured_program_limit() {
        let pattern = (0..1_000)
            .map(|index| format!("literal-{index}"))
            .collect::<Vec<_>>()
            .join("|");
        let error = compile_regex_with_limits(&pattern, 128, 128).unwrap_err();
        assert!(matches!(error, regex::Error::CompiledTooBig(128)));
    }

    #[test]
    fn release_resource_limits_match_the_documented_policy() {
        assert_eq!(MAX_PROJECT_INPUT_BYTES, 8 * 1024 * 1024);
        assert_eq!(MAX_CONFIG_INPUT_BYTES, 1024 * 1024);
        assert_eq!(MAX_SUPPRESSION_REASON_CHARS, 512);
        assert_eq!(MAX_BASELINE_INPUT_BYTES, 8 * 1024 * 1024);
        assert_eq!(MAX_REGEX_PATTERN_BYTES, 1024 * 1024);
        assert_eq!(REGEX_SIZE_LIMIT_BYTES, 1024 * 1024);
        assert_eq!(REGEX_DFA_SIZE_LIMIT_BYTES, 2 * 1024 * 1024);
        assert_eq!(MAX_STATIC_OVERLAP_FINDINGS, 10_000);
        assert_eq!(MAX_REGISTRATION_MODULES, 1_024);
        assert_eq!(MAX_REGISTRATION_MODULE_BYTES, 64 * 1024 * 1024);
        assert_eq!(MAX_REGISTRATION_RESOLUTION_STATES, 16_384);
        assert_eq!(MAX_PROJECT_CONFIG_EXTENDS_DEPTH, 16);
        assert_eq!(MAX_PROJECT_METADATA_FILES, 1_024);
        assert_eq!(MAX_PROJECT_METADATA_BYTES, 16 * 1024 * 1024);
        assert_eq!(MAX_PROJECT_MODULE_MAPPINGS, 16_384);
        assert_eq!(MAX_WORKSPACE_SCAN_ENTRIES, 100_000);
        assert_eq!(MAX_GHERKIN_MARKDOWN_CANDIDATES, 10_000);
        assert_eq!(MAX_GHERKIN_MARKDOWN_PROBE_BYTES, 64 * 1024 * 1024);
        assert_eq!(MAX_GHERKIN_MARKDOWN_PROBES, 128);
        assert_eq!(GHERKIN_MARKDOWN_RESTORE_BATCH_SIZE, 32);
        assert_eq!(MAX_CANDIDATE_COMPARISONS, 2_000_000);
        assert_eq!(MAX_STRUCTURAL_CLASS_COMPARISONS, 250_000);
    }
}
