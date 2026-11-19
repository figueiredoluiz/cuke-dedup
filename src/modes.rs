//! Changed-files and existing-findings baseline modes for incremental CI adoption.

use crate::config::canonical_platform_path;
use crate::model::{stable_fingerprint_parts, Finding, Suppression};
use crate::resource_limits::{read_utf8, MAX_BASELINE_INPUT_BYTES};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

// Snapshotting is additional disk work, separate from the analyzer's per-source read limits.
const MAX_BASELINE_CHECKOUT_BYTES: u64 = 512 * 1024 * 1024;

/// Returns tracked changes since `base` plus untracked files beneath `root`.
pub fn git_changed_files(root: &Path, base: &str) -> Result<BTreeSet<PathBuf>> {
    let git = ChangedGit::new(root)?;
    git.ensure_trackable(root)?;
    let base_oid = resolve_commit(git.metadata_command(root), base)?;
    let tracked = git_paths(
        git.worktree_command(root),
        &[
            "diff",
            "--relative",
            "--name-only",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            // Select rename destinations without loading historical blobs for similarity scoring.
            "--no-renames",
            "--ignore-submodules=dirty",
            "--diff-filter=ACMR",
            &base_oid,
            "--",
        ],
        &format!("git diff against `{base}` failed"),
    )?;
    let untracked = git_paths(
        git.worktree_command(root),
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
        "git untracked-file discovery failed",
    )?;
    Ok(tracked
        .into_iter()
        .chain(untracked)
        .map(|path| root.join(path))
        .collect())
}

/// Removes caller Git settings, including the repository environment exported to hooks.
fn clear_git_environment(command: &mut Command) {
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
}

/// Builds an isolated Git command with an explicit checkout and no process-wide Git config.
fn isolated_git(executable: &OsStr, root: &Path, scratch: &Path) -> Command {
    let mut command = Command::new(executable);
    clear_git_environment(&mut command);
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", scratch.join("no-config"))
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_GRAFT_FILE", scratch.join("no-grafts"))
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(root)
        .arg("-c")
        .arg(git_path_setting(
            "core.hooksPath",
            &scratch.join("no-hooks"),
        ));
    command
}

/// Reads checkout metadata with Git's protected configuration and with filesystem monitors disabled.
fn checkout_metadata_git(executable: &OsStr, root: &Path, scratch: &Path) -> Command {
    let mut command = isolated_git(executable, root, scratch);
    command
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env("GIT_ALLOW_PROTOCOL", "")
        .args(["-c", "core.fsmonitor="]);
    command
}

/// Formats a Git path setting without changing the bytes of the path.
fn git_path_setting(key: &str, value: &Path) -> OsString {
    let mut setting = OsString::from(key);
    setting.push("=");
    setting.push(value.as_os_str());
    setting
}

/// Resolves Git through absolute PATH entries outside the analyzed checkout.
fn git_executable(root: &Path) -> Result<PathBuf> {
    let root = canonical_platform_path(root)?;
    let checkout = root.ancestors().find(|path| path.join(".git").exists());
    let path = std::env::var_os("PATH").context("Git requires PATH")?;
    std::env::split_paths(&path)
        .filter(|directory| directory.is_absolute())
        .filter(|directory| checkout.is_none_or(|root| !directory.starts_with(root)))
        .find_map(|directory| {
            let resolved_directory = canonical_platform_path(&directory).ok()?;
            if checkout.is_some_and(|root| resolved_directory.starts_with(root)) {
                return None;
            }
            let candidate = directory.join(if cfg!(windows) { "git.exe" } else { "git" });
            let target = canonical_platform_path(&candidate).ok()?;
            // Command dispatchers may use the invoked name to select Git.
            (checkout.is_none_or(|root| !target.starts_with(root))
                && git_file_is_executable(&target))
            .then_some(candidate)
        })
        .context("could not locate Git in PATH")
}

/// Checks that a Git executable candidate is a regular executable file on Unix.
#[cfg(unix)]
fn git_file_is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|file| file.is_file() && file.permissions().mode() & 0o111 != 0)
}

