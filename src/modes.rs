//! Changed-files and existing-findings baseline modes for incremental CI adoption.

use crate::config::normalize_platform_path;
use crate::model::{
    stable_fingerprint, stable_fingerprint_parts, DefinitionComparison, Finding, Suppression,
};
use crate::resource_limits::{read_utf8, MAX_BASELINE_INPUT_BYTES};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

// Snapshotting is additional disk work, separate from the analyzer's per-source read limits.
const MAX_BASELINE_CHECKOUT_BYTES: u64 = 512 * 1024 * 1024;

/// Returns tracked changes since `base` plus untracked files beneath `root`.
pub fn git_changed_files(root: &Path, base: &str) -> Result<BTreeSet<PathBuf>> {
    let base_oid = resolve_commit(root, base)?;
    let tracked = git_paths(
        root,
        &[
            "diff",
            "--relative",
            "--name-only",
            "-z",
            "--diff-filter=ACMR",
            &base_oid,
            "--",
        ],
        &format!("git diff against `{base}` failed"),
    )?;
    let untracked = git_paths(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
        "git untracked-file discovery failed",
    )?;
    Ok(tracked
        .into_iter()
        .chain(untracked)
        .map(|path| root.join(path))
        .collect())
}

/// Git environment variables that a `git` process exports to the hooks it runs. They outrank a
/// child invocation's `-C`, so leaving them in place would point changed-files mode at whichever
/// repository invoked the hook instead of the analyzed root.
const INHERITED_GIT_VARIABLES: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
];

/// Builds a `git` invocation bound to `root` and nothing else, so results describe the analyzed
/// tree whether CukeDedup runs from a shell, CI, or another repository's Git hook.
fn git_at(root: &Path) -> Command {
    let mut command = Command::new("git");
    for variable in INHERITED_GIT_VARIABLES {
        command.env_remove(variable);
    }
    command.arg("-C").arg(root);
    command
}

fn resolve_commit(root: &Path, base: &str) -> Result<String> {
    if base.trim().is_empty() || base.starts_with('-') {
        bail!("invalid --changed-since revision `{base}`");
    }
    let revision = format!("{base}^{{commit}}");
    let output = git_at(root)
        .args(["rev-parse", "--verify", "--end-of-options"])
        .arg(&revision)
        .output()
        .with_context(|| "failed to resolve the --changed-since revision")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!("could not resolve --changed-since revision `{base}`: {stderr}");
    }
    let oid = String::from_utf8(output.stdout)
        .context("git returned a non-UTF-8 commit object id")?
        .trim()
        .to_owned();
    if oid.is_empty() || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("git returned an invalid commit object id for `{base}`");
    }
    Ok(oid)
}

