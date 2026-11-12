//! Windows denies directory replacement while the verified ancestors are open without delete sharing.
use anyhow::{bail, Result};
use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, ErrorKind},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf, Prefix},
};

const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;
const REPARSE_POINT: u32 = 0x400;

/// Holds the absolute path, including the UNC share when present, and keeps its ancestors open.
pub(super) struct Dir {
    path: PathBuf,
    locks: Vec<File>,
}

/// Rejects component spellings that Win32 can interpret as streams, aliases, or device names.
fn check_name(name: &OsStr) -> Result<()> {
    if !crate::config::plain_name(name) || !crate::config::plain_chars(name) {
        bail!("unsupported Windows output filename");
    }
    Ok(())
}

/// Pins a real directory, denying write and delete sharing to prevent reparse or rename.
fn lock_directory(path: &Path) -> io::Result<File> {
    let dir = OpenOptions::new()
        .access_mode(0)
        .share_mode(1)
        .custom_flags(BACKUP_SEMANTICS | OPEN_REPARSE_POINT)
        .open(path)?;
    let meta = dir.metadata()?;
    if !meta.is_dir() || meta.file_attributes() & REPARSE_POINT != 0 {
        return Err(io::Error::other("output parent must be a real directory"));
    }
    Ok(dir)
}

/// Resolves the volume first, then verifies each directory before advancing or handling a parent component.
pub(super) fn open_parent(path: &Path) -> Result<Dir> {
    let start = path.ancestors().last().unwrap_or(Path::new(""));
    let anchor = std::path::absolute(if start.as_os_str().is_empty() {
        Path::new(".")
    } else {
        start
    })?;
    if !matches!(anchor.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::UNC(_,_) | Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(_,_)))
    {
        bail!("unsupported Windows output prefix");
    }
    let root = anchor.ancestors().last().unwrap_or(&anchor);
    let mut dir = Dir {
        path: root.to_owned(),
        locks: vec![lock_directory(root)?],
    };
    for component in anchor
        .strip_prefix(root)?
        .components()
        .chain(path.strip_prefix(start)?.components())
    {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                if dir.path.pop() {
                    dir.locks.pop();
                }
            }
            Component::Normal(name) => {
                check_name(name)?;
                dir.path.push(name);
                let opened = match lock_directory(&dir.path) {
                    Err(e) if e.kind() == ErrorKind::NotFound => {
                        if let Err(e) = fs::create_dir(&dir.path) {
                            if e.kind() != ErrorKind::AlreadyExists {
                                return Err(e.into());
                            }
                        }
                        lock_directory(&dir.path)?
                    }
                    result => result?,
                };
                dir.locks.push(opened);
            }
            _ => bail!("unsupported output directory prefix"),
        }
    }
    Ok(dir)
}

/// Opens the destination itself without traversing a reparse point or truncating existing bytes.
pub(super) fn existing(parent: &Dir, name: &OsStr, read: bool) -> Result<Option<File>> {
    check_name(name)?;
    match OpenOptions::new()
        .read(read)
        .write(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(parent.path.join(name))
    {
        Ok(file) => {
            let meta = file.metadata()?;
            if !meta.is_file() || meta.file_attributes() & REPARSE_POINT != 0 {
                bail!("output destination must be a regular file");
            }
            Ok(Some(file))
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        // Opening a directory for writing is denied; name the cause the Unix check reports.
        Err(e)
            if e.kind() == ErrorKind::PermissionDenied
                && fs::symlink_metadata(parent.path.join(name)).is_ok_and(|meta| meta.is_dir()) =>
        {
            bail!("output destination must be a regular file")
        }
        Err(e) => Err(e.into()),
    }
}

/// Creates an empty destination exclusively; an existing entry, a reparse point included, fails.
pub(super) fn create_new(parent: &Dir, name: &OsStr) -> Result<File> {
    check_name(name)?;
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(OPEN_REPARSE_POINT)
        .open(parent.path.join(name))?)
}

/// Deletes `name` only while it still names `file`, so a substituted entry is kept.
pub(super) fn remove_if_same(parent: &Dir, name: &OsStr, file: File) {
    cleanup_temporary(parent, name, &file);
}

/// A file prepared beside the destination under the pinned directory, not yet published.
pub(super) struct Stage {
    temp: tempfile::NamedTempFile,
    source: std::ffi::OsString,
}

/// Creates the hidden temporary file in the pinned directory.
pub(super) fn stage(dir: &Dir, _replacing: bool) -> Result<Stage> {
    // Cleanup is explicit so removal follows an identity check of the temporary entry.
    let temp = tempfile::Builder::new()
        .disable_cleanup(true)
        .tempfile_in(&dir.path)?;
    let source = temp
        .path()
        .file_name()
        .expect("named temporary file")
        .to_owned();
    Ok(Stage { temp, source })
}

impl Stage {
    /// The payload the caller fills before committing.
    pub(super) fn file(&mut self) -> &mut File {
        self.temp.as_file_mut()
    }

    /// Replaces the entry after `check` passes, validating the prepared file before and after.
    pub(super) fn commit(
        self,
        dir: &Dir,
        name: &OsStr,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        let prepared = names_file(dir, &self.source, self.temp.as_file()).and_then(|same| {
            if !same {
                bail!("temporary output entry changed before replacement");
            }
            check()
        });
        if let Err(error) = prepared {
            self.discard(dir);
            return Err(error);
        }
        let published = match self.temp.persist(dir.path.join(name)) {
            Ok(file) => file,
            Err(error) => {
                cleanup_temporary(dir, &self.source, error.file.as_file());
                return Err(error.error.into());
            }
        };
        if !names_file(dir, name, &published)? {
            bail!("published output no longer names the prepared file");
        }
        Ok(())
    }

    /// Removes the unpublished temporary file while it still names the prepared file.
    pub(super) fn discard(self, dir: &Dir) {
        cleanup_temporary(dir, &self.source, self.temp.as_file());
    }
}

/// Removes a failed temporary name only after checking it against the original open file.
fn cleanup_temporary(dir: &Dir, name: &OsStr, file: &File) {
    if names_file(dir, name, file).unwrap_or(false) {
        let _ = fs::remove_file(dir.path.join(name));
    }
}

/// Opens the named entry without following reparse points and compares it with the prepared file.
fn names_file(dir: &Dir, name: &OsStr, file: &File) -> Result<bool> {
    match existing(dir, name, false)? {
        Some(entry) => same_file(&entry, file),
        None => Ok(false),
    }
}

/// Compares the volume and file identities obtained from the two open handles.
pub(super) fn same_file(first: &File, second: &File) -> Result<bool> {
    Ok(same_file::Handle::from_file(first.try_clone()?)?
        == same_file::Handle::from_file(second.try_clone()?)?)
}