/// Checks that a Git executable candidate is a regular file on Windows.
#[cfg(not(unix))]
fn git_file_is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Uses an index snapshot and the checkout's objects with an independent worktree config.
struct ChangedGit {
    scratch: tempfile::TempDir,
    executable: PathBuf,
    repository: PathBuf,
    objects: PathBuf,
    settings: Vec<OsString>,
}

impl ChangedGit {
    /// Prepares a Git view with only the checkout settings needed to compare worktree paths.
    fn new(root: &Path) -> Result<Self> {
        let root = &canonical_platform_path(root)?;
        let scratch = tempfile::tempdir().context("cannot prepare changed-file comparison")?;
        let executable = git_executable(root)?;
        let metadata = || checkout_metadata_git(executable.as_os_str(), root, scratch.path());
        let query = |arguments: &[&str], failure: &str| {
            checked_git_output(metadata().args(arguments), failure)
        };
        let repository = canonical_platform_path(&git_output_path(&query(
            &["rev-parse", "--show-toplevel"],
            "changed-files mode requires a Git repository",
        )?)?)?;
        // Relative paths from rev-parse are relative to the directory passed to Git.
        let path = |name| -> Result<PathBuf> {
            Ok(root.join(git_output_path(&query(
                &["rev-parse", "--git-path", name],
                "failed to locate changed-file comparison metadata",
            )?)?))
        };
        let index = path("index")?;
        let objects = path("objects")?;
        // Command::output closes stdin. Hashing the empty blob does not store it.
        let hash = query(
            &["hash-object", "--stdin"],
            "failed to identify Git object format",
        )?;
        let oid = hash
            .strip_suffix(b"\n")
            .filter(|oid| oid.iter().all(u8::is_ascii_hexdigit))
            .context("unsupported Git object format for changed-file comparison")?;
        let mut init = isolated_git(executable.as_os_str(), scratch.path(), scratch.path());
        init.args(["init", "--quiet", "--bare", "--template="]);
        match oid.len() {
            40 => {}
            64 => {
                init.arg("--object-format=sha256");
            }
            _ => bail!("unsupported Git object format for changed-file comparison"),
        }
        let git_directory = scratch.path().join("git");
        init.arg(&git_directory);
        checked_git_output(&mut init, "failed to prepare changed-file comparison")?;
        // Git may refresh stat entries during diff. Let it do that in the private index.
        let snapshot = git_directory.join("index");
        match copy_git_index(&index, &snapshot) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to snapshot the Git index"),
        }
        // A non-boolean splitIndex value makes Git keep the saved index layout, as when unset.
        let shared_index = checked_git_output(
            metadata().env("GIT_INDEX_FILE", &snapshot).args([
                "-c",
                "core.splitIndex=keep",
                "rev-parse",
                "--shared-index-path",
            ]),
            "failed to locate the Git shared index",
        )?;
        if !shared_index.is_empty() {
            // Older Git versions look for the linked index only under GIT_DIR.
            let source = root.join(git_output_path(&shared_index)?);
            let name = source
                .file_name()
                .context("invalid Git shared index path")?;
            copy_git_index(&source, &git_directory.join(name))
                .context("failed to preserve the Git shared index")?;
        }
        for name in ["info/exclude", "info/attributes", "info/sparse-checkout"] {
            copy_git_info(&path(name)?, &git_directory.join(name))?;
        }
        let settings = worktree_settings(executable.as_os_str(), root, scratch.path())?;
        Ok(Self {
            scratch,
            executable,
            repository,
            objects,
            settings,
        })
    }

    /// Runs metadata queries where the operator's revision names are defined.
    fn metadata_command(&self, root: &Path) -> Command {
        checkout_metadata_git(self.executable.as_os_str(), root, self.scratch.path())
    }

    /// Reads the worktree with the private config and index and the checkout's objects.
    fn worktree_command(&self, root: &Path) -> Command {
        let scratch = self.scratch.path();
        let mut command = isolated_git(self.executable.as_os_str(), root, scratch);
        command
            .env("GIT_ALLOW_PROTOCOL", "")
            .env("GIT_DIR", scratch.join("git"))
            .env("GIT_WORK_TREE", &self.repository)
            .env("GIT_OBJECT_DIRECTORY", &self.objects)
            .args(["-c", "core.bare=false"]);
        for setting in &self.settings {
            command.arg("-c").arg(setting);
        }
        command
    }

    /// Requires the requested scope to be in the worktree and addressable by Git.
    fn ensure_trackable(&self, root: &Path) -> Result<()> {
        let root = canonical_platform_path(root)?;
        let relative = root
            .strip_prefix(&self.repository)
            .context("analysis directory is outside the Git repository")?;
        if relative.as_os_str().is_empty() {
            return Ok(());
        }
        // Capture stderr: inherited, Git's text would reach the terminal unescaped.
        let output = self
            .worktree_command(&self.repository)
            .args(["check-ignore", "--quiet", "--"])
            // Prevent a leading colon in the pathname from introducing Git pathspec magic.
            .arg(Path::new(".").join(relative))
            .output()
            .context("failed to check whether the changed-files target is ignored")?;
        match output.status.code() {
            Some(0) => bail!(
                "changed-files target {} is ignored by Git and cannot be analyzed incrementally",
                root.display()
            ),
            Some(1) => Ok(()),
            _ => bail!(
                "git check-ignore failed for target {}: {}",
                root.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }
}

/// Reads allowlisted worktree settings using Git's normal configuration precedence.
fn worktree_settings(executable: &OsStr, root: &Path, scratch: &Path) -> Result<Vec<OsString>> {
    let output = checkout_metadata_git(executable, root, scratch)
        .args(["config", "--null", "--get-regexp", r"^(core\.(filemode|symlinks|ignorecase|precomposeunicode|autocrlf|eol|safecrlf|excludesfile|attributesfile|checkstat|trustctime|sparsecheckout|sparsecheckoutcone|longpaths)|index\.sparse)$"])
        .output().context("failed to read Git worktree settings")?;
    // Git returns 1 without output when no selected settings exist.
    if output.status.code() == Some(1) && output.stderr.is_empty() && output.stdout.is_empty() {
        return Ok(Vec::new());
    }
    if !output.status.success() {
        bail!("failed to read Git worktree settings");
    }
    output
        .stdout
        .strip_suffix(b"\0")
        .context("invalid Git worktree settings")?
        .split(|byte| *byte == b'\0')
        .map(|record| {
            let mut setting = record.to_vec();
            if let Some(separator) = setting.iter_mut().find(|byte| **byte == b'\n') {
                *separator = b'=';
            }
            Ok(path_from_git_bytes(&setting)?.into_os_string())
        })
        .collect()
}

/// Copies a Git index and preserves its modification time for Git's stat comparisons.
fn copy_git_index(source: &Path, destination: &Path) -> std::io::Result<()> {
    let mut input = File::open(source)?;
    let metadata = input.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Git index is not a regular file",
        ));
    }
    let mut output = File::create(destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.set_times(fs::FileTimes::new().set_modified(metadata.modified()?))
}

