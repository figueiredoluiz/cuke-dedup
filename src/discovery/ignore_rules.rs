//! Ignore files loaded by discovery instead of the `ignore` crate's walker, so a file located
//! under the root is read only through [`Contained`]. Matching follows `ignore` 0.4.34 as
//! discovery configured it (standard filters, `.cuke-dedupignore`, `require_git(false)`):
//!
//! - per directory level, `.cuke-dedupignore` > `.ignore` > `.gitignore` > git exclude, each class
//!   decided by the deepest level with a match, then the global excludes file;
//! - the root's ancestors are levels too; their ignore files and the global file are read as
//!   before, while git metadata is classified by its own location;
//! - an ignore match prunes the entry, a whitelist keeps it even when hidden, no match leaves it to
//!   the hidden filter;
//! - load errors under the root are dropped (the crate attaches them to an entry nobody reads); an
//!   ancestor error is one walk error, unless it is a file's single I/O error.

use crate::config::canonical_platform_path;
use crate::contained_read::{candidate_exists, Contained, LINK_REMEDY};
use crate::resource_limits::{is_fatal_input_error, read_bytes, MAX_CONFIG_INPUT_BYTES};
use anyhow::Result;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::{DirEntry, Match};
use std::io::BufRead;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Per-directory ignore files, in precedence order.
const IGNORE_FILES: [&str; 3] = [".cuke-dedupignore", ".ignore", ".gitignore"];

/// The matchers one directory contributes: the three ignore files, then git exclude.
#[derive(Default)]
struct Level([Option<Gitignore>; 4]);

/// Errors while loading files: whether any would surface as a walk error.
#[derive(Default)]
struct Surfaced(bool);

/// Whether a file under the root is read contained, or a file outside it as before.
#[derive(Clone, Copy)]
enum Located<'a> {
    Inside(&'a Contained<'a>),
    Outside(&'a Contained<'a>),
}

