use super::*;
use crate::resource_limits::is_fatal_input_error;
use std::fs;

/// The canonical root of a fresh temporary directory, as `Config::root` holds it.
fn canonical(directory: &tempfile::TempDir) -> PathBuf {
    crate::config::canonical_platform_path(directory.path()).unwrap()
}

/// The containment refusal in `error`'s chain, if any.
fn refusal(error: &anyhow::Error) -> Option<&ContainmentRefused> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ContainmentRefused>())
}

/// A regular file under the root reads; the result is its exact content on every platform.
#[test]
fn a_regular_file_inside_the_root_reads() {
    let directory = tempfile::tempdir().unwrap();
    let root = canonical(&directory);
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/file.json"), "{}\n").unwrap();
    let contained = Contained::new(&root, CONFIG_REMEDY);
    let text = contained
        .read_utf8(&root.join("nested/file.json"), "test file", 16)
        .unwrap();
    assert_eq!(text, "{}\n");
}

/// Known external paths stay external; uninspectable aliases require contained reads.
#[test]
fn location_checks_keep_unknown_aliases_contained() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = canonical(&directory);
    let external = canonical(&outside);
    let filesystem_root = root.ancestors().last().unwrap();
    let unknown = external.join("missing/child");
    let contained = Contained::new(&root, CONFIG_REMEDY);
    for (path, expected) in [
        (root.as_path(), true),
        (external.as_path(), false),
        (filesystem_root, false),
        (unknown.as_path(), true),
    ] {
        assert_eq!(contained.holds(path), expected, "{}", path.display());
    }
}

/// A missing file is an ordinary I/O error, not a refusal.
#[test]
fn a_missing_file_is_not_a_refusal() {
    let directory = tempfile::tempdir().unwrap();
    let root = canonical(&directory);
    let error = Contained::new(&root, CONFIG_REMEDY)
        .read_utf8(&root.join("absent.json"), "test file", 16)
        .unwrap_err();
    assert!(refusal(&error).is_none(), "{error:#}");
    assert!(!is_fatal_input_error(&error), "{error:#}");
}

/// Invalid UTF-8 in a contained file is an ordinary error naming the file.
#[test]
fn invalid_utf8_is_not_a_refusal() {
    let directory = tempfile::tempdir().unwrap();
    let root = canonical(&directory);
    fs::write(root.join("bad.json"), [0xff]).unwrap();
    let error = Contained::new(&root, CONFIG_REMEDY)
        .read_utf8(&root.join("bad.json"), "test file", 16)
        .unwrap_err();
    assert!(format!("{error:#}").contains("test file"), "{error:#}");
    assert!(
        format!("{error:#}").contains("is not valid UTF-8"),
        "{error:#}"
    );
    assert!(!is_fatal_input_error(&error), "{error:#}");
}

/// The input limit still applies to contained reads and stays fatal.
#[test]
fn the_input_limit_applies() {
    let directory = tempfile::tempdir().unwrap();
    let root = canonical(&directory);
    fs::write(root.join("large.json"), "x".repeat(17)).unwrap();
    let error = Contained::new(&root, CONFIG_REMEDY)
        .read_bytes(&root.join("large.json"), "test file", 16)
        .unwrap_err();
    assert!(is_fatal_input_error(&error), "{error:#}");
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::symlink;

    /// An unreadable regular file retains its ordinary I/O classification.
    #[test]
    fn an_unreadable_regular_file_is_not_a_refusal() {
        use std::os::unix::fs::PermissionsExt;
        for linked in [false, true] {
            for deny_parent in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let root = canonical(&directory);
                let parent = root.join("private");
                fs::create_dir(&parent).unwrap();
                let target = parent.join("regular.json");
                fs::write(&target, "{}").unwrap();
                let path = if linked {
                    let path = root.join("linked.json");
                    symlink(&target, &path).unwrap();
                    path
                } else {
                    target.clone()
                };
                let denied = if deny_parent { &parent } else { &target };
                fs::set_permissions(denied, fs::Permissions::from_mode(0o0)).unwrap();
                let read = Contained::new(&root, CONFIG_REMEDY).read_utf8(&path, "test file", 64);
                fs::set_permissions(denied, fs::Permissions::from_mode(0o700)).unwrap();
                let error = read.unwrap_err();
                assert!(refusal(&error).is_none(), "{error:#}");
                assert!(!is_fatal_input_error(&error), "{error:#}");
                assert_eq!(
                    error.downcast_ref::<io::Error>().unwrap().kind(),
                    io::ErrorKind::PermissionDenied
                );
            }
        }
    }

    /// Links resolving inside the root read; links or parents resolving outside are refused.
    #[test]
    fn links_are_contained_by_their_target() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = canonical(&directory);
        fs::write(root.join("target.json"), "inside").unwrap();
        fs::write(outside.path().join("target.json"), "outside").unwrap();
        symlink(root.join("target.json"), root.join("inside-link.json")).unwrap();
        symlink(
            outside.path().join("target.json"),
            root.join("outside-link.json"),
        )
        .unwrap();
        symlink(outside.path(), root.join("outside-dir")).unwrap();

        let contained = Contained::new(&root, CONFIG_REMEDY);
        assert_eq!(
            contained
                .read_utf8(&root.join("inside-link.json"), "test file", 64)
                .unwrap(),
            "inside"
        );
        for name in ["outside-link.json", "outside-dir/target.json"] {
            let error = contained
                .read_utf8(&root.join(name), "test file", 64)
                .unwrap_err();
            let refused = refusal(&error).unwrap_or_else(|| panic!("{name}: {error:#}"));
            assert_eq!(refused.reason, Reason::Outside, "{name}");
            assert!(is_fatal_input_error(&error), "{name}");
            let message = error.to_string();
            assert!(
                message.contains("resolves outside the analysis root")
                    && message.contains(CONFIG_REMEDY),
                "{message}"
            );
        }
    }

    /// A handle that is not the file the path now names is refused: the path was replaced after
    /// it was opened, so what was opened is not what containment checked.
    #[test]
    fn a_replaced_path_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = canonical(&directory);
        fs::write(outside.path().join("secret"), "outside").unwrap();
        fs::write(root.join("config.json"), "inside").unwrap();
        // The outside file was opened; the path then names an inside file.
        let opened = File::open(outside.path().join("secret")).unwrap();
        let error = Contained::new(&root, LINK_REMEDY)
            .verify(&root.join("config.json"), "test file", &opened)
            .unwrap_err();
        let refused = refusal(&error).expect("a refusal");
        assert_eq!(refused.reason, Reason::Replaced);
        assert!(
            error.to_string().contains("changed after it was opened"),
            "{error:#}"
        );
    }

    /// A FIFO is refused promptly instead of blocking the open or the read.
    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let directory = tempfile::tempdir().unwrap();
        let root = canonical(&directory);
        let path = root.join("pipe.json");
        assert!(std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        let error = Contained::new(&root, CONFIG_REMEDY)
            .read_utf8(&path, "test file", 64)
            .unwrap_err();
        assert_eq!(
            refusal(&error).expect("a refusal").reason,
            Reason::NotRegular
        );
        assert!(
            error.to_string().contains("not a regular file"),
            "{error:#}"
        );
    }
}