/// Copies a bounded Git metadata file into the private comparison directory.
fn copy_git_info(source: &Path, destination: &Path) -> Result<()> {
    const LIMIT: u64 = 1024 * 1024;
    let metadata = match source.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        bail!("Git comparison metadata must be a regular file of at most 1 MiB");
    }
    let mut bytes = Vec::new();
    File::open(source)?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        bail!("Git comparison metadata must be a regular file of at most 1 MiB");
    }
    let directory = destination
        .parent()
        .context("missing Git metadata directory")?;
    fs::create_dir_all(directory)?;
    fs::write(destination, bytes)?;
    Ok(())
}

/// Runs a Git command and reports failure before consuming its output.
fn checked_git_output(command: &mut Command, failure: &str) -> Result<Vec<u8>> {
    let output = command.output().with_context(|| failure.to_owned())?;
    if !output.status.success() {
        bail!(
            "{failure}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

/// Decodes Git's single path output, preserving whitespace that belongs to the name.
fn git_output_path(bytes: &[u8]) -> Result<PathBuf> {
    path_from_git_bytes(bytes.strip_suffix(b"\n").context("missing path from Git")?)
}

/// Peels `base` to a commit and returns its object id.
///
/// Rejects a blank or `-`-prefixed `base` before running Git; errors when `rev-parse` fails or
/// prints anything other than a hex object id.
fn resolve_commit(mut git: Command, base: &str) -> Result<String> {
    if base.trim().is_empty() || base.starts_with('-') {
        bail!("invalid --changed-since revision `{base}`");
    }
    let revision = format!("{base}^{{commit}}");
    let output = git
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
    if !matches!(oid.len(), 40 | 64) || !oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
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
    let executable = git_executable(root)?;
    let temporary = tempfile::tempdir().context("cannot create temporary baseline checkout")?;
    let source_git = |directory: &Path| {
        checkout_metadata_git(executable.as_os_str(), directory, temporary.path())
    };
    let git = |mut command: Command, args: &[&OsStr]| -> Result<Vec<u8>> {
        command.args(["-c", "core.fsmonitor=false"]).args(args);
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
        source_git(root),
        &[
            "rev-parse".as_ref(),
            "--verify".as_ref(),
            "--end-of-options".as_ref(),
            format!("{revision}^{{commit}}").as_ref(),
        ],
    )?;
    let oid = String::from_utf8(oid).context("invalid baseline commit id")?;
    let tree = git(
        source_git(root),
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
    let repository = git(
        source_git(root),
        &["rev-parse".as_ref(), "--show-toplevel".as_ref()],
    )?;
    // Remove Git's one line terminator, not whitespace belonging to the path.
    let repository = crate::config::normalize_platform_path(path_from_git_bytes(
        repository.strip_suffix(b"\n").unwrap_or(&repository),
    )?);
    let relative = root
        .strip_prefix(&repository)
        .context("baseline root is outside repository")?;
    let checkout = temporary.path().join("checkout");
    // The source uses the operator's trust settings. Only a local transport is needed.
    let mut clone = source_git(&repository);
    clone.env("GIT_ALLOW_PROTOCOL", "file");
    git(
        clone,
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
    // Materialize the clone with its independent config and no checkout hooks.
    git(
        isolated_git(executable.as_os_str(), &checkout, temporary.path()),
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
    let scoped = crate::config::canonical_platform_path(&scoped)?;
    if !scoped.starts_with(crate::config::canonical_platform_path(&checkout)?) {
        bail!("baseline analysis directory escapes its checkout");
    }
    Ok((temporary, scoped))
}

/// Rejects changed-file mode when the analyzed root itself is excluded by Git.
pub fn ensure_changed_root_is_trackable(root: &Path) -> Result<()> {
    ChangedGit::new(root)?.ensure_trackable(root)
}

/// Runs a Git path query and decodes its NUL-delimited output.
fn git_paths(mut git: Command, arguments: &[&str], failure: &str) -> Result<Vec<PathBuf>> {
    let bytes = checked_git_output(git.args(arguments), failure)?;
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    bytes
        .strip_suffix(b"\0")
        .context("invalid Git path listing")?
        .split(|byte| *byte == b'\0')
        .map(|bytes| {
            let path = path_from_git_bytes(bytes)?;
            if path.as_os_str().is_empty()
                || !path
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                bail!("invalid relative path in Git listing");
            }
            Ok(path)
        })
        .collect()
}

/// Converts a raw Git path to a `PathBuf` byte for byte; never fails on Unix.
#[cfg(unix)]
fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

/// Converts a Git path to a `PathBuf` on non-Unix targets; errors when the bytes are not UTF-8.
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
pub const BASELINE_SCHEMA_VERSION: u32 = 3;

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

    /// Returns a baseline with no fingerprints at the current schema version.
    fn empty() -> Self {
        Self::new(BTreeMap::new())
    }
}

impl Default for BaselineFile {
    /// Returns a baseline with no fingerprints at the current schema version.
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

/// Suppresses `findings` against a baseline built from the active findings in `accepted`.
///
/// `revision` stands in for the baseline path, so suppression reasons read
/// `present in baseline <revision>`.
pub(crate) fn apply_reference_baseline(
    findings: &mut [Finding],
    accepted: &[Finding],
    revision: &str,
) -> Result<BaselineOutcome> {
    apply_loaded_baseline(findings, Path::new(revision), &build_baseline(accepted))
}

/// Rewrites a baseline from the current active findings and suppresses those findings.
pub fn update_baseline(findings: &mut [Finding], baseline_path: &Path) -> Result<BaselineOutcome> {
    let destination = crate::output_file::OutputFile::open(baseline_path, true)?;
    let previous = if let Some(file) = destination.previous() {
        load_baseline_for_update(baseline_path, file)?
    } else {
        BaselineFile::empty()
    };
    let current = build_baseline(findings);
    let added = count_added(&previous.fingerprints, &current.fingerprints);
    let removed = count_added(&current.fingerprints, &previous.fingerprints);
    let mut text = serde_json::to_vec_pretty(&current).context("failed to serialize baseline")?;
    text.push(b'\n');
    destination
        .replace(&text)
        .with_context(|| format!("failed to write baseline {}", baseline_path.display()))?;
    let mut outcome = apply_loaded_baseline(findings, baseline_path, &current)?;
    outcome.added = added;
    outcome.removed = removed;
    Ok(outcome)
}

/// Parses a baseline to apply; errors unless its schema version equals `BASELINE_SCHEMA_VERSION`.
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

/// Parses a baseline that is about to be replaced; accepts older schema versions and errors on a
/// newer one.
fn load_baseline_for_update(baseline_path: &Path, file: &std::fs::File) -> Result<BaselineFile> {
    let text = crate::resource_limits::read_file_utf8(
        file.try_clone()?,
        baseline_path,
        "baseline",
        MAX_BASELINE_INPUT_BYTES,
    )?;
    let baseline = decode_baseline(&text, baseline_path)?;
    if baseline.schema_version > BASELINE_SCHEMA_VERSION {
        bail!(
            "baseline schema version `{}` is newer than supported version `{}` and cannot be replaced safely",
            baseline.schema_version,
            BASELINE_SCHEMA_VERSION
        );
    }
    Ok(baseline)
}

/// Reads and deserializes a baseline without checking its schema version.
///
/// Errors on I/O failure, input over `MAX_BASELINE_INPUT_BYTES`, invalid UTF-8, or invalid JSON.
fn parse_baseline(baseline_path: &Path) -> Result<BaselineFile> {
    let text = read_utf8(baseline_path, "baseline", MAX_BASELINE_INPUT_BYTES)?;
    decode_baseline(&text, baseline_path)
}

/// Decodes one bounded semantic baseline with path context on malformed input.
fn decode_baseline(text: &str, baseline_path: &Path) -> Result<BaselineFile> {
    // serde_json's default recursion limit remains enabled for untrusted baseline input.
    serde_json::from_str(text)
        .with_context(|| format!("failed to parse baseline {}", baseline_path.display()))
}

/// Counts active findings per `finding_fingerprint`; suppressed and `Off` findings are left out.
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

/// Suppresses each active finding whose fingerprint has multiplicity left in `baseline`,
/// consuming one per match in finding order; unmatched active findings count as new.
///
/// Inactive findings are skipped. Never returns `Err`.
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
    let rule = finding.rule.to_string();
    if let Some(cluster) = &finding.evidence.cluster {
        let mut fingerprints = cluster
            .definition_fingerprints
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        fingerprints.sort_unstable();
        return stable_fingerprint_parts(
            ["finding-cluster", rule.as_str()]
                .into_iter()
                .chain(fingerprints),
        );
    }
    if let Some(comparison) = &finding.evidence.comparison {
        let mut sides = [
            comparison.left_fingerprint.as_str(),
            comparison.right_fingerprint.as_str(),
        ];
        sides.sort_unstable();
        return stable_fingerprint_parts(
            ["finding-comparison", rule.as_str()]
                .into_iter()
                .chain(sides),
        );
    }
    if let Some(original) = &finding.original_fingerprint {
        return crate::model::fingerprint_hex(original);
    }
    stable_fingerprint_parts([
        "finding-fallback",
        rule.as_str(),
        &finding.message,
        &finding.evidence.matcher_difference,
        &finding.evidence.handler_evidence,
    ])
}

/// Sums, per fingerprint, the instances `current` has beyond `previous`; fingerprints present
/// only in `previous` contribute nothing.
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
