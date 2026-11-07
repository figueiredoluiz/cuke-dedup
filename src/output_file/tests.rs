use super::OutputFile;
use std::fs;

/// A replaced parent cannot redirect an in-flight write and a changed leaf aborts publication.
#[cfg(unix)]
#[test]
fn replacement_stays_in_the_open_directory_and_refuses_changed_leaves() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("parent");
    let moved = root.path().join("moved");
    let external = tempfile::tempdir().unwrap();
    let path = parent.join("result");
    let output = OutputFile::open(&path, false).unwrap();
    fs::rename(&parent, &moved).unwrap();
    symlink(external.path(), &parent).unwrap();
    output.replace(b"first").unwrap();
    assert!(!external.path().join("result").exists());
    let path = moved.join("result");
    let output = OutputFile::open(&path, true).unwrap();
    fs::rename(&path, moved.join("original")).unwrap();
    fs::write(&path, b"second").unwrap();
    assert!(output.replace(b"unexpected").is_err());
    assert_eq!(fs::read(&path).unwrap(), b"second");
    assert_eq!(fs::read(moved.join("original")).unwrap(), b"first");
    assert_eq!(fs::read_dir(&moved).unwrap().count(), 2);
}

/// Traversing a real directory requires search permission, including when listing it is forbidden.
#[cfg(unix)]
#[test]
fn writes_through_search_only_directories() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let ancestor = root.path().join("private");
    let child = ancestor.join("reports");
    fs::create_dir_all(&child).unwrap();
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o111)).unwrap();
    let path = child.join("result");
    let result = OutputFile::open(&path, false).and_then(|file| file.replace(b"published"));
    fs::set_permissions(&ancestor, fs::Permissions::from_mode(0o700)).unwrap();
    result.unwrap();
    assert_eq!(fs::read(path).unwrap(), b"published");
}

/// Junctions must be refused before a new report directory is created in their target.
#[cfg(windows)]
#[test]
fn refuses_junction_before_creating_report_directory() {
    let parent = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let link = parent.path().join("linked");
    assert!(std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(&link)
        .arg(external.path())
        .status()
        .unwrap()
        .success());
    let report = external.path().join("result");
    fs::write(&report, b"previous").unwrap();
    let alias = external.path().join("other-name");
    fs::hard_link(&report, &alias).unwrap();
    OutputFile::open(&report, true)
        .unwrap()
        .replace(b"new")
        .unwrap();
    assert_eq!(fs::read(&alias).unwrap(), b"previous");
    assert_eq!(fs::read(&report).unwrap(), b"new");
    assert!(OutputFile::open(&link.join("new/result"), false).is_err());
    assert!(!external.path().join("new").exists());
    fs::remove_dir(link).unwrap();
}

/// Replacing an entry in the output directory cannot substitute the payload of a successful report.
#[test]
fn concurrent_staging_entry_replacement_cannot_publish_other_bytes() {
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let dir = super::platform::open_parent(root.path()).unwrap();
    let report = root.path().join("report");
    let result = super::platform::publish(&dir, std::ffi::OsStr::new("report"), false, |file| {
        file.write_all(b"prepared")?;
        let staging = fs::read_dir(root.path())?.next().unwrap()?.path();
        fs::rename(&staging, root.path().join("moved"))?;
        fs::write(staging, b"substitution")?;
        Ok(())
    });
    #[cfg(unix)]
    result.unwrap();
    #[cfg(windows)]
    if result.is_err() {
        assert!(!report.exists());
        return;
    }
    assert_eq!(fs::read(report).unwrap(), b"prepared");
}

/// Relative directory handles preserve the invocation directory without changing process-wide cwd.
#[cfg(unix)]
#[test]
fn relative_directory_handles_preserve_current_directory() {
    let before = std::env::current_dir().unwrap();
    let directory = super::platform::open_parent(std::path::Path::new(".")).unwrap();
    assert!(super::platform::same_file(&directory, &fs::File::open(".").unwrap()).unwrap());
    assert_eq!(std::env::current_dir().unwrap(), before);
}

/// Permission errors and removed payloads abort publication without damaging previous bytes.
#[cfg(unix)]
#[test]
fn output_failures_preserve_files_and_remove_owned_staging_entries() {
    use std::{ffi::OsStr, io::Write, os::unix::fs::PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report");
    fs::write(&path, b"previous").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    let refused = OutputFile::open(&path, false).is_err();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(refused);
    assert_eq!(fs::read(&path).unwrap(), b"previous");

    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let refused = OutputFile::open(&root.path().join("new/report"), false).is_err();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(refused);
    assert!(!root.path().join("new").exists());

    let dir = super::platform::open_parent(root.path()).unwrap();
    for substitute in [false, true] {
        let result = super::platform::publish(&dir, OsStr::new("report"), true, |file| {
            file.write_all(b"prepared")?;
            let stage = fs::read_dir(root.path())?
                .map(|entry| entry.unwrap().path())
                .find(|path| path.is_dir())
                .unwrap();
            fs::remove_file(stage.join("payload"))?;
            if substitute {
                fs::write(stage.join("payload"), b"another writer")?;
            }
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        // A replacement payload belongs to the competing writer and is preserved.
        for entry in fs::read_dir(root.path()).unwrap() {
            let stage = entry.unwrap().path();
            if stage.is_dir() {
                assert!(substitute);
                assert_eq!(fs::read(stage.join("payload")).unwrap(), b"another writer");
                fs::remove_dir_all(stage).unwrap();
            }
        }
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}
