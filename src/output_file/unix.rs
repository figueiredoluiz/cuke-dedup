//! POSIX directory-relative opens and atomic replacement.
use anyhow::{bail, Result};
use rustix::fs::{
    mkdirat, openat, renameat, statat, unlinkat, AtFlags, FileType, Mode, OFlags, Stat,
};
use rustix::io::Errno;
use std::{
    ffi::OsStr,
    fs::File,
    os::unix::fs::MetadataExt,
    path::{Component, Path},
};

pub(super) type Dir = File;

/// Opens an existing regular leaf without following it or blocking on a substituted FIFO.
pub(super) fn existing(parent: &Dir, name: &OsStr, read: bool) -> Result<Option<File>> {
    match statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Err(Errno::NOENT) => return Ok(None),
        Ok(entry) if FileType::from_raw_mode(entry.st_mode) != FileType::RegularFile => {
            bail!("output destination must be a regular file")
        }
        entry => {
            entry?;
        }
    }
    let access = if read { OFlags::RDWR } else { OFlags::WRONLY };
    let file = File::from(openat(
        parent,
        name,
        access | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    require_regular(&file)?;
    Ok(Some(file))
}

/// Separates the filesystem anchor from the components that must be opened without following links.
fn directory_start(path: &Path) -> Result<(&Path, &Path)> {
    // Darwin exposes these physical system directories through aliases at the volume root.
    #[cfg(target_os = "macos")]
    for (alias, physical) in [
        ("/var", "/private/var"),
        ("/tmp", "/private/tmp"),
        ("/etc", "/private/etc"),
    ] {
        if let Ok(rest) = path.strip_prefix(alias) {
            return Ok((Path::new(physical), rest));
        }
    }
    let start = path.ancestors().last().unwrap_or(Path::new(""));
    Ok((start, path.strip_prefix(start)?))
}

/// Opens each component without following it, using only the previously verified directory handle.
pub(super) fn open_parent(path: &Path) -> Result<Dir> {
    let (start, tail) = directory_start(path)?;
    // Search-only directory handles let operators publish below non-listable ancestors.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let access = OFlags::PATH;
    #[cfg(target_os = "macos")]
    let access = OFlags::from_bits_retain(libc::O_SEARCH as _);
    #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
    let access = OFlags::RDONLY;
    let flags = access | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let anchor = if start.as_os_str().is_empty() {
        Path::new(".")
    } else {
        start
    };
    let mut parent = File::from(rustix::fs::open(anchor, flags, Mode::empty())?);
    for component in tail.components() {
        // The anchor has already been stripped, so only relative components remain.
        let name = component.as_os_str();
        let fd = match openat(&parent, name, flags, Mode::empty()) {
            Err(Errno::NOENT) if matches!(component, Component::Normal(_)) => {
                match mkdirat(&parent, name, Mode::from_raw_mode(0o777)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(error) => return Err(error.into()),
                }
                openat(&parent, name, flags, Mode::empty())?
            }
            result => result?,
        };
        parent = File::from(fd);
    }
    Ok(parent)
}

/// Checks the entry itself against the open handle without following a substituted link.
fn names_file(dir: &Dir, name: &OsStr, file: &File) -> Result<bool> {
    let entry = match statat(dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(entry) => entry,
        Err(Errno::NOENT) => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let handle = rustix::fs::fstat(file)?;
    Ok(entry.st_dev == handle.st_dev && entry.st_ino == handle.st_ino)
}

/// Rejects a descriptor whose entry stopped being a regular file while it was opened.
pub(super) fn require_regular(file: &File) -> Result<()> {
    if !file.metadata()?.is_file() {
        bail!("output destination changed while opening it");
    }
    Ok(())
}

/// Checks the name against the still-open prepared file, including missing and substituted entries.
pub(super) fn require_entry(dir: &Dir, name: &OsStr, file: &File, message: &str) -> Result<()> {
    if !names_file(dir, name, file)? {
        bail!("{message}");
    }
    Ok(())
}

/// Opens a staging directory without following a link and checks its creation snapshot.
pub(super) fn open_stage(parent: &Dir, name: &OsStr, initial: &Stat) -> Result<File> {
    let stage = File::from(openat(
        parent,
        name,
        OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?);
    let opened = rustix::fs::fstat(&stage)?;
    if opened.st_uid != rustix::process::geteuid().as_raw()
        || (opened.st_dev, opened.st_ino, opened.st_mode & 0o777)
            != (initial.st_dev, initial.st_ino, initial.st_mode & 0o777)
    {
        bail!("output staging directory changed after creation");
    }
    Ok(stage)
}

/// Removes only the caller-owned staging directory that still matches the creation snapshot.
pub(super) fn cleanup_stage(parent: &Dir, name: &OsStr, initial: &Stat) {
    let owned = initial.st_uid == rustix::process::geteuid().as_raw()
        && FileType::from_raw_mode(initial.st_mode) == FileType::Directory;
    if let Ok(current) = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        if owned && (initial.st_dev, initial.st_ino) == (current.st_dev, current.st_ino) {
            let _ = unlinkat(parent, name, AtFlags::REMOVEDIR);
        }
    }
}

/// A file prepared in a private staging directory beside the destination, not yet published.
pub(super) struct Stage {
    name: std::ffi::OsString,
    initial: Stat,
    dir: File,
    file: File,
}

const PAYLOAD: &str = "payload";

/// Creates the staging directory and its empty payload under the verified parent.
pub(super) fn stage(parent: &Dir, replacing: bool) -> Result<Stage> {
    let name = std::ffi::OsString::from(format!(
        ".cuke-dedup-{:016x}{:016x}.tmp",
        getrandom::u64()?,
        getrandom::u64()?
    ));
    mkdirat(parent, &name, Mode::RWXU)?;
    let initial = statat(parent, &name, AtFlags::SYMLINK_NOFOLLOW)?;
    let opened = open_stage(parent, &name, &initial).and_then(|dir| {
        let mode = Mode::from_raw_mode(if replacing { 0o600 } else { 0o666 });
        let flags = OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC;
        let file = File::from(openat(&dir, PAYLOAD, flags, mode)?);
        Ok((dir, file))
    });
    match opened {
        Ok((dir, file)) => Ok(Stage {
            name,
            initial,
            dir,
            file,
        }),
        Err(error) => {
            cleanup_stage(parent, &name, &initial);
            Err(error)
        }
    }
}

impl Stage {
    /// The payload the caller fills before committing.
    pub(super) fn file(&mut self) -> &mut File {
        &mut self.file
    }

    /// Renames the payload over `name` after `check` passes, validating the entry before and after.
    pub(super) fn commit(
        self,
        parent: &Dir,
        name: &OsStr,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let payload = OsStr::new(PAYLOAD);
        let result = require_entry(
            &self.dir,
            payload,
            &self.file,
            "temporary output entry changed before replacement",
        )
        .and_then(|()| check())
        .and_then(|()| Ok(renameat(&self.dir, payload, parent, name)?))
        .and_then(|()| {
            require_entry(
                parent,
                name,
                &self.file,
                "published output no longer names the prepared file",
            )
        });
        self.discard(parent);
        result
    }

    /// Removes an unpublished payload and the staging directory, each only while it is still ours.
    pub(super) fn discard(self, parent: &Dir) {
        let payload = OsStr::new(PAYLOAD);
        if names_file(&self.dir, payload, &self.file).unwrap_or(false) {
            let _ = unlinkat(&self.dir, payload, AtFlags::empty());
        }
        cleanup_stage(parent, &self.name, &self.initial);
    }
}

/// Creates an empty leaf exclusively; an existing entry, a link included, fails instead.
pub(super) fn create_new(parent: &Dir, name: &OsStr) -> Result<File> {
    let flags = OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    Ok(File::from(openat(
        parent,
        name,
        flags,
        Mode::from_raw_mode(0o666),
    )?))
}

/// Unlinks `name` only while it still names `file`, so a substituted entry is kept.
pub(super) fn remove_if_same(parent: &Dir, name: &OsStr, file: File) {
    if names_file(parent, name, &file).unwrap_or(false) {
        let _ = unlinkat(parent, name, AtFlags::empty());
    }
}

/// Compares two open regular files without resolving either pathname again.
pub(super) fn same_file(first: &File, second: &File) -> Result<bool> {
    let (a, b) = (first.metadata()?, second.metadata()?);
    Ok((a.dev(), a.ino()) == (b.dev(), b.ino()))
}
