//! Directory-relative publication of regular report and baseline files.
#[cfg(unix)]
#[path = "output_file/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "output_file/windows.rs"]
mod platform;

use anyhow::{bail, Context, Result};
use std::{ffi::OsString, fs::File, io::Write, path::Path};

/// A destination bound to its parent directory and, when present, its original regular file.
pub(crate) struct OutputFile {
    parent: platform::Dir,
    name: OsString,
    previous: Option<File>,
    /// Whether `previous` is the empty file [`reserve`](Self::reserve) created.
    reserved: bool,
}

impl OutputFile {
    /// Binds a writable destination. Baseline updates also request read access to its original file.
    pub(crate) fn open(path: &Path, read: bool) -> Result<Self> {
        let name = path
            .file_name()
            .filter(|name| {
                path.as_os_str()
                    .as_encoded_bytes()
                    .ends_with(name.as_encoded_bytes())
            })
            .context("expected an output filename")?
            .to_owned();
        let parent = platform::open_parent(path.parent().context("expected an output parent")?)
            .with_context(|| format!("cannot open output directory for {}", path.display()))?;
        let previous = platform::existing(&parent, &name, read)
            .with_context(|| format!("cannot open output file {}", path.display()))?;
        Ok(Self {
            parent,
            name,
            previous,
            reserved: false,
        })
    }

    /// Binds `path` like [`open`](Self::open) and creates an absent destination empty and
    /// exclusively, so other paths can be compared with it by file identity before it is written.
    pub(crate) fn reserve(path: &Path) -> Result<Self> {
        let mut output = Self::open(path, false)?;
        if output.previous.is_none() {
            output.previous = Some(
                platform::create_new(&output.parent, &output.name)
                    .with_context(|| format!("cannot create output file {}", path.display()))?,
            );
            output.reserved = true;
        }
        Ok(output)
    }

    /// Borrows the verified original file for baseline parsing without reopening its path.
    pub(crate) fn previous(&self) -> Option<&File> {
        self.previous.as_ref()
    }

    /// Publishes complete bytes by replacing the directory entry, preserving other hard-link names.
    pub(crate) fn replace(self, bytes: &[u8]) -> Result<()> {
        self.stage(|out| Ok(out.write_all(bytes)?))?.commit()
    }

    /// Streams the bytes `fill` writes into a hidden file beside the destination, which
    /// [`Staged::commit`] publishes; dropping the result publishes nothing.
    pub(crate) fn stage(self, fill: impl FnOnce(&mut dyn Write) -> Result<()>) -> Result<Staged> {
        let stage = platform::stage(&self.parent, self.previous.is_some())?;
        let mut staged = Staged {
            output: self,
            stage: Some(stage),
        };
        let file = staged
            .stage
            .as_mut()
            .expect("a new stage holds its file")
            .file();
        if let Some(previous) = &staged.output.previous {
            let permissions = previous.metadata()?.permissions();
            #[cfg(unix)]
            let permissions = {
                use std::os::unix::fs::PermissionsExt;
                std::fs::Permissions::from_mode(permissions.mode() & 0o777)
            };
            file.set_permissions(permissions)?;
        }
        let mut out = std::io::BufWriter::new(file);
        fill(&mut out)?;
        out.flush()?;
        drop(out);
        Ok(staged)
    }
}

impl Drop for OutputFile {
    /// Removes the empty file [`reserve`](Self::reserve) created when nothing was published over it,
    /// only while its name still points to that file.
    fn drop(&mut self) {
        if !self.reserved {
            return;
        }
        self.previous
            .take()
            .into_iter()
            .for_each(|file| platform::remove_if_same(&self.parent, &self.name, file));
    }
}

/// Bytes staged beside a bound destination. Dropping it without [`commit`](Self::commit) removes
/// the staged file and, through the destination, an empty file [`OutputFile::reserve`] created.
pub(crate) struct Staged {
    output: OutputFile,
    stage: Option<platform::Stage>,
}

impl Staged {
    /// Checks the bound destination before replacement; concurrent directory writers are not serialized.
    pub(crate) fn commit(mut self) -> Result<()> {
        let stage = self
            .stage
            .take()
            .expect("an uncommitted stage holds its file");
        // Windows refuses to replace an open file, so there the handle closes once the check
        // passes; a failed check keeps it to identify the reservation for cleanup.
        #[cfg(windows)]
        let mut held = self.output.previous.take();
        #[cfg(not(windows))]
        let mut held: Option<File> = None;
        let output = &self.output;
        let result = stage.commit(&output.parent, &output.name, || {
            let current = platform::existing(&output.parent, &output.name, false)?;
            match (output.previous.as_ref().or(held.as_ref()), current) {
                (None, None) => {}
                (Some(old), Some(ref new)) if platform::same_file(old, new)? => {}
                _ => bail!("output destination changed before replacement"),
            }
            drop(held.take());
            Ok(())
        });
        // A handle closed before a failed rename cannot identify the reservation, which is kept.
        #[cfg(windows)]
        {
            self.output.reserved &= result.is_ok() || held.is_some();
            self.output.previous = held;
        }
        result?;
        self.output.reserved = false;
        Ok(())
    }
}

impl Drop for Staged {
    /// Discards an unpublished stage; the destination then drops its own reservation.
    fn drop(&mut self) {
        if let Some(stage) = self.stage.take() {
            stage.discard(&self.output.parent);
        }
    }
}

#[cfg(test)]
mod tests;
