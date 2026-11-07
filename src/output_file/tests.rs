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
        let entries: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(fs::read(root.path().join("moved")).unwrap(), b"prepared");
        let substituted = entries
            .iter()
            .find(|path| path.file_name().unwrap() != "moved")
            .unwrap();
        assert_eq!(fs::read(substituted).unwrap(), b"substitution");
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
    if !rustix::process::geteuid().is_root() {
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
    }

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

/// Descriptor guards reject nonregular files and missing, replaced or linked names without changing data.
#[cfg(unix)]
#[test]
fn descriptor_guards_preserve_original_and_external_bytes() {
    use std::{ffi::OsStr, os::unix::fs::symlink};
    let root = tempfile::tempdir().unwrap();
    let original = root.path().join("original");
    let external = root.path().join("external");
    fs::write(&original, b"original").unwrap();
    fs::write(&external, b"external").unwrap();
    let file = fs::File::open(&original).unwrap();
    let dir = super::platform::open_parent(root.path()).unwrap();
    assert!(super::platform::require_regular(&file).is_ok());
    assert!(super::platform::require_regular(&dir).is_err());
    let check = |parent: &fs::File, name: &str| {
        super::platform::require_entry(parent, OsStr::new(name), &file, "entry changed")
    };
    assert!(check(&dir, "original").is_ok());
    assert!(check(&file, "child").is_err());
    assert!(check(&dir, "missing").is_err());
    let entry = root.path().join("entry");
    fs::write(&entry, b"other").unwrap();
    assert!(check(&dir, "entry").is_err());
    assert_eq!(fs::read(&entry).unwrap(), b"other");
    fs::remove_file(&entry).unwrap();
    symlink(&external, &entry).unwrap();
    assert!(check(&dir, "entry").is_err());
    assert!(entry.is_symlink());
    assert_eq!(fs::read(&original).unwrap(), b"original");
    assert_eq!(fs::read(&external).unwrap(), b"external");
}

/// Stage guards and cleanup preserve substitutions and handle entries removed by another writer.
#[cfg(unix)]
#[test]
fn staging_guards_validate_real_entries_and_preserve_substitutions() {
    use rustix::fs::{statat, AtFlags};
    use std::{
        ffi::OsStr,
        os::unix::fs::{symlink, PermissionsExt},
    };
    for case in ["ordinary", "mode-changed", "replaced", "linked", "removed"] {
        let root = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("sentinel"), b"external").unwrap();
        let stage = root.path().join("stage");
        let saved = root.path().join("saved");
        fs::create_dir(&stage).unwrap();
        fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
        let dir = super::platform::open_parent(root.path()).unwrap();
        let name = OsStr::new("stage");
        let initial = statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).unwrap();
        match case {
            "ordinary" => {}
            "mode-changed" => {
                fs::set_permissions(&stage, fs::Permissions::from_mode(0o755)).unwrap()
            }
            "replaced" => {
                fs::rename(&stage, &saved).unwrap();
                fs::create_dir(&stage).unwrap();
                fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
            }
            "linked" => {
                fs::rename(&stage, &saved).unwrap();
                symlink(external.path(), &stage).unwrap();
            }
            "removed" => fs::remove_dir(&stage).unwrap(),
            _ => unreachable!(),
        }
        let opened = super::platform::open_stage(&dir, name, &initial);
        assert_eq!(opened.is_ok(), case == "ordinary", "{case}");
        drop(opened);
        super::platform::cleanup_stage(&dir, name, &initial);
        match case {
            "replaced" => assert!(stage.is_dir() && saved.is_dir()),
            "linked" => assert!(stage.is_symlink() && saved.is_dir()),
            _ => assert!(!stage.exists(), "{case}"),
        }
        assert_eq!(
            fs::read(external.path().join("sentinel")).unwrap(),
            b"external"
        );

        let ordinary = root.path().join("ordinary-file");
        fs::write(&ordinary, b"preserve").unwrap();
        let initial = statat(&dir, OsStr::new("ordinary-file"), AtFlags::SYMLINK_NOFOLLOW).unwrap();
        super::platform::cleanup_stage(&dir, OsStr::new("ordinary-file"), &initial);
        assert_eq!(fs::read(&ordinary).unwrap(), b"preserve");
    }
}

/// Windows failures remove only unchanged temporary entries and preserve substitutions and destination directories.
#[cfg(windows)]
#[test]
fn windows_failure_cleanup_preserves_foreign_entries() {
    use std::{ffi::OsStr, io::Write};
    for case in ["prepare", "substitute", "persist"] {
        let root = tempfile::tempdir().unwrap();
        let report = root.path().join("report");
        let dir = super::platform::open_parent(root.path()).unwrap();
        let result = super::platform::publish(&dir, OsStr::new("report"), false, |file| {
            file.write_all(b"prepared")?;
            if case == "persist" {
                fs::create_dir(&report)?;
                return Ok(());
            }
            if case == "substitute" {
                let source = fs::read_dir(root.path())?.next().unwrap()?.path();
                fs::rename(&source, root.path().join("moved"))?;
                fs::write(source, b"substitute")?;
            }
            anyhow::bail!("owned preparation failure")
        });
        assert!(result.is_err(), "{case}");
        let entries: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        match case {
            "prepare" => assert!(entries.is_empty()),
            "substitute" => {
                assert_eq!(entries.len(), 2);
                assert_eq!(fs::read(root.path().join("moved")).unwrap(), b"prepared");
                let substitute = entries
                    .iter()
                    .find(|entry| entry.file_name().unwrap() != "moved")
                    .unwrap();
                assert_eq!(fs::read(substitute).unwrap(), b"substitute");
                assert!(!report.exists());
            }
            "persist" => {
                assert_eq!(entries.len(), 1);
                assert!(report.is_dir());
            }
            _ => unreachable!(),
        }
    }
}
