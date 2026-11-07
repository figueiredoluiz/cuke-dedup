//! Directory-relative publication of regular report and baseline files.
#[cfg(unix)]
#[path = "output_file/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "output_file/windows.rs"]
mod platform;

use anyhow::{bail, Context, Result};
use std::{ffi::OsStr, fs::File, io::Write, path::Path};

/// A destination bound to its parent directory and, when present, its original regular file.
pub(crate) struct OutputFile<'a> {
    parent: platform::Dir,
    name: &'a OsStr,
    previous: Option<File>,
}

impl<'a> OutputFile<'a> {
    /// Binds a writable destination. Baseline updates also request read access to its original file.
    pub(crate) fn open(path: &'a Path, read: bool) -> Result<Self> {
        let name = path
            .file_name()
            .filter(|name| {
                path.as_os_str()
                    .as_encoded_bytes()
                    .ends_with(name.as_encoded_bytes())
            })
            .context("expected an output filename")?;
        let parent = platform::open_parent(path.parent().context("expected an output parent")?)
            .with_context(|| format!("cannot open output directory for {}", path.display()))?;
        let previous = platform::existing(&parent, name, read)
            .with_context(|| format!("cannot open output file {}", path.display()))?;
        Ok(Self {
            parent,
            name,
            previous,
        })
    }

    /// Borrows the verified original file for baseline parsing without reopening its path.
    pub(crate) fn previous(&self) -> Option<&File> {
        self.previous.as_ref()
    }

    /// Publishes complete bytes by replacing the directory entry, preserving other hard-link names.
    pub(crate) fn replace(self, bytes: &[u8]) -> Result<()> {
        platform::publish(&self.parent, self.name, self.previous.is_some(), |temp| {
            if let Some(previous) = &self.previous {
                let permissions = previous.metadata()?.permissions();
                #[cfg(unix)]
                let permissions = {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::Permissions::from_mode(permissions.mode() & 0o777)
                };
                temp.set_permissions(permissions)?;
            }
            temp.write_all(bytes)?;
            let current = platform::existing(&self.parent, self.name, false)?;
            match (self.previous, current) {
                (None, None) => Ok(()),
                (Some(ref old), Some(ref new)) if platform::same_file(old, new)? => Ok(()),
                _ => bail!("output destination changed before replacement"),
            }
        })
    }
}

#[cfg(test)]
mod tests;
