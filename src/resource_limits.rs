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
pub(crate) const MAX_ASSERTION_RESOLUTION_WORK: usize = 1_000_000;
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
pub(crate) const SOURCE_CLASSIFICATION_PREFIX_BYTES: usize = 64 * 1024;
pub(crate) const MINIFIED_MEAN_LINE_BYTES: usize = 200;

/// Work counted against a limit. `charge` always takes the work and reports a total past the limit;
/// `admit` takes it only when the total stays within the limit.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WorkBudget {
    limit: usize,
    spent: usize,
}

impl WorkBudget {
    /// A budget of `limit` with nothing spent.
    pub(crate) const fn new(limit: usize) -> Self {
        Self { limit, spent: 0 }
    }

    /// Charges `amount`, saturating, and reports whether the total now exceeds the limit. The total
    /// never decreases, so once it exceeds the limit every later charge reports it too.
    pub(crate) fn charge(&mut self, amount: usize) -> bool {
        self.spent = self.spent.saturating_add(amount);
        self.spent > self.limit
    }

    /// Charges `amount` only when the total stays within the limit, and reports whether it did.
    /// A refusal does not latch: a smaller later amount may still fit, so a caller that must not
    /// admit anything after a cut stops at the first refusal or records it itself.
    pub(crate) fn admit(&mut self, amount: usize) -> bool {
        let fits = self.fits(amount);
        if fits {
            self.spent = self.spent.saturating_add(amount);
        }
        fits
    }

    /// Reports whether `amount` would keep the total within the limit, charging nothing. A caller
    /// that charges only after later work succeeds checks here first.
    pub(crate) const fn fits(&self, amount: usize) -> bool {
        self.spent.saturating_add(amount) <= self.limit
    }

    /// The limit this budget was created with.
    pub(crate) const fn limit(&self) -> usize {
        self.limit
    }

    /// The total charged so far.
    #[cfg(test)]
    pub(crate) const fn spent(&self) -> usize {
        self.spent
    }
}

/// Work counted against one limit across a whole run and within the current pass. Exhaustion
/// latches: once either total exceeds the limit every later check reports it, including after a
/// new pass restarts the per-pass total.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PassBudget {
    limit: usize,
    run: usize,
    pass: usize,
    exhausted: bool,
}

impl PassBudget {
    /// A budget of `limit` for the run and for each pass, not yet exhausted.
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            limit,
            run: 0,
            pass: 0,
            exhausted: false,
        }
    }

    /// Charges work that spans every pass and reports latched exhaustion.
    pub(crate) fn charge_run(&mut self, amount: usize) -> bool {
        self.run = self.run.saturating_add(amount);
        self.latch()
    }

    /// Charges work to the current pass and reports latched exhaustion.
    pub(crate) fn charge_pass(&mut self, amount: usize) -> bool {
        self.pass = self.pass.saturating_add(amount);
        self.latch()
    }

    /// Restarts the per-pass total, keeping run work and the latch, and reports exhaustion.
    pub(crate) fn begin_pass(&mut self) -> bool {
        self.pass = 0;
        self.latch()
    }

    /// Reports latched exhaustion.
    pub(crate) const fn exhausted(&self) -> bool {
        self.exhausted
    }

    /// Latches either total exceeding the limit and reports the latched state.
    fn latch(&mut self) -> bool {
        self.exhausted |= self.run > self.limit || self.pass > self.limit;
        self.exhausted
    }

    /// Replaces the limit, keeping the work already charged, and latches a total now over it.
    #[cfg(test)]
    pub(crate) fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        self.latch();
    }

    /// Work charged across the run and within the current pass.
    #[cfg(test)]
    pub(crate) const fn spent(&self) -> (usize, usize) {
        (self.run, self.pass)
    }
}

#[derive(Debug)]
struct InputLimitExceeded {
    path: PathBuf,
    kind: &'static str,
    limit: u64,
}

impl fmt::Display for InputLimitExceeded {
    /// Writes the kind, path and byte limit with a hint to shrink the file.
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

/// Reads `path` as UTF-8, failing with `InputLimitExceeded` when metadata or the bytes read exceed
/// `limit`; open, metadata and read failures and invalid UTF-8 return other contextual errors.
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

/// Reports whether any cause in `error`'s chain is an `InputLimitExceeded`.
pub(crate) fn is_input_limit_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<InputLimitExceeded>().is_some())
}

/// Returns the standard input-limit diagnostic when metadata already proves a file is too large.
/// Metadata failures and files that can still grow are left to the bounded reader.
pub(crate) fn known_input_limit_exceeded(
    path: &Path,
    kind: &'static str,
    limit: u64,
) -> Option<String> {
    (std::fs::metadata(path).ok()?.len() > limit).then(|| {
        InputLimitExceeded {
            path: path.to_owned(),
            kind,
            limit,
        }
        .to_string()
    })
}

/// Compiles `pattern` with the release program and lazy-DFA size limits.
pub(crate) fn compile_regex(pattern: &str) -> Result<regex::Regex, regex::Error> {
    compile_regex_with_limits(pattern, REGEX_SIZE_LIMIT_BYTES, REGEX_DFA_SIZE_LIMIT_BYTES)
}

/// Compiles `pattern` with the given program size limit and lazy-DFA cache limit.
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
mod tests;
