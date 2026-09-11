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
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn resolve_commit(root: &Path, base: &str) -> Result<String> {
    if base.trim().is_empty() || base.starts_with('-') {
        bail!("invalid --changed-since revision `{base}`");
    }
    let revision = format!("{base}^{{commit}}");
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
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

/// Rejects changed-file mode when the analyzed root itself is excluded by Git.
pub fn ensure_changed_root_is_trackable(root: &Path) -> Result<()> {
    let root = normalize_platform_path(root.to_path_buf());
    let repository = Command::new("git")
        .arg("-C")
        .arg(&root)
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
    let ignored = Command::new("git")
        .arg("-C")
        .arg(&repository)
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
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Accepted semantic finding fingerprints and their multiplicities.
pub struct BaselineFile {
    /// Baseline format version.
    pub schema_version: u32,
    /// Stable fingerprint counts, sorted for deterministic pull-request diffs.
    pub fingerprints: BTreeMap<String, usize>,
}

impl BaselineFile {
    fn empty() -> Self {
        Self {
            schema_version: BASELINE_SCHEMA_VERSION,
            fingerprints: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Effect of applying or rewriting a baseline.
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
mod tests {
    use super::*;
    use crate::model::{DefinitionCluster, FindingEvidence, Rule, Severity, SourceLocation};

    fn initialize_repository(root: &Path) {
        for arguments in [
            ["init", "-q"].as_slice(),
            ["config", "user.email", "test@example.com"].as_slice(),
            ["config", "user.name", "Test"].as_slice(),
            ["add", "."].as_slice(),
            ["commit", "-qm", "initial"].as_slice(),
        ] {
            assert!(Command::new("git")
                .args(arguments)
                .current_dir(root)
                .status()
                .unwrap()
                .success());
        }
    }

    fn finding(root: &Path, file: &str) -> Finding {
        Finding {
            rule: Rule::DuplicateMatcher,
            severity: Severity::Error,
            message: "duplicate".to_owned(),
            primary: SourceLocation::new(root.join(file), 2, 1, 2, 10),
            related: vec![SourceLocation::new(
                root.join("steps/shared.ts"),
                4,
                1,
                4,
                10,
            )],
            evidence: FindingEvidence {
                matcher_similarity: Some(1.0),
                handler_similarity: Some(0.5),
                matcher_difference: "same".to_owned(),
                handler_evidence: "different".to_owned(),
                comparison: None,
                cluster: None,
            },
            suggested_action: "consolidate".to_owned(),
            suppression: None,
        }
    }

    #[test]
    fn changed_mode_keeps_findings_related_to_a_changed_file() {
        let root = Path::new("/repo");
        let mut findings = vec![
            finding(root, "steps/changed.ts"),
            finding(root, "steps/old.ts"),
        ];
        let changed = BTreeSet::from([root.join("steps/changed.ts")]);
        retain_changed_findings(&mut findings, &changed);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].primary.path.ends_with("changed.ts"));
    }

    #[test]
    fn cluster_fingerprints_are_location_and_member_order_independent() {
        let root = Path::new("/repo");
        let mut original = finding(root, "steps/a.ts");
        original.evidence.cluster = Some(DefinitionCluster {
            member_count: 3,
            definition_fingerprints: vec!["charlie".into(), "alpha".into(), "bravo".into()],
            pair_findings_collapsed: 2,
            members_truncated: false,
        });
        let mut moved = finding(root, "moved/renamed.ts");
        moved.evidence.cluster = Some(DefinitionCluster {
            member_count: 3,
            definition_fingerprints: vec!["bravo".into(), "charlie".into(), "alpha".into()],
            pair_findings_collapsed: 99,
            members_truncated: false,
        });

        assert_eq!(finding_fingerprint(&original), finding_fingerprint(&moved));
        moved
            .evidence
            .cluster
            .as_mut()
            .unwrap()
            .definition_fingerprints
            .push("delta".into());
        assert_ne!(finding_fingerprint(&original), finding_fingerprint(&moved));
    }

    #[test]
    fn changed_files_are_relative_to_a_subdirectory_and_include_untracked_files() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("packages/e2e/steps")).unwrap();
        fs::write(root.join("packages/e2e/steps/träcked.ts"), "before").unwrap();
        initialize_repository(root);
        fs::write(root.join("packages/e2e/steps/träcked.ts"), "after").unwrap();
        fs::write(root.join("packages/e2e/steps/ new step.ts"), "new").unwrap();
        #[cfg(unix)]
        fs::write(root.join("packages/e2e/steps/trailing step.ts "), "new").unwrap();

        let subdirectory = root.join("packages/e2e");
        let changed = git_changed_files(&subdirectory, "HEAD").unwrap();
        let mut expected = BTreeSet::from([
            subdirectory.join("steps/ new step.ts"),
            subdirectory.join("steps/träcked.ts"),
        ]);
        #[cfg(unix)]
        expected.insert(subdirectory.join("steps/trailing step.ts "));
        assert_eq!(changed, expected);
    }

    #[cfg(unix)]
    #[test]
    fn changed_files_preserve_newlines_in_unix_names() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let steps = root.join("steps");
        fs::create_dir_all(&steps).unwrap();
        let newline = steps.join("line\nbreak.ts");
        fs::write(&newline, "before").unwrap();
        initialize_repository(root);
        fs::write(&newline, "after").unwrap();

        let changed = git_changed_files(root, "HEAD").unwrap();
        assert_eq!(changed, BTreeSet::from([newline]));
    }

    #[cfg(unix)]
    #[test]
    fn git_path_decoder_preserves_non_utf8_unix_bytes() {
        use std::os::unix::ffi::OsStrExt;

        let bytes = b"steps/non-utf8-\xff.ts";
        let path = path_from_git_bytes(bytes).unwrap();
        assert_eq!(path.as_os_str().as_bytes(), bytes);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn changed_files_preserve_non_utf8_linux_names() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let steps = root.join("steps");
        fs::create_dir_all(&steps).unwrap();
        let non_utf8 = steps.join(OsString::from_vec(b"non-utf8-\xff.ts".to_vec()));
        fs::write(&non_utf8, "before").unwrap();
        initialize_repository(root);
        fs::write(&non_utf8, "after").unwrap();

        let changed = git_changed_files(root, "HEAD").unwrap();
        assert_eq!(changed, BTreeSet::from([non_utf8]));
    }

    #[test]
    fn semantic_baseline_survives_location_changes_and_enforces_multiplicity() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let baseline_path = root.join("baseline.json");
        let mut accepted = vec![finding(root, "steps/original.ts")];
        let update = update_baseline(&mut accepted, &baseline_path).unwrap();
        assert_eq!(update.added, 1);
        assert_eq!(update.removed, 0);
        assert_eq!(update.suppressed, 1);

        let mut current = vec![
            finding(root, "moved/renamed.ts"),
            finding(root, "steps/third-copy.ts"),
        ];
        current[0].primary.line = 200;
        let outcome = apply_baseline(&mut current, &baseline_path).unwrap();
        assert_eq!(outcome.suppressed, 1);
        assert_eq!(outcome.new_findings, 1);
        assert!(!current[0].is_active());
        assert!(current[1].is_active());

        let baseline: BaselineFile =
            serde_json::from_str(&fs::read_to_string(&baseline_path).unwrap()).unwrap();
        assert_eq!(baseline.schema_version, BASELINE_SCHEMA_VERSION);
        assert_eq!(baseline.fingerprints.values().sum::<usize>(), 1);
    }

    #[test]
    fn changed_mode_validates_revisions_repositories_and_ignored_roots() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("tracked.ts"), "initial").unwrap();
        initialize_repository(directory.path());
        assert!(
            ensure_changed_root_is_trackable(&directory.path().canonicalize().unwrap()).is_ok()
        );
        for revision in ["", " ", "--all", "missing-revision"] {
            assert!(
                resolve_commit(directory.path(), revision).is_err(),
                "{revision}"
            );
        }
        assert!(git_paths(directory.path(), &["not-a-command"], "expected failure").is_err());

        let ignored = directory.path().join("ignored");
        fs::create_dir(&ignored).unwrap();
        fs::write(directory.path().join(".gitignore"), "ignored/\n").unwrap();
        assert!(
            ensure_changed_root_is_trackable(&ignored.canonicalize().unwrap())
                .unwrap_err()
                .to_string()
                .contains("ignored by Git")
        );

        let outside = tempfile::tempdir().unwrap();
        assert!(ensure_changed_root_is_trackable(outside.path()).is_err());
    }

    #[test]
    fn changed_mode_keeps_findings_with_changed_related_locations() {
        let root = Path::new("/repo");
        let mut findings = vec![finding(root, "steps/old.ts")];
        let changed = BTreeSet::from([root.join("steps/shared.ts")]);
        retain_changed_findings(&mut findings, &changed);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn baseline_validation_counts_removals_and_ignores_suppressed_findings() {
        let directory = tempfile::tempdir().unwrap();
        let baseline_path = directory.path().join("nested/baseline.json");
        let mut initial = vec![
            finding(directory.path(), "one.ts"),
            finding(directory.path(), "two.ts"),
        ];
        initial[1].message = "second duplicate".to_owned();
        update_baseline(&mut initial, &baseline_path).unwrap();

        let mut current = vec![finding(directory.path(), "one.ts")];
        current[0].suppression = Some(Suppression {
            reason: "already accepted elsewhere".to_owned(),
        });
        let outcome = update_baseline(&mut current, &baseline_path).unwrap();
        assert_eq!(outcome.added, 0);
        assert_eq!(outcome.removed, 2);
        assert_eq!(outcome.suppressed, 0);

        fs::write(&baseline_path, r#"{"schemaVersion":99,"fingerprints":{}}"#).unwrap();
        assert!(apply_baseline(&mut [], &baseline_path)
            .unwrap_err()
            .to_string()
            .contains("unsupported baseline schema"));
        fs::write(&baseline_path, "not json").unwrap();
        assert!(apply_baseline(&mut [], &baseline_path)
            .unwrap_err()
            .to_string()
            .contains("failed to parse baseline"));
    }

    #[test]
    fn comparison_fingerprints_are_independent_of_pair_order() {
        let comparison = DefinitionComparison {
            left_fingerprint: "left".to_owned(),
            right_fingerprint: "right".to_owned(),
            left_matcher: String::new(),
            right_matcher: String::new(),
            left_handler: String::new(),
            right_handler: String::new(),
            matcher_diff: crate::model::MatcherDiff {
                prefix: String::new(),
                left_change: String::new(),
                right_change: String::new(),
                suffix: String::new(),
            },
        };
        let mut reversed = comparison.clone();
        std::mem::swap(
            &mut reversed.left_fingerprint,
            &mut reversed.right_fingerprint,
        );
        assert_eq!(
            comparison_fingerprint_input(&comparison),
            comparison_fingerprint_input(&reversed)
        );
        assert_eq!(
            count_added(
                &BTreeMap::from([("a".to_owned(), 3)]),
                &BTreeMap::from([("a".to_owned(), 1)])
            ),
            0
        );
    }
}