/// Materializes a fetched commit without running checkout hooks or configured filters.
/// An independent local clone avoids registering worktrees or modifying the caller's index.
pub(crate) fn baseline_snapshot(
    root: &Path,
    revision: &str,
) -> Result<(tempfile::TempDir, PathBuf)> {
    if revision.trim().is_empty() || revision.starts_with('-') {
        bail!("invalid baseline revision `{revision}`");
    }
    let temporary = tempfile::tempdir().context("cannot create temporary baseline checkout")?;
    let git = |directory: &Path, args: &[&std::ffi::OsStr]| -> Result<Vec<u8>> {
        let mut command = git_at(directory);
        // Neither caller-supplied Git config nor global templates/filters may execute code.
        // Clone does not copy the source repository's local config or hooks.
        for (name, _) in std::env::vars_os() {
            // Windows preserves spelling but treats environment names case-insensitively.
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                command.env_remove(name);
            }
        }
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_GLOBAL", temporary.path().join("no-config"))
            .arg("-c")
            .arg(format!(
                "core.hooksPath={}",
                temporary.path().join("no-hooks").display()
            ))
            .args(["-c", "core.fsmonitor=false"])
            .args(args);
        if args.first().is_some_and(|arg| *arg == "ls-tree") {
            // Fixed format: six mode digits, space, at most 20 size digits, newline.
            let limit = 28 * crate::resource_limits::MAX_WORKSPACE_SCAN_ENTRIES as u64;
            let mut child = command
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            let mut bytes = Vec::new();
            let read = child
                .stdout
                .take()
                .context("missing baseline Git stdout")
                .and_then(|stdout| Ok(stdout.take(limit + 1).read_to_end(&mut bytes)?));
            if read.is_err() || bytes.len() as u64 > limit {
                let _ = child.kill();
                let _ = child.wait();
                read?;
                bail!("baseline tree listing exceeds the bounded snapshot metadata limit");
            }
            if !child.wait()?.success() {
                bail!("baseline Git tree enumeration failed");
            }
            return Ok(bytes);
        }
        let output = command.output().context("failed to run Git for baseline")?;
        if !output.status.success() {
            bail!(
                "baseline Git operation failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    };
    let oid = git(
        root,
        &[
            "rev-parse".as_ref(),
            "--verify".as_ref(),
            "--end-of-options".as_ref(),
            format!("{revision}^{{commit}}").as_ref(),
        ],
    )?;
    let oid = String::from_utf8(oid).context("invalid baseline commit id")?;
    let tree = git(
        root,
        &[
            "ls-tree".as_ref(),
            "--full-tree".as_ref(),
            "-r".as_ref(),
            "--format=%(objectmode) %(objectsize)".as_ref(),
            oid.trim().as_ref(),
        ],
    )?;
    let mut total_bytes = 0_u64;
    for (index, entry) in String::from_utf8(tree)?.lines().enumerate() {
        if entry.starts_with("160000 ") {
            bail!("baseline checkout contains unsupported submodules");
        }
        let (_, size) = entry
            .split_once(' ')
            .context("invalid baseline tree entry")?;
        total_bytes = total_bytes.saturating_add(size.parse::<u64>()?);
        if total_bytes > MAX_BASELINE_CHECKOUT_BYTES
            || index >= crate::resource_limits::MAX_WORKSPACE_SCAN_ENTRIES
        {
            bail!("baseline checkout exceeds the 512 MiB / 100000-file snapshot limit");
        }
    }
    let repository = git(root, &["rev-parse".as_ref(), "--show-toplevel".as_ref()])?;
    // Remove Git's one line terminator, not whitespace belonging to the path.
    let repository = crate::config::normalize_platform_path(path_from_git_bytes(
        repository.strip_suffix(b"\n").unwrap_or(&repository),
    )?);
    let relative = root
        .strip_prefix(&repository)
        .context("baseline root is outside repository")?;
    let checkout = temporary.path().join("checkout");
    git(
        &repository,
        &[
            "clone".as_ref(),
            "--shared".as_ref(),
            "--no-checkout".as_ref(),
            "--template=".as_ref(),
            "--".as_ref(),
            repository.as_os_str(),
            checkout.as_os_str(),
        ],
    )?;
    git(
        &checkout,
        &[
            "checkout".as_ref(),
            "--detach".as_ref(),
            oid.trim().as_ref(),
        ],
    )?;
    let scoped = checkout.join(relative);
    if !scoped.is_dir() {
        bail!("baseline revision does not contain the analysis directory");
    }
    let scoped = crate::config::normalize_platform_path(scoped.canonicalize()?);
    if !scoped.starts_with(crate::config::normalize_platform_path(
        checkout.canonicalize()?,
    )) {
        bail!("baseline analysis directory escapes its checkout");
    }
    Ok((temporary, scoped))
}

/// Rejects changed-file mode when the analyzed root itself is excluded by Git.
pub fn ensure_changed_root_is_trackable(root: &Path) -> Result<()> {
    let root = normalize_platform_path(root.to_path_buf());
    let repository = git_at(&root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .with_context(|| "failed to locate the Git repository for changed-files mode")?;
    if !repository.status.success() {
        bail!(
            "changed-files mode requires a Git repository: {}",
            String::from_utf8_lossy(&repository.stderr).trim()
        );
    }
    let repository = normalize_platform_path(
        PathBuf::from(
            String::from_utf8(repository.stdout)
                .context("git returned a non-UTF-8 repository path")?
                .trim(),
        )
        .canonicalize()
        .with_context(|| "failed to canonicalize the Git repository root")?,
    );
    if root == repository {
        return Ok(());
    }
    let relative = root.strip_prefix(&repository).with_context(|| {
        format!(
            "target {} is outside Git repository {}",
            root.display(),
            repository.display()
        )
    })?;
    let ignored = git_at(&repository)
        .args(["check-ignore", "--quiet", "--"])
        .arg(relative)
        .status()
        .with_context(|| "failed to check whether the changed-files target is ignored")?;
    match ignored.code() {
        Some(0) => bail!(
            "changed-files target {} is ignored by Git and cannot be analyzed incrementally",
            root.display()
        ),
        Some(1) => Ok(()),
        _ => bail!("git check-ignore failed for target {}", root.display()),
    }
}

fn git_paths(root: &Path, arguments: &[&str], failure: &str) -> Result<Vec<PathBuf>> {
    let output = git_at(root)
        .args(["-c", "core.quotePath=false"])
        .args(arguments)
        .output()
        .with_context(|| "failed to run git for changed-files mode")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!("{failure}: {stderr}");
    }
    output
        .stdout
        .split(|byte| *byte == b'\0')
        .filter(|path| !path.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

#[cfg(unix)]
fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::str::from_utf8(bytes).context("git returned a non-UTF-8 file name")?,
    ))
}

/// Retains findings whose primary or related location belongs to `changed`.
pub fn retain_changed_findings(findings: &mut Vec<Finding>, changed: &BTreeSet<PathBuf>) {
    findings.retain(|finding| {
        changed.contains(&finding.primary.path)
            || finding
                .related
                .iter()
                .any(|location| changed.contains(&location.path))
    });
}

/// Schema version of the compact, reviewable baseline format.
pub const BASELINE_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Accepted semantic finding fingerprints and their multiplicities.
#[non_exhaustive]
pub struct BaselineFile {
    /// Baseline format version.
    pub schema_version: u32,
    /// Stable fingerprint counts, sorted for deterministic pull-request diffs.
    pub fingerprints: BTreeMap<String, usize>,
}

impl BaselineFile {
    /// Creates a baseline using the current schema version.
    pub fn new(fingerprints: BTreeMap<String, usize>) -> Self {
        Self {
            schema_version: BASELINE_SCHEMA_VERSION,
            fingerprints,
        }
    }

    fn empty() -> Self {
        Self::new(BTreeMap::new())
    }
}

impl Default for BaselineFile {
    fn default() -> Self {
        Self::new(BTreeMap::new())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Effect of applying or rewriting a baseline.
#[non_exhaustive]
pub struct BaselineOutcome {
    /// Findings matched and suppressed by the baseline.
    pub suppressed: usize,
    /// Active findings beyond the baseline's recorded multiplicities.
    pub new_findings: usize,
    /// Fingerprint instances added while updating.
    pub added: usize,
    /// Fingerprint instances removed while updating.
    pub removed: usize,
}

/// Suppresses findings found in a schema-compatible semantic baseline.
///
/// Multiplicity is consumed in finding order, so adding another instance of accepted
/// duplication remains visible even when its content fingerprint is already present.
pub fn apply_baseline(findings: &mut [Finding], baseline_path: &Path) -> Result<BaselineOutcome> {
    let baseline = load_baseline(baseline_path)?;
    apply_loaded_baseline(findings, baseline_path, &baseline)
}

pub(crate) fn apply_reference_baseline(
    findings: &mut [Finding],
    accepted: &[Finding],
    revision: &str,
) -> Result<BaselineOutcome> {
    apply_loaded_baseline(findings, Path::new(revision), &build_baseline(accepted))
}

/// Rewrites a baseline from the current active findings and suppresses those findings.
pub fn update_baseline(findings: &mut [Finding], baseline_path: &Path) -> Result<BaselineOutcome> {
    let previous = if baseline_path.is_file() {
        load_baseline_for_update(baseline_path)?
    } else {
        BaselineFile::empty()
    };
    let current = build_baseline(findings);
    let added = count_added(&previous.fingerprints, &current.fingerprints);
    let removed = count_added(&current.fingerprints, &previous.fingerprints);
    save_baseline(baseline_path, &current)?;
    let mut outcome = apply_loaded_baseline(findings, baseline_path, &current)?;
    outcome.added = added;
    outcome.removed = removed;
    Ok(outcome)
}

fn load_baseline(baseline_path: &Path) -> Result<BaselineFile> {
    let baseline = parse_baseline(baseline_path)?;
    if baseline.schema_version != BASELINE_SCHEMA_VERSION {
        bail!(
            "unsupported baseline schema version `{}` (expected `{}`); regenerate it with --update-baseline",
            baseline.schema_version,
            BASELINE_SCHEMA_VERSION
        );
    }
    Ok(baseline)
}

fn load_baseline_for_update(baseline_path: &Path) -> Result<BaselineFile> {
    let baseline = parse_baseline(baseline_path)?;
    if baseline.schema_version > BASELINE_SCHEMA_VERSION {
        bail!(
            "baseline schema version `{}` is newer than supported version `{}` and cannot be replaced safely",
            baseline.schema_version,
            BASELINE_SCHEMA_VERSION
        );
    }
    Ok(baseline)
}

fn parse_baseline(baseline_path: &Path) -> Result<BaselineFile> {
    let text = read_utf8(baseline_path, "baseline", MAX_BASELINE_INPUT_BYTES)?;
    // serde_json's default recursion limit remains enabled for untrusted baseline input.
    serde_json::from_str(&text)
        .with_context(|| format!("failed to parse baseline {}", baseline_path.display()))
}

fn save_baseline(path: &Path, baseline: &BaselineFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create baseline directory {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(baseline).context("failed to serialize baseline")?;
    fs::write(path, format!("{text}\n"))
        .with_context(|| format!("failed to write baseline {}", path.display()))
}

fn build_baseline(findings: &[Finding]) -> BaselineFile {
    let mut baseline = BaselineFile::empty();
    for finding in findings.iter().filter(|finding| finding.is_active()) {
        *baseline
            .fingerprints
            .entry(finding_fingerprint(finding))
            .or_default() += 1;
    }
    baseline
}

fn apply_loaded_baseline(
    findings: &mut [Finding],
    baseline_path: &Path,
    baseline: &BaselineFile,
) -> Result<BaselineOutcome> {
    let mut allowance = baseline.fingerprints.clone();
    let mut outcome = BaselineOutcome::default();
    for finding in findings {
        if !finding.is_active() {
            continue;
        }
        let fingerprint = finding_fingerprint(finding);
        match allowance.get_mut(&fingerprint) {
            Some(remaining) if *remaining > 0 => {
                *remaining -= 1;
                finding.suppression = Some(Suppression {
                    reason: format!("present in baseline {}", baseline_path.display()),
                });
                outcome.suppressed += 1;
            }
            _ => outcome.new_findings += 1,
        }
    }
    Ok(outcome)
}

/// Returns a location-independent fingerprint for baseline comparison.
pub fn finding_fingerprint(finding: &Finding) -> String {
    if let Some(cluster) = &finding.evidence.cluster {
        let rule = finding.rule.to_string();
        let mut fingerprints = cluster
            .definition_fingerprints
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        fingerprints.sort_unstable();
        return stable_fingerprint_parts(std::iter::once(rule.as_str()).chain(fingerprints));
    }
    let semantic = finding
        .evidence
        .comparison
        .as_ref()
        .map(comparison_fingerprint_input)
        .unwrap_or_else(|| {
            format!(
                "{}\u{0}{}\u{0}{}",
                finding.message,
                finding.evidence.matcher_difference,
                finding.evidence.handler_evidence
            )
        });
    stable_fingerprint(&format!("{}\u{0}{semantic}", finding.rule))
}

fn comparison_fingerprint_input(comparison: &DefinitionComparison) -> String {
    let mut sides = [
        comparison.left_fingerprint.clone(),
        comparison.right_fingerprint.clone(),
    ];
    sides.sort();
    sides.join("\u{0}")
}

fn count_added(previous: &BTreeMap<String, usize>, current: &BTreeMap<String, usize>) -> usize {
    current
        .iter()
        .map(|(fingerprint, count)| {
            count.saturating_sub(previous.get(fingerprint).copied().unwrap_or_default())
        })
        .sum()
}

#[cfg(test)]
mod tests;
