//! Changed-files and existing-findings baseline modes for incremental CI adoption.

use crate::config::canonical_platform_path;
use crate::model::{stable_fingerprint_parts, Finding, Rule, StepDefinition, Suppression};
use crate::resource_limits::{
    LimitedWriter, MAX_BASELINE_INPUT_BYTES, MAX_CHANGED_PATH_BYTES, MAX_CHANGED_PATH_RECORDS,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdout, Command, ExitStatus, Stdio};

mod git_input;

// Snapshotting is additional disk work, separate from the analyzer's per-source read limits.
const MAX_BASELINE_CHECKOUT_BYTES: u64 = 512 * 1024 * 1024;

/// Returns tracked changes since `base` plus untracked files beneath `root`.
pub fn git_changed_files(root: &Path, base: &str) -> Result<BTreeSet<PathBuf>> {
    git_changed_files_with_limits(root, base, MAX_CHANGED_PATH_RECORDS, MAX_CHANGED_PATH_BYTES)
}

/// Runs both native listings under one allowance, returning a set only if both succeed.
fn git_changed_files_with_limits(
    root: &Path,
    base: &str,
    records: usize,
    bytes: usize,
) -> Result<BTreeSet<PathBuf>> {
    let mut budget = ChangedPathBudget { records, bytes };
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
        root,
        &mut budget,
    )?;
    let untracked = git_paths(
        git.worktree_command(root),
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
        "git untracked-file discovery failed",
        root,
        &mut budget,
    )?;
    Ok(tracked.into_iter().chain(untracked).collect())
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
    fn new(requested_root: &Path) -> Result<Self> {
        let root = &canonical_platform_path(requested_root)?;
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
        for name in ["info/exclude", "info/attributes", "info/sparse-checkout"] {
            copy_git_info(&path(name)?, &git_directory.join(name))?;
        }
        snapshot_shared_index(
            executable.as_os_str(),
            &index,
            &git_directory,
            &objects,
            oid.len(),
        )?;
        #[cfg(windows)]
        let alias = git_input::trusted_checkout_alias(requested_root, root, &repository);
        #[cfg(not(windows))]
        let alias: Option<PathBuf> = None;
        let settings = worktree_settings(
            executable.as_os_str(),
            &repository,
            alias.as_deref(),
            scratch.path(),
        )?;
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

/// Checks the private index and copies its one shared counterpart if requested by Git.
fn snapshot_shared_index(
    executable: &OsStr,
    original_index: &Path,
    directory: &Path,
    objects: &Path,
    oid_len: usize,
) -> Result<()> {
    let mut probe = isolated_git(executable, directory, directory);
    probe
        .env("GIT_DIR", ".")
        .env("GIT_OBJECT_DIRECTORY", objects)
        .env("LC_ALL", "C")
        .args([
            "-c",
            "core.splitIndex=keep",
            "rev-parse",
            "--shared-index-path",
        ]);
    let mut inspect = || -> Result<_> {
        let (_, status, stderr) = read_git_process(
            &mut probe,
            "failed to inspect the private Git index",
            |mut stdout| Ok(std::io::copy(&mut stdout, &mut std::io::sink())?),
        )?;
        Ok((status, stderr))
    };
    let (status, stderr) = inspect()?;
    if status.success() {
        return Ok(());
    }
    // Git 2.31 and 2.54 first read the referenced shared index, even for this path query.
    // Probe only in scratch, and use the diagnostic as a filename hint for a bounded copy.
    let name = (status.code() == Some(128))
        .then(|| shared_index_hint(&stderr, directory, oid_len))
        .flatten()
        .with_context(|| {
            format!(
                "failed to inspect the private Git index: {}",
                String::from_utf8_lossy(&stderr).trim()
            )
        })?;
    copy_git_index(&original_index.with_file_name(&name), &directory.join(name))
        .context("failed to snapshot the Git shared index")?;
    let (status, stderr) = inspect()?;
    if !status.success() {
        bail!(
            "failed to inspect the private Git shared index: {}",
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    Ok(())
}

/// Extracts a single shared-index filename from Git's missing-file diagnostic in scratch.
fn shared_index_hint(stderr: &[u8], directory: &Path, oid_len: usize) -> Option<OsString> {
    const MARKER: &[u8] = b": index file open failed:";
    let mut names = stderr.split(|byte| *byte == b'\n').filter_map(|line| {
        let line = line.strip_prefix(b"fatal: ")?;
        let end = line
            .windows(MARKER.len())
            .rposition(|part| part == MARKER)?;
        let path = crate::config::normalize_platform_path(path_from_git_bytes(&line[..end]).ok()?);
        let name = path.file_name()?;
        let digest = name.to_str()?.strip_prefix("sharedindex.")?;
        let in_scratch = directory.join(&path) == directory.join(name);
        (in_scratch
            && digest.len() == oid_len
            && digest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| name.to_owned())
    });
    let name = names.next()?;
    names.next().is_none().then_some(name)
}

const GIT_FILE_SETTINGS: [&str; 2] = ["core.excludesfile", "core.attributesfile"];

/// Captures comparison settings and scoped file selections in one bounded Git query.
fn worktree_settings(
    executable: &OsStr,
    repository: &Path,
    alias: Option<&Path>,
    scratch: &Path,
) -> Result<Vec<OsString>> {
    const INVALID: &str = "invalid Git worktree settings";
    let mut git = checkout_metadata_git(executable, repository, scratch);
    git.args(["config", "--null", "--show-scope", "--get-regexp", r"^(core\.(filemode|symlinks|ignorecase|precomposeunicode|autocrlf|eol|safecrlf|checkstat|trustctime|sparsecheckout|sparsecheckoutcone|longpaths|excludesfile|attributesfile)|index\.sparse)$"]);
    let Some(bytes) =
        read_worktree_configuration(&mut git, "failed to read Git worktree settings")?
    else {
        return Ok(Vec::new());
    };
    let fields: Vec<_> = bytes
        .strip_suffix(b"\0")
        .context(INVALID)?
        .split(|byte| *byte == 0)
        .collect();
    let (entries, incomplete) = fields.as_chunks::<2>();
    if !incomplete.is_empty() {
        bail!(INVALID);
    }

    let mut settings = Vec::new();
    for &[_, record] in entries {
        let separator = record.iter().position(|byte| *byte == b'\n');
        let key = &record[..separator.unwrap_or(record.len())];
        if GIT_FILE_SETTINGS
            .iter()
            .any(|setting| key == setting.as_bytes())
        {
            continue;
        }
        let mut setting = record.to_vec();
        if let Some(separator) = separator {
            setting[separator] = b'=';
        }
        settings.push(path_from_git_bytes(&setting)?.into_os_string());
    }
    snapshot_configured_git_inputs(
        executable,
        repository,
        alias,
        scratch,
        entries,
        &mut settings,
    )?;
    Ok(settings)
}

/// Collects bounded Git configuration output and distinguishes no value from a query failure.
fn read_worktree_configuration(git: &mut Command, failure: &str) -> Result<Option<Vec<u8>>> {
    const LIMIT: u64 = 256 * 1024;
    let (stdout, status, stderr) = read_git_process(git, failure, |stdout| {
        let mut bytes = Vec::new();
        stdout.take(LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT {
            bail!("Git worktree settings exceed the 256 KiB limit");
        }
        Ok(bytes)
    })?;
    // Git returns 1 without output when no selected settings exist.
    if status.code() == Some(1) && stderr.is_empty() && stdout.is_empty() {
        return Ok(None);
    }
    if !status.success() {
        bail!("{failure}");
    }
    Ok(Some(stdout))
}

/// Snapshots selected Git files, confining local and worktree settings to the checkout.
fn snapshot_configured_git_inputs(
    executable: &OsStr,
    repository: &Path,
    alias: Option<&Path>,
    scratch: &Path,
    entries: &[[&[u8]; 2]],
    settings: &mut Vec<OsString>,
) -> Result<()> {
    for key in GIT_FILE_SETTINGS {
        // Git prints matching records in read order. The last value for each key wins.
        let Some((scope, value)) = entries.iter().rev().find_map(|&[scope, record]| {
            let mut record = record.splitn(2, |byte| *byte == b'\n');
            (record.next() == Some(key.as_bytes())).then(|| (scope, record.next()))
        }) else {
            continue;
        };
        let repository_owned = match scope {
            b"local" | b"worktree" => true,
            b"global" | b"system" => false,
            _ => bail!("cannot verify the Git configuration scope for {key}"),
        };
        let value = value.with_context(|| format!("Git setting {key} needs a path"))?;
        let (path, optional) = value
            .strip_prefix(b":(optional)")
            .map_or((value, false), |path| (path, true));
        let path = expand_captured_git_path(executable, scratch, key, path)?;
        let destination = scratch.join(key);
        if git_input_is_disabled(&path) {
            if !optional || !path.as_os_str().is_empty() {
                settings.push(git_path_setting(key, &destination));
            }
            continue;
        }
        let source = configured_git_input_source(repository, &path)?;
        let kind = format!("Git setting {key}");
        let opened = if repository_owned {
            git_input::open_repository_input(repository, alias, &source)
        } else {
            open_operator_git_input(&source)
        }
        .with_context(|| format!("failed to open {kind}"))?;
        if let Some(opened) = opened {
            let unanchored = (!repository_owned).then_some(source.as_path());
            copy_bounded_git_metadata(unanchored, &destination, opened, &kind)?;
        } else if optional {
            continue; // An absent optional value leaves Git's implicit default active.
        }
        settings.push(git_path_setting(key, &destination));
    }
    Ok(())
}

/// Expands one captured raw value using Git's path interpolation without rereading configuration.
fn expand_captured_git_path(
    executable: &OsStr,
    scratch: &Path,
    key: &str,
    raw: &[u8],
) -> Result<PathBuf> {
    let mut default = OsString::from("--default=");
    // The caller removed the optional marker so Git cannot inspect the target during expansion.
    if raw.starts_with(b":(optional)") {
        default.push("./");
    }
    default.push(path_from_git_bytes(raw)?.as_os_str());
    let mut git = isolated_git(executable, scratch, scratch);
    git.args(["config", "--file"])
        .arg(scratch.join("git/config"))
        .args(["--null", "--path"])
        .arg(default)
        .args(["--get", key]);
    let failure = format!("failed to resolve Git setting {key}");
    let bytes = read_worktree_configuration(&mut git, &failure)?.context(failure.clone())?;
    let path = bytes
        .strip_suffix(b"\0")
        .filter(|bytes| !bytes.contains(&0))
        .context(failure)?;
    path_from_git_bytes(path)
}

/// Opens a regular file selected by Git's system or operator configuration.
fn open_operator_git_input(source: &Path) -> Result<Option<File>> {
    match source.metadata() {
        Ok(metadata) if metadata.is_file() => {
            Ok(Some(crate::contained_read::open_nonblocking(source)?))
        }
        Ok(_) => bail!("must select a regular file of at most 1 MiB"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Recognizes explicit disabled Git file inputs without consulting a device.
fn git_input_is_disabled(path: &Path) -> bool {
    path.as_os_str().is_empty()
        || path == Path::new("/dev/null")
        || (cfg!(windows) && path.as_os_str().eq_ignore_ascii_case("NUL"))
}

/// Resolves a configured Git input relative to the worktree root before opening it.
#[cfg(not(windows))]
fn configured_git_input_source(repository: &Path, path: &Path) -> Result<PathBuf> {
    Ok(repository.join(path))
}

/// Admits Windows disk and file-share paths before any filesystem operation.
#[cfg(windows)]
fn configured_git_input_source(repository: &Path, path: &Path) -> Result<PathBuf> {
    use std::path::{Component, Prefix};

    if !path.has_root() && matches!(path.components().next(), Some(Component::Prefix(_))) {
        bail!("configured Git input must use a Windows filesystem path with a stable base");
    }
    let source = std::path::absolute(repository.join(path))?;
    let supported = match source.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(_) | Prefix::VerbatimDisk(_) => true,
            Prefix::UNC(_, share) | Prefix::VerbatimUNC(_, share) => {
                !share.eq_ignore_ascii_case("pipe") && !share.eq_ignore_ascii_case("mailslot")
            }
            _ => false, // fail-closed: other namespaces do not identify ordinary filesystem paths
        },
        _ => false, // fail-closed: a Windows absolute source needs a disk or share prefix
    };
    if !supported || source.file_name().is_some_and(windows_git_device_name) {
        bail!("configured Git input must use a Windows filesystem path");
    }
    Ok(source)
}

/// Identifies reserved DOS device basenames, including extension and stream spellings.
#[cfg(windows)]
fn windows_git_device_name(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    let basename = name
        .split(['.', ':'])
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(
        basename.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].into_iter().any(|prefix| {
        basename.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(suffix.as_bytes(), [b'1'..=b'9']) || matches!(suffix, "¹" | "²" | "³")
        })
    })
}

/// Copies a regular Git index of at most 64 MiB and preserves its modification time for stat comparisons.
fn copy_git_index(source: &Path, destination: &Path) -> std::io::Result<()> {
    const LIMIT: u64 = 64 * 1024 * 1024;
    const LIMIT_ERROR: &str = "Git index exceeds the 64 MiB comparison limit";
    let input = crate::contained_read::open_nonblocking(source)?;
    let metadata = input.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Git index is not a regular file",
        ));
    }
    if metadata.len() > LIMIT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            LIMIT_ERROR,
        ));
    }
    let parent = destination
        .parent()
        .ok_or(std::io::ErrorKind::InvalidInput)?;
    let mut output = tempfile::NamedTempFile::new_in(parent)?;
    copy_git_bytes(input, &mut output, LIMIT, LIMIT_ERROR)?;
    output
        .as_file()
        .set_times(fs::FileTimes::new().set_modified(metadata.modified()?))?;
    output
        .persist(destination)
        .map(|_| ())
        .map_err(|error| error.error)
}

