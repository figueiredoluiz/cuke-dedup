//! Non-regular automatic-file fixtures shared by configuration and ignore-file tests.

use std::fs;
use std::path::Path;

/// A directory everywhere, or an unopenable, FIFO or linked non-regular entry on Unix.
#[derive(Clone, Copy, Debug)]
pub(crate) enum NonRegular {
    Directory,
    #[cfg(unix)]
    Fifo,
    #[cfg(unix)]
    LinkedFifo,
    #[cfg(unix)]
    Socket,
    #[cfg(unix)]
    LinkedSocket,
    #[cfg(unix)]
    DanglingLink,
    #[cfg(unix)]
    CycleLink,
    #[cfg(unix)]
    LinkedCycle,
}

impl NonRegular {
    /// Every form supported by the host platform.
    pub(crate) const FORMS: &[Self] = &[
        Self::Directory,
        #[cfg(unix)]
        Self::Fifo,
        #[cfg(unix)]
        Self::LinkedFifo,
        #[cfg(unix)]
        Self::Socket,
        #[cfg(unix)]
        Self::LinkedSocket,
        #[cfg(unix)]
        Self::DanglingLink,
        #[cfg(unix)]
        Self::CycleLink,
        #[cfg(unix)]
        Self::LinkedCycle,
    ];

    /// Creates the entry, placing a linked target separately under `root`.
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
            #[cfg(unix)]
            Self::Socket | Self::LinkedSocket => {
                let target = if matches!(self, Self::LinkedSocket) {
                    root.join("socket-source")
                } else {
                    path.to_owned()
                };
                let _listener = std::os::unix::net::UnixListener::bind(&target).unwrap();
                if matches!(self, Self::LinkedSocket) {
                    std::os::unix::fs::symlink(target, path).unwrap();
                }
            }
            #[cfg(unix)]
            Self::DanglingLink => {
                std::os::unix::fs::symlink(root.join("absent-source"), path).unwrap();
            }
            #[cfg(unix)]
            Self::CycleLink | Self::LinkedCycle => {
                let target = if matches!(self, Self::LinkedCycle) {
                    let target = root.join("cycle-source");
                    std::os::unix::fs::symlink(path, &target).unwrap();
                    target
                } else {
                    path.to_owned()
                };
                std::os::unix::fs::symlink(target, path).unwrap();
            }
        }
        let _ = root; // Used only by FIFO forms on Unix.
    }

    /// Replaces the entry itself with a regular file, leaving any linked target unread.
    pub(crate) fn replace(self, path: &Path, payload: &str) {
        if matches!(self, Self::Directory) {
            fs::remove_dir(path).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
        fs::write(path, payload).unwrap();
    }
}
