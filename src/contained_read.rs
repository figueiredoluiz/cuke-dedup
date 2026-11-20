//! Reads of files found automatically under the analysis root, refused when they resolve outside.
//!
//! The file is opened first and checked through its handle: a regular file, at a canonical path
//! under the canonical root, and the same file as that path. No byte is read before the check
//! passes. The pathname checks assume no concurrent link changes under the root.

use crate::config::canonical_platform_path;
use crate::resource_limits::read_file_bytes;
use anyhow::{Context, Result};
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

/// Remedy for a standalone configuration candidate, which `--config` can replace.
pub(crate) const CONFIG_REMEDY: &str = "pass it with --config to use it explicitly";
/// Remedy for any other automatic file: `package.json`, framework configuration, ignore files.
pub(crate) const LINK_REMEDY: &str = "replace the link with a file inside the root";

/// Only a missing entry is not a candidate. File type and read errors belong to the reader;
/// this check must not silently skip present non-regular files or dangling links.
pub(crate) fn candidate_exists(path: &Path) -> bool {
    !path
        .symlink_metadata()
        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
}

/// Why a contained read was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    /// The path's canonical form is not under the root.
    Outside,
    /// The opened file is not a regular file.
    NotRegular,
    /// The opened file is not the file the canonical path names.
    Replaced,
}

/// A refused contained read; fatal, like an exceeded input limit.
#[derive(Debug)]
pub(crate) struct ContainmentRefused {
    pub(crate) reason: Reason,
    kind: &'static str,
    path: PathBuf,
    root: PathBuf,
    remedy: &'static str,
}

impl fmt::Display for ContainmentRefused {
    /// Names the file, the root and the remedy.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, path) = (self.kind, self.path.display());
        match self.reason {
            Reason::Outside => write!(
                formatter,
                "{kind} {path} resolves outside the analysis root {}; {}",
                self.root.display(),
                self.remedy
            ),
            Reason::NotRegular => write!(formatter, "{kind} {path} is not a regular file"),
            Reason::Replaced => write!(formatter, "{kind} {path} changed after it was opened"),
        }
    }
}

impl Error for ContainmentRefused {}

/// Reads files under one canonical root.
pub(crate) struct Contained<'a> {
    root: &'a Path,
    remedy: &'static str,
}

impl<'a> Contained<'a> {
    /// Reads under `root`, compared in its canonical, platform-normalized form.
    pub(crate) fn new(root: &'a Path, remedy: &'static str) -> Self {
        Self { root, remedy }
    }

    /// Whether `path` names a location under the root, compared lexically; reads still verify it.
    pub(crate) fn holds(&self, path: &Path) -> bool {
        path.starts_with(self.root)
    }

    /// Bounded bytes of `path` once it is verified contained.
    pub(crate) fn read_bytes(
        &self,
        path: &Path,
        kind: &'static str,
        limit: u64,
    ) -> Result<Vec<u8>> {
        let file = self.open(path, kind)?;
        read_file_bytes(file, path, kind, limit)
    }

    /// Bounded UTF-8 text of `path` once it is verified contained.
    pub(crate) fn read_utf8(&self, path: &Path, kind: &'static str, limit: u64) -> Result<String> {
        String::from_utf8(self.read_bytes(path, kind, limit)?)
            .with_context(|| format!("{kind} {} is not valid UTF-8", path.display()))
    }

    /// Opens `path` without blocking and verifies the handle.
    fn open(&self, path: &Path, kind: &'static str) -> Result<File> {
        let file = open_nonblocking(path)
            .with_context(|| format!("failed to read {kind} {}", path.display()))?;
        self.verify(path, kind, &file)?;
        Ok(file)
    }

    /// Checks that `file` is a regular file and is the file `path` names under the root.
    pub(crate) fn verify(&self, path: &Path, kind: &'static str, file: &File) -> Result<()> {
        let refuse = |reason| ContainmentRefused {
            reason,
            kind,
            path: path.to_owned(),
            root: self.root.to_owned(),
            remedy: self.remedy,
        };
        // Built eagerly: these failures need the file to vanish mid-check, which no test stages.
        let context = format!("failed to inspect {kind} {}", path.display());
        if !file.metadata().context(context.clone())?.is_file() {
            return Err(refuse(Reason::NotRegular).into());
        }
        let canonical = canonical_platform_path(path).context(context.clone())?;
        let root_context = format!("cannot access analysis root {}", self.root.display());
        let root = canonical_platform_path(self.root).context(root_context)?;
        if !canonical.starts_with(&root) {
            return Err(refuse(Reason::Outside).into());
        }
        if !same_file_as(file, &canonical).context(context)? {
            return Err(refuse(Reason::Replaced).into());
        }
        Ok(())
    }
}

/// Opens for reading; on Unix with `O_NONBLOCK`, so a FIFO put in place never blocks the open.
fn open_nonblocking(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NONBLOCK);
    // Open directory handles too, so regular-file verification refuses them on Windows.
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::custom_flags(&mut options, 0x0200_0000);
    options.open(path)
}

/// Whether `target` is the file already open as `file`, read from metadata: device and inode on
/// Unix, so no file is opened (a write-only target or a FIFO neither fails nor blocks).
#[cfg(unix)]
pub(crate) fn same_file_as(file: &File, target: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let (open, other) = (file.metadata()?, std::fs::metadata(target)?);
    Ok(open.dev() == other.dev() && open.ino() == other.ino())
}

/// Whether `target` is the file already open as `file`: volume and file index, through a handle
/// opened with no access rights.
#[cfg(not(unix))]
pub(crate) fn same_file_as(file: &File, target: &Path) -> io::Result<bool> {
    Ok(same_file::Handle::from_file(file.try_clone()?)? == same_file::Handle::from_path(target)?)
}

#[cfg(test)]
mod tests;