/// Copies a bounded Git metadata file into the private comparison directory.
fn copy_git_info(source: &Path, destination: &Path) -> Result<()> {
    const KIND: &str = "Git comparison metadata";
    let metadata = match source.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let input = metadata
        .is_file()
        .then(|| crate::contained_read::open_nonblocking(source))
        .transpose()?;
    match input {
        Some(input) if source.symlink_metadata()?.is_file() => {
            copy_bounded_git_metadata(Some(source), destination, input, KIND)
        }
        _ => bail!("{KIND} must be a regular file of at most 1 MiB"),
    }
}

/// Copies a verified regular Git metadata handle with a byte limit independent of its size hint.
fn copy_bounded_git_metadata(
    unanchored: Option<&Path>,
    destination: &Path,
    input: File,
    kind: &str,
) -> Result<()> {
    const LIMIT: u64 = 1024 * 1024;
    let opened = input.metadata()?;
    let refusal = format!("{kind} must be a regular file of at most 1 MiB");
    if !opened.is_file() || opened.len() > LIMIT {
        bail!("{refusal}");
    }
    if let Some(source) = unanchored {
        if !crate::contained_read::same_file_as(&input, source)? {
            bail!("{refusal}");
        }
    }
    let mut bytes = Vec::new();
    copy_git_bytes(input, &mut bytes, LIMIT, &refusal)?;
    let directory = destination
        .parent()
        .context("missing Git metadata directory")?;
    fs::create_dir_all(directory)?;
    fs::write(destination, bytes)?;
    Ok(())
}