impl Located<'_> {
    /// Keeps a metadata entry contained when either its lexical or resolved parent is in-root.
    /// An unresolved parent cannot establish an external location.
    fn of(self, path: &Path) -> Self {
        let (Located::Inside(contained) | Located::Outside(contained)) = self;
        let parent = path.parent().and_then(|parent| {
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            canonical_platform_path(parent).ok()
        });
        if contained.holds(&lexical_absolute(path))
            || parent
                .as_deref()
                .is_none_or(|parent| contained.holds(parent))
        {
            Located::Inside(contained)
        } else {
            Located::Outside(contained)
        }
    }

    /// Bounded bytes of an optional file; inside-root candidates must pass regular-file checks.
    /// Missing or unreadable files return `None`; a refusal or input-limit error is returned.
    fn read(self, path: &Path) -> Result<Option<Vec<u8>>> {
        self.read_recording(path, None)
    }

    /// Reads optional bytes, recording ordinary I/O errors only for Git pointer metadata.
    fn read_recording(
        self,
        path: &Path,
        surfaced: Option<&mut Surfaced>,
    ) -> Result<Option<Vec<u8>>> {
        let read = match self {
            Located::Inside(contained) => {
                if !candidate_exists(path) {
                    return Ok(None);
                }
                contained.read_bytes(path, "ignore file", MAX_CONFIG_INPUT_BYTES)
            }
            Located::Outside(_) => {
                // pattern-ok: outside-root sources preserve the existing optional-file behavior.
                if !path.is_file() {
                    return Ok(None);
                }
                read_bytes(path, "ignore file", MAX_CONFIG_INPUT_BYTES)
            }
        };
        match read {
            Ok(bytes) => Ok(Some(bytes)),
            // The crate drops I/O errors on ignore files; a refusal or the input limit is fatal.
            Err(error) if !is_fatal_input_error(&error) => {
                if let Some(surfaced) = surfaced {
                    surfaced.0 = true;
                }
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

/// `path` made absolute against the working directory, where `ignore` reads a relative one, with
/// `..` folded by name; a link it names is still resolved and verified when read.
fn lexical_absolute(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let mut folded = PathBuf::new();
    for component in absolute.components() {
        // `components` already drops `.` after the absolute prefix.
        match component {
            Component::ParentDir => {
                folded.pop();
            }
            other => folded.push(other),
        }
    }
    folded
}

/// Builds a matcher rooted at `dir` from `bytes` read from `path`, reading lines like
/// `BufRead::lines`: a first-line BOM trimmed, stopping at the first non-UTF-8 line, skipping bad
/// globs. Records whether the file's errors would surface: any glob error, or more than one error.
fn matcher(dir: &Path, path: &Path, bytes: &[u8], surfaced: &mut Surfaced) -> Gitignore {
    let mut builder = GitignoreBuilder::new(dir);
    let (mut glob_errors, mut io_errors) = (0usize, 0usize);
    let mut rest = bytes;
    let mut first = true;
    while !rest.is_empty() {
        let (line, next) = match rest.iter().position(|&byte| byte == b'\n') {
            Some(end) => {
                let line = &rest[..end];
                (line.strip_suffix(b"\r").unwrap_or(line), &rest[end + 1..])
            }
            None => (rest, &rest[rest.len()..]),
        };
        rest = next;
        let Ok(line) = std::str::from_utf8(line) else {
            io_errors += 1;
            break;
        };
        let line = if first {
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        first = false;
        if builder.add_line(Some(path.to_owned()), line).is_err() {
            glob_errors += 1;
        }
    }
    // Every line was compiled by `add_line`, so building does not fail in practice.
    let built = builder.build();
    glob_errors += usize::from(built.is_err());
    let matcher = built.unwrap_or_else(|_| Gitignore::empty());
    surfaced.0 |= glob_errors > 0 || io_errors > 1;
    matcher
}

/// Uses the pinned global-file resolver and cwd-relative matching, with the same byte bound as
/// other ignore files. Missing files and ordinary matching errors remain optional.
fn global_matcher(contained: &Contained<'_>) -> Result<Gitignore> {
    std::env::current_dir()
        .ok()
        .zip(ignore::gitignore::gitconfig_excludes_path())
        .map(|(cwd, path)| {
            Located::Outside(contained).read(&path).map(|bytes| {
                bytes.map_or_else(Gitignore::empty, |bytes| {
                    matcher(&cwd, &path, &bytes, &mut Surfaced::default())
                })
            })
        })
        .transpose()
        .map(|matcher| matcher.unwrap_or_else(Gitignore::empty))
}

/// Loads the matchers `dir` contributes.
fn load_level(dir: &Path, located: Located<'_>, surfaced: &mut Surfaced) -> Result<Level> {
    let mut level = Level::default();
    for (slot, name) in IGNORE_FILES.iter().enumerate() {
        let path = dir.join(name);
        if let Some(bytes) = located.read(&path)? {
            level.0[slot] = Some(matcher(dir, &path, &bytes, surfaced));
        }
    }
    if let Some(exclude) = exclude_path(dir, located, surfaced)? {
        if let Some(bytes) = located.of(&exclude).read(&exclude)? {
            level.0[3] = Some(matcher(dir, &exclude, &bytes, surfaced));
        }
    }
    Ok(level)
}

/// `info/exclude` of the git directory at `dir`, following a `.git` file and its `commondir` as
/// `ignore` does; `None` without a `.git` entry or when the file does not name a git directory.
fn exclude_path(
    dir: &Path,
    located: Located<'_>,
    surfaced: &mut Surfaced,
) -> Result<Option<PathBuf>> {
    let dot_git = dir.join(".git");
    if dot_git.metadata().is_ok_and(|metadata| metadata.is_dir()) {
        return Ok(Some(dot_git.join("info/exclude")));
    }
    // Use the same line reader as `ignore`: absent, empty and invalid lines are distinct.
    let first_line = |bytes: Vec<u8>, surfaced: &mut Surfaced| match bytes.as_slice().lines().next()
    {
        Some(Ok(line)) => Some(line),
        Some(Err(_)) => {
            surfaced.0 = true;
            None
        }
        None => None,
    };
    let Some(pointer) = located
        .read_recording(&dot_git, Some(&mut *surfaced))?
        .and_then(|bytes| first_line(bytes, surfaced))
    else {
        return Ok(None);
    };
    let Some(git_dir) = pointer.strip_prefix("gitdir: ").map(PathBuf::from) else {
        return Ok(None);
    };
    let commondir = git_dir.join("commondir");
    let Some(common) = located
        .of(&commondir)
        .read(&commondir)?
        .and_then(|bytes| first_line(bytes, surfaced))
    else {
        return Ok(None);
    };
    let common = if common.starts_with('.') {
        git_dir.join(common)
    } else {
        PathBuf::from(common)
    };
    Ok(Some(common.join("info/exclude")))
}

/// Ignore matching for one walk under a canonical root.
pub(super) struct IgnoreRules {
    root: PathBuf,
    hidden: bool,
    /// The root's ancestors, nearest first.
    ancestors: Vec<Level>,
    global: Gitignore,
    /// Levels from the root down the walk's current path.
    stack: Mutex<Vec<(PathBuf, Arc<Level>)>>,
    refusal: Mutex<Option<anyhow::Error>>,
}

impl IgnoreRules {
    /// Loads the global file, the ancestors' files and the root's own; `hidden` enables the hidden
    /// filter. The flag reports an ancestor error the crate's walker would yield.
    pub(super) fn new(root: &Path, hidden: bool) -> Result<(Self, bool)> {
        let mut surfaced = Surfaced::default();
        let contained = Contained::new(root, LINK_REMEDY);
        let ancestors = root
            .ancestors()
            .skip(1)
            .map(|dir| load_level(dir, Located::Outside(&contained), &mut surfaced))
            .collect::<Result<_>>()?;
        let ancestor_error = surfaced.0;
        let rules = Self {
            root: root.to_owned(),
            hidden,
            ancestors,
            global: global_matcher(&contained)?,
            stack: Mutex::default(),
            refusal: Mutex::default(),
        };
        rules.path_levels(root)?;
        Ok((rules, ancestor_error))
    }

    /// The levels from the root down to `dir`, root first, loaded contained. Only the walk's
    /// current path is retained: a depth-first walk does not return to a directory it has left,
    /// so levels off the path are dropped, and a revisit reloads them.
    fn path_levels(&self, dir: &Path) -> Result<Vec<Arc<Level>>> {
        let relative = dir.strip_prefix(&self.root).unwrap_or(Path::new(""));
        let dirs = std::iter::once(self.root.clone())
            .chain(relative.components().scan(self.root.clone(), |dir, part| {
                dir.push(part);
                Some(dir.clone())
            }))
            .collect::<Vec<_>>();
        let mut stack = self.stack.lock().expect("ignore levels lock");
        let kept = stack
            .iter()
            .zip(&dirs)
            .take_while(|((held, _), dir)| held == *dir)
            .count();
        stack.truncate(kept);
        let contained = Contained::new(&self.root, LINK_REMEDY);
        for dir in &dirs[kept..] {
            let level = load_level(dir, Located::Inside(&contained), &mut Surfaced::default())?;
            stack.push((dir.clone(), Arc::new(level)));
        }
        Ok(stack.iter().map(|(_, level)| Arc::clone(level)).collect())
    }

    /// Whether the walk keeps `entry` (below the root). A refused ignore file is recorded and the
    /// entry kept; the walk loop stops at it, or after the walk, with [`IgnoreRules::check`].
    pub(super) fn admits(&self, entry: &DirEntry) -> bool {
        let path = entry.path();
        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
        let chain = match self.path_levels(path.parent().unwrap_or(&self.root)) {
            Ok(chain) => chain,
            Err(error) => {
                self.refusal
                    .lock()
                    .expect("ignore refusal lock")
                    .get_or_insert(error);
                return true;
            }
        };
        let mut found = [Match::None, Match::None, Match::None, Match::None];
        let levels = chain
            .iter()
            .rev()
            .map(|level| &**level)
            .chain(&self.ancestors);
        for level in levels {
            for (slot, matcher) in level.0.iter().enumerate() {
                if let (true, Some(matcher)) = (found[slot].is_none(), matcher) {
                    found[slot] = matcher.matched(path, is_dir).map(|_| ());
                }
            }
        }
        let [custom, ignore, git, exclude] = found;
        let global = self.global.matched(path, is_dir).map(|_| ());
        match custom.or(ignore).or(git).or(exclude).or(global) {
            Match::Ignore(()) => false,
            Match::Whitelist(()) => true,
            Match::None => !(self.hidden && is_hidden(entry)),
        }
    }

    /// Levels held under the root.
    #[cfg(test)]
    pub(super) fn retained(&self) -> usize {
        self.stack.lock().expect("ignore levels lock").len()
    }

    /// The first refused ignore file, as the run's fatal error.
    pub(super) fn check(&self) -> Result<()> {
        match self.refusal.lock().expect("ignore refusal lock").take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// A name starting with `.`; on Windows also the hidden attribute, as `ignore` decides.
fn is_hidden(entry: &DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if entry
            .metadata()
            .is_ok_and(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        {
            return true;
        }
    }
    entry.file_name().as_encoded_bytes().starts_with(b".")
}
