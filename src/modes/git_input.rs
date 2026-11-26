//! Opens repository-configured Git input files through the selected checkout.

use anyhow::{bail, Result};
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

const OUTSIDE: &str = "selects a path outside the checkout";
const NOT_REGULAR: &str = "must select a regular file of at most 1 MiB";
const MAX_LINKS: usize = 40;

enum Entry {
    Directory(File),
    File(File),
    Link(PathBuf),
}

/// Opens an existing Git input by walking verified parent handles under the checkout.
pub(super) fn open_repository_input(
    repository: &Path,
    alias: Option<&Path>,
    source: &Path,
) -> Result<Option<File>> {
    let relative = checkout_relative(repository, alias, source)?;
    let mut remaining = VecDeque::new();
    prepend_components(&relative, &mut remaining)?;
    let root = platform::open_root(repository)?;
    let mut parents = vec![root];
    let mut links = 0;
    while let Some(name) = remaining.pop_front() {
        if name == "." {
            continue;
        }
        if name == ".." {
            if parents.len() == 1 {
                bail!(OUTSIDE);
            }
            parents.pop();
            continue;
        }
        let parent = parents.last().expect("the checkout handle stays open");
        let entry = match platform::open_git_component(parent, &name, remaining.is_empty()) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            result => result?,
        };
        match entry {
            Entry::Directory(directory) => parents.push(directory),
            Entry::File(file) => return Ok(Some(file)),
            Entry::Link(target) => {
                links += 1;
                if links > MAX_LINKS {
                    bail!("too many links in the configured Git input path");
                }
                let target = if target.is_absolute() {
                    parents.truncate(1);
                    checkout_relative(repository, alias, &target)?
                } else {
                    target
                };
                prepend_components(&target, &mut remaining)?;
            }
        }
    }
    bail!(NOT_REGULAR)
}

/// Admits an absolute configured path without resolving an external prefix on disk.
pub(super) fn checkout_relative(
    repository: &Path,
    alias: Option<&Path>,
    source: &Path,
) -> Result<PathBuf> {
    let source = crate::config::normalize_platform_path(source.to_path_buf());
    if let Some(relative) = std::iter::once(repository)
        .chain(alias)
        .find_map(|prefix| source.strip_prefix(prefix).ok())
    {
        return Ok(relative.to_path_buf());
    }
    // These system aliases are also recognized by the POSIX output path reader.
    #[cfg(target_os = "macos")]
    for (alias, physical) in [
        ("/var", "/private/var"),
        ("/tmp", "/private/tmp"),
        ("/etc", "/private/etc"),
    ] {
        if let Ok(rest) = source.strip_prefix(alias) {
            let expanded = Path::new(physical).join(rest);
            if let Ok(relative) = expanded.strip_prefix(repository) {
                return Ok(relative.to_path_buf());
            }
        }
    }
    bail!(OUTSIDE)
}

/// Verifies an alternate checkout spelling derived solely from the operator's analysis path.
#[cfg(windows)]
pub(super) fn trusted_checkout_alias(
    requested_root: &Path,
    canonical_root: &Path,
    repository: &Path,
) -> Option<PathBuf> {
    let depth = canonical_root
        .strip_prefix(repository)
        .ok()?
        .components()
        .count();
    let requested = std::path::absolute(requested_root).ok()?;
    let alias =
        crate::config::normalize_platform_path(requested.ancestors().nth(depth)?.to_path_buf());
    (alias != repository
        && crate::config::canonical_platform_path(&alias)
            .is_ok_and(|resolved| resolved == repository))
    .then_some(alias)
}

/// Queues relative names and preserves an explicit final directory requirement.
fn prepend_components(path: &Path, remaining: &mut VecDeque<OsString>) -> Result<()> {
    #[cfg(windows)]
    if path.has_root() || matches!(path.components().next(), Some(Component::Prefix(_))) {
        bail!(OUTSIDE);
    }
    if path.file_name().is_some_and(|name| {
        !path
            .as_os_str()
            .as_encoded_bytes()
            .ends_with(name.as_encoded_bytes())
    }) {
        remaining.push_front(OsString::from("."));
    }
    for component in path.components().rev() {
        match component {
            Component::Normal(name) => remaining.push_front(name.to_owned()),
            Component::ParentDir => remaining.push_front(OsString::from("..")),
            _ => (), // The caller supplied a checkout-relative path or a relative link target.
        }
    }
    Ok(())
}

#[cfg(unix)]
mod platform {
    use super::*;
    use rustix::fs::{open, openat, readlinkat, statat, AtFlags, FileType, Mode, OFlags};
    use std::os::unix::ffi::OsStringExt;

    /// Requests a searchable directory handle without following a replacement link.
    fn directory_flags() -> OFlags {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let access = OFlags::PATH;
        #[cfg(target_os = "macos")]
        let access = OFlags::from_bits_retain(libc::O_SEARCH as _);
        #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
        let access = OFlags::RDONLY;
        access | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
    }

    /// Opens the canonical checkout directory selected by the operator.
    pub(super) fn open_root(repository: &Path) -> io::Result<File> {
        Ok(File::from(open(
            repository,
            directory_flags(),
            Mode::empty(),
        )?))
    }

    /// Reads only link data or opens a child of the current directory handle.
    pub(super) fn open_git_component(parent: &File, name: &OsStr, leaf: bool) -> io::Result<Entry> {
        let metadata = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let kind = FileType::from_raw_mode(metadata.st_mode);
        if kind == FileType::Symlink {
            let target = readlinkat(parent, name, Vec::new())?;
            return Ok(Entry::Link(OsString::from_vec(target.into_bytes()).into()));
        }
        if !leaf {
            let directory = openat(parent, name, directory_flags(), Mode::empty())?;
            return Ok(Entry::Directory(File::from(directory)));
        }
        if kind != FileType::RegularFile {
            return Err(io::Error::new(io::ErrorKind::InvalidData, NOT_REGULAR));
        }
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
        Ok(Entry::File(File::from(openat(
            parent,
            name,
            flags,
            Mode::empty(),
        )?)))
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use cap_primitives::fs as handle;
    use std::os::windows::fs::MetadataExt;

    const REPARSE_POINT: u32 = 0x400;

    /// Opens the canonical checkout directory selected by the operator.
    pub(super) fn open_root(repository: &Path) -> io::Result<File> {
        handle::open_ambient_dir(repository, cap_primitives::ambient_authority())
    }

    /// Inspects one Windows child using handle-relative opens and reads any link by handle.
    pub(super) fn open_git_component(parent: &File, name: &OsStr, leaf: bool) -> io::Result<Entry> {
        if !crate::config::plain_name(name) || !crate::config::plain_chars(name) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported Windows path component",
            ));
        }
        let name = Path::new(name);
        let entry = handle::stat(parent, name, handle::FollowSymlinks::No)?;
        if handle::MetadataExt::file_attributes(&entry) & REPARSE_POINT != 0 {
            return Ok(Entry::Link(handle::read_link_contents(parent, name)?));
        }
        if leaf && !entry.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, NOT_REGULAR));
        }
        let file = if leaf {
            handle::open(parent, name, handle::OpenOptions::new().read(true))?
        } else {
            handle::open_dir_nofollow(parent, name)?
        };
        if file.metadata()?.file_attributes() & REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Git input path changed during lookup",
            ));
        }
        Ok(if leaf {
            Entry::File(file)
        } else {
            Entry::Directory(file)
        })
    }
}