/// Copies at most the Git input limit and checks that the input ended without writing excess bytes.
fn copy_git_bytes(
    mut input: impl Read,
    mut output: impl Write,
    limit: u64,
    refusal: &str,
) -> std::io::Result<()> {
    std::io::copy(&mut input.by_ref().take(limit), &mut output)?;
    if std::io::copy(&mut input.take(1), &mut std::io::sink())? != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            refusal.to_owned(),
        ));
    }
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

/// Counts every Git record and byte of its absolute path before collection.
struct ChangedPathBudget {
    records: usize,
    bytes: usize,
}

/// Reads one listing, draining bounded diagnostics independently and reaping a stopped child.
fn git_paths(
    mut git: Command,
    arguments: &[&str],
    failure: &str,
    root: &Path,
    budget: &mut ChangedPathBudget,
) -> Result<Vec<PathBuf>> {
    let (paths, status, stderr) = read_git_process(git.args(arguments), failure, |stdout| {
        read_git_paths(BufReader::new(stdout), root, budget)
    })?;
    if !status.success() {
        bail!("{failure}: {}", String::from_utf8_lossy(&stderr).trim());
    }
    Ok(paths)
}

/// Runs a Git query with a caller-supplied bounded stdout reader and capped diagnostic retention.
/// Stops and reaps the process if stdout cannot be read completely.
fn read_git_process<T>(
    git: &mut Command,
    failure: &str,
    read_stdout: impl FnOnce(ChildStdout) -> Result<T>,
) -> Result<(T, ExitStatus, Vec<u8>)> {
    let mut child = git
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| failure.to_owned())?;
    let result = (|| {
        let stdout = child.stdout.take().context("missing Git stdout")?;
        let stderr = child.stderr.take().context("missing Git stderr")?;
        let diagnostics =
            std::thread::Builder::new().spawn(move || read_git_diagnostics(stderr))?;
        let data = read_stdout(stdout);
        if data.is_err() {
            let _ = child.kill();
        }
        let status = child.wait().with_context(|| failure.to_owned())?;
        let stderr = diagnostics
            .join()
            .ok()
            .context("Git diagnostic reader failed")??;
        Ok((data?, status, stderr))
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

/// Keeps only the first 8 KiB of Git diagnostics while draining the pipe.
fn read_git_diagnostics(mut reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut prefix = Vec::new();
    reader.by_ref().take(8 * 1024).read_to_end(&mut prefix)?;
    std::io::copy(&mut reader, &mut std::io::sink())?;
    Ok(prefix)
}

/// Streams NUL-delimited paths and refuses an over-limit record before decoding or retaining it.
fn read_git_paths(
    mut reader: impl BufRead,
    root: &Path,
    budget: &mut ChangedPathBudget,
) -> Result<Vec<PathBuf>> {
    let root_prefix = root.join("x").as_os_str().len() - 1;
    let mut paths = Vec::new();
    let mut record = Vec::new();
    loop {
        record.clear();
        let max_record = if budget.records == 0 {
            1
        } else {
            budget.bytes.saturating_sub(root_prefix).saturating_add(1)
        };
        let read = reader
            .by_ref()
            .take(max_record as u64)
            .read_until(0, &mut record)?;
        if read == 0 {
            return Ok(paths);
        }
        let cost = root_prefix.saturating_add(read);
        if budget.records == 0 || cost > budget.bytes {
            bail!("Git changed-path listing exceeds the record or byte limit");
        }
        budget.records -= 1;
        budget.bytes -= cost;
        let bytes = record
            .strip_suffix(b"\0")
            .context("invalid Git path listing")?;
        let path = path_from_git_bytes(bytes)?;
        if path.as_os_str().is_empty()
            || !path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            bail!("invalid relative path in Git listing");
        }
        paths.push(root.join(path));
    }
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

/// Resolves baseline identity from a finding's definition file when comparison spans suites.
pub(crate) struct BaselineDomains<'a>(BTreeMap<&'a Path, &'static str>);

impl<'a> BaselineDomains<'a> {
    /// Indexes extracted definition files by their analysis domain.
    pub(crate) fn new(definitions: &'a [StepDefinition]) -> Self {
        Self(
            definitions
                .iter()
                .map(|definition| {
                    (
                        definition.location.path.as_path(),
                        definition.analysis_profile().domain.label,
                    )
                })
                .collect(),
        )
    }
}

/// Wraps the ordinary fingerprint in a domain key when suite isolation is required.
fn baseline_key(finding: &Finding, domains: Option<&BaselineDomains<'_>>) -> Result<String> {
    let fingerprint = finding_fingerprint(finding);
    let Some(domains) = domains else {
        return Ok(fingerprint);
    };
    // An ambiguous feature step is anchored to its first matched definition.
    let source = if finding.rule == Rule::AmbiguousStep {
        finding
            .related
            .first()
            .context("ambiguous finding has no matched definition")?
    } else {
        &finding.primary
    };
    let domain = domains
        .0
        .get(source.path.as_path())
        .context("finding has no baseline comparison domain")?;
    Ok(stable_fingerprint_parts([
        "finding-domain",
        domain,
        &fingerprint,
    ]))
}

/// Suppresses findings found in a schema-compatible semantic baseline.
///
/// Multiplicity is consumed in finding order, so adding another instance of accepted
/// duplication remains visible even when its content fingerprint is already present.
pub fn apply_baseline(findings: &mut [Finding], baseline_path: &Path) -> Result<BaselineOutcome> {
    apply_baseline_with_domains(findings, baseline_path, None)
}

/// Applies a saved file using either domain-specific keys or the ordinary public keys.
pub(crate) fn apply_baseline_with_domains(
    findings: &mut [Finding],
    baseline_path: &Path,
    domains: Option<&BaselineDomains<'_>>,
) -> Result<BaselineOutcome> {
    let baseline = load_baseline(baseline_path)?;
    apply_loaded_baseline(findings, baseline_path, &baseline, domains)
}

/// Suppresses `findings` against a baseline built from the active findings in `accepted`.
///
/// `revision` stands in for the baseline path, so suppression reasons read
/// `present in baseline <revision>`.
pub(crate) fn apply_reference_baseline(
    findings: &mut [Finding],
    accepted: &[Finding],
    revision: &str,
    domains: Option<(&BaselineDomains<'_>, &BaselineDomains<'_>)>,
) -> Result<BaselineOutcome> {
    let (live, reference) = domains.unzip();
    let baseline = build_baseline(accepted, reference)?;
    apply_loaded_baseline(findings, Path::new(revision), &baseline, live)
}

/// Rewrites a baseline from the current active findings and suppresses those findings.
pub fn update_baseline(findings: &mut [Finding], baseline_path: &Path) -> Result<BaselineOutcome> {
    update_baseline_with_domains(findings, baseline_path, None)
}

/// Rewrites the saved baseline with the identity used by this run.
pub(crate) fn update_baseline_with_domains(
    findings: &mut [Finding],
    baseline_path: &Path,
    domains: Option<&BaselineDomains<'_>>,
) -> Result<BaselineOutcome> {
    update_baseline_with_limit(
        findings,
        baseline_path,
        MAX_BASELINE_INPUT_BYTES as usize,
        domains,
    )
}

/// Stages a baseline within the read ceiling before replacing it or suppressing any findings.
fn update_baseline_with_limit(
    findings: &mut [Finding],
    baseline_path: &Path,
    max_bytes: usize,
    domains: Option<&BaselineDomains<'_>>,
) -> Result<BaselineOutcome> {
    let destination = crate::output_file::OutputFile::open(baseline_path, true)?;
    let previous = if let Some(file) = destination.previous() {
        load_baseline_for_update(baseline_path, file)?
    } else {
        BaselineFile::empty()
    };
    let current = build_baseline(findings, domains)?;
    let added = count_added(&previous.fingerprints, &current.fingerprints);
    let removed = count_added(&current.fingerprints, &previous.fingerprints);
    let stage = destination.stage(|out| {
        let mut writer = LimitedWriter {
            inner: out,
            remaining: max_bytes,
        };
        serde_json::to_writer_pretty(&mut writer, &current)?;
        std::io::Write::write_all(&mut writer, b"\n")?;
        Ok(())
    });
    stage
        .and_then(crate::output_file::Staged::commit)
        .with_context(|| format!("failed to write baseline {}", baseline_path.display()))?;
    let mut outcome = apply_loaded_baseline(findings, baseline_path, &current, domains)?;
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
    let file = crate::contained_read::open_nonblocking(baseline_path)
        .with_context(|| format!("failed to read baseline {}", baseline_path.display()))?;
    let inspect_context = format!("failed to inspect baseline {}", baseline_path.display());
    if !file.metadata().context(inspect_context)?.is_file() {
        bail!("baseline {} is not a regular file", baseline_path.display());
    }
    let text = crate::resource_limits::read_file_utf8(
        file,
        baseline_path,
        "baseline",
        MAX_BASELINE_INPUT_BYTES,
    )?;
    decode_baseline(&text, baseline_path)
}

/// Decodes one bounded semantic baseline with path context on malformed input.
fn decode_baseline(text: &str, baseline_path: &Path) -> Result<BaselineFile> {
    // serde_json's default recursion limit remains enabled for untrusted baseline input.
    serde_json::from_str(text)
        .with_context(|| format!("failed to parse baseline {}", baseline_path.display()))
}

/// Counts active findings per baseline key; suppressed and `Off` findings are left out.
fn build_baseline(
    findings: &[Finding],
    domains: Option<&BaselineDomains<'_>>,
) -> Result<BaselineFile> {
    let mut baseline = BaselineFile::empty();
    for finding in findings.iter().filter(|finding| finding.is_active()) {
        *baseline
            .fingerprints
            .entry(baseline_key(finding, domains)?)
            .or_default() += 1;
    }
    Ok(baseline)
}

/// Suppresses each active finding whose fingerprint has multiplicity left in `baseline`,
/// consuming one per match in finding order; unmatched active findings count as new.
///
/// Skips inactive findings and rejects missing domain evidence when isolation is required.
fn apply_loaded_baseline(
    findings: &mut [Finding],
    baseline_path: &Path,
    baseline: &BaselineFile,
    domains: Option<&BaselineDomains<'_>>,
) -> Result<BaselineOutcome> {
    let mut allowance = baseline.fingerprints.clone();
    let mut outcome = BaselineOutcome::default();
    for finding in findings {
        if !finding.is_active() {
            continue;
        }
        let fingerprint = baseline_key(finding, domains)?;
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
