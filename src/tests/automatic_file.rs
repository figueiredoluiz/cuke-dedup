//! Non-regular automatic-file fixtures shared by configuration and ignore-file tests.

use std::fs;
use std::path::Path;

/// A directory everywhere, or a direct/linked FIFO on Unix.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NonRegular {
    Directory,
    #[cfg(unix)]
    Fifo,
    #[cfg(unix)]
    LinkedFifo,
}

impl NonRegular {
    /// Every form supported by the host platform.
    pub(crate) const FORMS: &[Self] = &[
        Self::Directory,
        #[cfg(unix)]
        Self::Fifo,
        #[cfg(unix)]
        Self::LinkedFifo,
    ];

    /// Creates the entry, placing a linked FIFO's target separately under `root`.
    pub(crate) fn create(self, root: &Path, path: &Path) {
        match self {
            Self::Directory => fs::create_dir(path).unwrap(),
            #[cfg(unix)]
            Self::Fifo | Self::LinkedFifo => {
                let target = if matches!(self, Self::LinkedFifo) {
                    root.join("pipe-source")
                } else {
                    path.to_owned()
                };
                assert!(std::process::Command::new("mkfifo")
                    .arg(&target)
                    .status()
                    .unwrap()
                    .success());
                if matches!(self, Self::LinkedFifo) {
                    std::os::unix::fs::symlink(target, path).unwrap();
                }
            }
        }
        let _ = root; // Used only by FIFO forms on Unix.
    }

    /// Replaces the entry itself with a regular file, leaving any FIFO target unread.
    pub(crate) fn replace(self, path: &Path, payload: &str) {
        if matches!(self, Self::Directory) {
            fs::remove_dir(path).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
        fs::write(path, payload).unwrap();
    }
}
