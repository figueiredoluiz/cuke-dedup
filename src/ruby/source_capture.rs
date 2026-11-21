//! Records each Ruby source the adapter reads, so the sources the test suite writes inline can be
//! compiled under real Ruby (`scripts/check/check-ruby-syntax.mjs --captured <dir>`).
//!
//! Built only with the `ruby-source-capture` feature, and active only while
//! `CUKE_DEDUP_RUBY_SOURCE_CAPTURE` names a directory. Without the feature `record` is empty.

/// Environment variable naming the capture directory; unset or empty records nothing.
#[cfg(feature = "ruby-source-capture")]
const DIRECTORY: &str = "CUKE_DEDUP_RUBY_SOURCE_CAPTURE";

/// Records `source` when capture is enabled.
#[cfg(feature = "ruby-source-capture")]
pub(crate) fn record(source: &str) {
    let _recorded = record_in(std::env::var_os(DIRECTORY).as_deref(), source);
}

/// Records nothing: the build has no capture feature.
#[cfg(not(feature = "ruby-source-capture"))]
#[inline(always)]
pub(crate) fn record(_source: &str) {}

/// Writes `source` once per distinct content into `directory` and returns its file, or `None`
/// when capture is off.
///
/// Test processes run concurrently, so each writer stages a uniquely named file and renames it
/// into place; a reader never sees a partial source. A write failure panics: a source that is not
/// recorded would pass the Ruby check unseen.
#[cfg(feature = "ruby-source-capture")]
fn record_in(directory: Option<&std::ffi::OsStr>, source: &str) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static STAGED: AtomicUsize = AtomicUsize::new(0);

    // No directory, or an empty one, means capture is off: never fall back to the working directory.
    let directory = directory.filter(|value| !value.is_empty())?;
    let directory = std::path::Path::new(directory);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    let target = directory.join(format!("{:016x}-{}.rb", hasher.finish(), source.len()));
    if target.is_file() {
        return Some(target);
    }
    let staged = directory.join(format!(
        ".{}-{}.staged",
        std::process::id(),
        STAGED.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(directory)
        .and_then(|()| std::fs::write(&staged, source))
        .and_then(|()| std::fs::rename(&staged, &target))
        .unwrap_or_else(|error| {
            panic!(
                "cannot record Ruby source in {}: {error}",
                directory.display()
            )
        });
    Some(target)
}

#[cfg(all(test, feature = "ruby-source-capture"))]
mod tests;
