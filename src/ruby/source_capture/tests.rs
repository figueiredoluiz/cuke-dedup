use super::record_in;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FIRST: &str = "Given('a') { }\n";
const SECOND: &str = "Given('b') { }\n";

/// Records `source` into `directory`.
fn record_into(directory: &Path, source: &str) -> Option<PathBuf> {
    record_in(Some(directory.as_os_str()), source)
}

/// File name to contents for everything in `directory`.
fn recorded(directory: &Path) -> BTreeMap<String, String> {
    std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                text,
            )
        })
        .collect()
}

/// Repeated content is stored once; each file holds the exact bytes and names its length.
#[test]
fn records_each_distinct_source_once_with_its_exact_bytes() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("captured");
    let first = record_into(&directory, FIRST);
    assert_eq!(record_into(&directory, FIRST), first);
    record_into(&directory, SECOND);
    let files = recorded(&directory);
    let mut contents: Vec<_> = files.values().map(String::as_str).collect();
    contents.sort_unstable();
    assert_eq!(contents, [FIRST, SECOND], "{files:?}");
    assert!(
        files
            .iter()
            .all(|(name, text)| name.ends_with(&format!("-{}.rb", text.len()))),
        "{files:?}"
    );
}

/// Capture stays off without a directory; the same call with one records.
#[test]
fn unset_or_empty_directory_records_nothing() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(record_in(None, FIRST), None);
    // An empty value must not fall back to the working directory.
    assert_eq!(record_in(Some(std::ffi::OsStr::new("")), FIRST), None);
    assert!(recorded(root.path()).is_empty());
    // Control: the same call with a directory records the source.
    let target = record_into(root.path(), FIRST).unwrap();
    assert_eq!(target.parent(), Some(root.path()));
    assert_eq!(recorded(root.path()).len(), 1);
}

/// A directory that cannot be created fails loudly, naming it, instead of dropping the source.
#[test]
fn unwritable_directory_panics_with_its_path() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("not-a-directory");
    std::fs::write(&file, "").unwrap();
    let panic = std::panic::catch_unwind(|| record_into(&file, FIRST)).unwrap_err();
    let message = panic.downcast_ref::<String>().unwrap();
    let expected = format!("cannot record Ruby source in {}: ", file.display());
    assert!(message.starts_with(&expected), "{message}");
}
