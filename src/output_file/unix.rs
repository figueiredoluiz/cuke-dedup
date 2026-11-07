//! POSIX directory-relative opens and atomic replacement.
use anyhow::{bail, Result};
use rustix::fs::{mkdirat, openat, renameat, statat, unlinkat, AtFlags, FileType, Mode, OFlags};
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
    if !file.metadata()?.is_file() {
        bail!("output destination changed while opening it");
    }
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

/// Binds the staging directory and validates the prepared file before and after atomic publication.
pub(super) fn publish(
    parent: &Dir,
    name: &OsStr,
    replacing: bool,
    prepare: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    let temp = format!(
        ".cuke-dedup-{:016x}{:016x}.tmp",
        getrandom::u64()?,
        getrandom::u64()?
    );
    let stage_name = OsStr::new(&temp);
    mkdirat(parent, stage_name, Mode::RWXU)?;
    let initial = statat(parent, stage_name, AtFlags::SYMLINK_NOFOLLOW)?;
    let owned = initial.st_uid == rustix::process::geteuid().as_raw()
        && FileType::from_raw_mode(initial.st_mode) == FileType::Directory;
    let result = (|| {
        let stage = File::from(openat(
            parent,
            stage_name,
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
        let payload = OsStr::new("payload");
        let mode = Mode::from_raw_mode(if replacing { 0o600 } else { 0o666 });
        let flags = OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC;
        let mut file = File::from(openat(&stage, payload, flags, mode)?);
        let result = prepare(&mut file).and_then(|()| {
            if !names_file(&stage, payload, &file)? {
                bail!("temporary output entry changed before replacement");
            }
            renameat(&stage, payload, parent, name)?;
            if !names_file(parent, name, &file)? {
                bail!("published output no longer names the prepared file");
            }
            Ok(())
        });
        if result.is_err() && names_file(&stage, payload, &file).unwrap_or(false) {
            let _ = unlinkat(&stage, payload, AtFlags::empty());
        }
        result
    })();
    if let Ok(current) = statat(parent, stage_name, AtFlags::SYMLINK_NOFOLLOW) {
        if owned && (initial.st_dev, initial.st_ino) == (current.st_dev, current.st_ino) {
            let _ = unlinkat(parent, stage_name, AtFlags::REMOVEDIR);
        }
    }
    result
}

/// Compares two open regular files without resolving either pathname again.
pub(super) fn same_file(first: &File, second: &File) -> Result<bool> {
    let (a, b) = (first.metadata()?, second.metadata()?);
    Ok((a.dev(), a.ino()) == (b.dev(), b.ino()))
}
