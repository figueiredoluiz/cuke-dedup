use super::*;
use crate::model::{
    DefinitionCluster, DefinitionComparison, FindingEvidence, Rule, Severity, SourceLocation,
};
use std::fs;

/// Initializes a temporary Git repository with a test identity and commits its fixture files.
fn initialize_repository(root: &Path) {
    for arguments in [
        ["init", "-q"].as_slice(),
        ["config", "user.email", "test@example.com"].as_slice(),
        ["config", "user.name", "Test"].as_slice(),
        ["add", "."].as_slice(),
        ["commit", "-qm", "initial"].as_slice(),
    ] {
        assert!(fixture_git()
            .args(arguments)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
}

/// Git exports `GIT_DIR`, `GIT_INDEX_FILE`, and friends to the processes it spawns. When this
/// suite runs from a Git hook, those variables outrank the `current_dir` and `-C` of a nested
/// invocation, so a fixture's `git add` would stage temporary paths into the real repository's
/// index and mark every tracked file deleted. Strip that environment instead.
fn fixture_git() -> Command {
    let mut command = Command::new("git");
    clear_git_environment(&mut command);
    command
}

/// Builds a duplicate-matcher finding with primary and shared related locations beneath the supplied root.
fn finding(root: &Path, file: &str) -> Finding {
    Finding {
        original_fingerprint: None,
        rule: Rule::DuplicateMatcher,
        severity: Severity::Error,
        message: "duplicate".to_owned(),
        primary: SourceLocation::new(root.join(file), 2, 1, 2, 10),
        related: vec![SourceLocation::new(
            root.join("steps/shared.ts"),
            4,
            1,
            4,
            10,
        )],
        evidence: FindingEvidence {
            matcher_similarity: Some(1.0),
            handler_similarity: Some(0.5),
            matcher_difference: "same".to_owned(),
            handler_evidence: "different".to_owned(),
            comparison: None,
            cluster: None,
        },
        suggested_action: "consolidate".to_owned(),
        suppression: None,
    }
}

/// Checks changed mode retains only the finding whose primary file changed.
#[test]
fn changed_mode_keeps_findings_related_to_a_changed_file() {
    let root = Path::new("/repo");
    let mut findings = vec![
        finding(root, "steps/changed.ts"),
        finding(root, "steps/old.ts"),
    ];
    let changed = BTreeSet::from([root.join("steps/changed.ts")]);
    retain_changed_findings(&mut findings, &changed);
    assert_eq!(findings.len(), 1);
    assert!(findings[0].primary.path.ends_with("changed.ts"));
}

/// Checks cluster fingerprints ignore locations and member order but change when membership changes.
#[test]
fn cluster_fingerprints_are_location_and_member_order_independent() {
    let root = Path::new("/repo");
    let mut original = finding(root, "steps/a.ts");
    original.evidence.cluster = Some(DefinitionCluster {
        member_count: 3,
        definition_fingerprints: vec!["charlie".into(), "alpha".into(), "bravo".into()],
        pair_findings_collapsed: 2,
        members_truncated: false,
    });
    let mut moved = finding(root, "moved/renamed.ts");
    moved.evidence.cluster = Some(DefinitionCluster {
        member_count: 3,
        definition_fingerprints: vec!["bravo".into(), "charlie".into(), "alpha".into()],
        pair_findings_collapsed: 99,
        members_truncated: false,
    });

    assert_eq!(finding_fingerprint(&original), finding_fingerprint(&moved));
    moved
        .evidence
        .cluster
        .as_mut()
        .unwrap()
        .definition_fingerprints
        .push("delta".into());
    assert_ne!(finding_fingerprint(&original), finding_fingerprint(&moved));
}

/// Checks embedded separators cannot make an unrelated cluster match a baseline fingerprint.
#[test]
fn multipart_framing_cannot_suppress_a_different_cluster() {
    let directory = tempfile::tempdir().unwrap();
    let baseline_path = directory.path().join("baseline.json");
    let mut accepted = finding(directory.path(), "steps/accepted.ts");
    accepted.evidence.cluster = Some(DefinitionCluster {
        member_count: 2,
        definition_fingerprints: vec!["a\0b".into(), "c".into()],
        pair_findings_collapsed: 1,
        members_truncated: false,
    });
    let mut unrelated = finding(directory.path(), "steps/unrelated.ts");
    unrelated.evidence.cluster = Some(DefinitionCluster {
        member_count: 2,
        definition_fingerprints: vec!["a".into(), "b\0c".into()],
        pair_findings_collapsed: 1,
        members_truncated: false,
    });
    let baseline = BaselineFile::new(BTreeMap::from([(finding_fingerprint(&accepted), 1)]));
    fs::write(&baseline_path, serde_json::to_string(&baseline).unwrap()).unwrap();

    let outcome = apply_baseline(std::slice::from_mut(&mut unrelated), &baseline_path).unwrap();

    assert_eq!(outcome.suppressed, 0);
    assert_eq!(outcome.new_findings, 1);
    assert!(unrelated.is_active());
}

/// Checks fallback fingerprint framing suppresses an equivalent finding without conflating different evidence fields.
#[test]
fn multipart_framing_cannot_suppress_a_different_fallback_finding() {
    let directory = tempfile::tempdir().unwrap();
    let baseline_path = directory.path().join("baseline.json");
    let mut accepted = finding(directory.path(), "steps/accepted.ts");
    accepted.message = "a\0b".to_owned();
    let mut equivalent = accepted.clone();
    let mut unrelated = finding(directory.path(), "steps/unrelated.ts");
    unrelated.message = "a".to_owned();
    unrelated.evidence.matcher_difference = "b\0same".to_owned();
    let baseline = BaselineFile::new(BTreeMap::from([(finding_fingerprint(&accepted), 1)]));
    fs::write(&baseline_path, serde_json::to_string(&baseline).unwrap()).unwrap();

    let equivalent_outcome =
        apply_baseline(std::slice::from_mut(&mut equivalent), &baseline_path).unwrap();
    let unrelated_outcome =
        apply_baseline(std::slice::from_mut(&mut unrelated), &baseline_path).unwrap();

    assert_eq!(equivalent_outcome.suppressed, 1);
    assert!(!equivalent.is_active());
    assert_eq!(unrelated_outcome.suppressed, 0);
    assert_eq!(unrelated_outcome.new_findings, 1);
}

/// Checks changed-file discovery handles subdirectory roots, untracked files, Unicode, and significant filename spaces.
#[test]
fn changed_files_are_relative_to_a_subdirectory_and_include_untracked_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("packages/e2e/steps")).unwrap();
    fs::write(root.join("packages/e2e/steps/träcked.ts"), "before").unwrap();
    initialize_repository(root);
    fs::write(root.join("packages/e2e/steps/träcked.ts"), "after").unwrap();
    fs::write(root.join("packages/e2e/steps/ new step.ts"), "new").unwrap();
    #[cfg(unix)]
    fs::write(root.join("packages/e2e/steps/trailing step.ts "), "new").unwrap();

    let subdirectory = root.join("packages/e2e");
    let changed = git_changed_files(&subdirectory, "HEAD").unwrap();
    let mut expected = BTreeSet::from([
        subdirectory.join("steps/ new step.ts"),
        subdirectory.join("steps/träcked.ts"),
    ]);
    #[cfg(unix)]
    expected.insert(subdirectory.join("steps/trailing step.ts "));
    assert_eq!(changed, expected);
}

/// Checks linked worktrees compare their own ordinary or split index and honor shared ignore rules.
#[test]
fn changed_files_use_the_linked_worktree_index() {
    for split_index in [false, true] {
        assert_linked_worktree_changes(split_index);
    }
}

/// Checks clean and changed outcomes for one linked worktree index layout.
fn assert_linked_worktree_changes(split_index: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("repository");
    let linked = directory.path().join("linked");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("unstaged.ts"), "before").unwrap();
    initialize_repository(&root);
    let output = fixture_git()
        .current_dir(&root)
        .args(["worktree", "add", "--detach"])
        .arg(&linked)
        .arg("HEAD")
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(git_changed_files(&linked, "HEAD").unwrap(), BTreeSet::new());
    fs::create_dir_all(root.join(".git/info")).unwrap();
    fs::write(root.join(".git/info/exclude"), "ignored.ts\n").unwrap();
    for name in ["ignored.ts", "unstaged.ts", "staged.ts", "untracked.ts"] {
        fs::write(linked.join(name), "after").unwrap();
    }
    let output = fixture_git()
        .current_dir(&linked)
        .args(["add", "staged.ts"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);

    let layout = if split_index {
        "--split-index"
    } else {
        "--no-split-index"
    };
    let output = fixture_git()
        .current_dir(&linked)
        .args(["update-index", layout])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);

    let expected = BTreeSet::from([
        linked.join("staged.ts"),
        linked.join("unstaged.ts"),
        linked.join("untracked.ts"),
    ]);
    for configured_split in ["false", "true"] {
        let output = fixture_git()
            .current_dir(&linked)
            .args(["config", "core.splitIndex", configured_split])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        let changed = git_changed_files(&linked, "HEAD").unwrap_or_else(|error| {
            panic!("index layout {layout}, core.splitIndex={configured_split}: {error:#}")
        });
        assert_eq!(
            changed, expected,
            "{layout}, core.splitIndex={configured_split}"
        );
    }
}

/// Checks changed-file discovery preserves newline bytes in Unix filenames.
#[cfg(unix)]
#[test]
fn changed_files_preserve_newlines_in_unix_names() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let steps = root.join("steps");
    fs::create_dir_all(&steps).unwrap();
    let newline = steps.join("line\nbreak.ts");
    fs::write(&newline, "before").unwrap();
    initialize_repository(root);
    fs::write(&newline, "after").unwrap();

    let changed = git_changed_files(root, "HEAD").unwrap();
    assert_eq!(changed, BTreeSet::from([newline]));
}

/// Checks Git path decoding retains non-UTF-8 Unix filename bytes.
#[cfg(unix)]
#[test]
fn git_path_decoder_preserves_non_utf8_unix_bytes() {
    use std::os::unix::ffi::OsStrExt;

    let bytes = b"steps/non-utf8-\xff.ts";
    let path = path_from_git_bytes(bytes).unwrap();
    assert_eq!(path.as_os_str().as_bytes(), bytes);
}

/// Checks changed-file discovery retains a modified non-UTF-8 Linux filename.
#[cfg(target_os = "linux")]
#[test]
fn changed_files_preserve_non_utf8_linux_names() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let steps = root.join("steps");
    fs::create_dir_all(&steps).unwrap();
    let non_utf8 = steps.join(OsString::from_vec(b"non-utf8-\xff.ts".to_vec()));
    fs::write(&non_utf8, "before").unwrap();
    initialize_repository(root);
    fs::write(&non_utf8, "after").unwrap();

    let changed = git_changed_files(root, "HEAD").unwrap();
    assert_eq!(changed, BTreeSet::from([non_utf8]));
}

/// Checks baseline matching survives file and line changes but suppresses only the recorded number of occurrences.
#[test]
fn semantic_baseline_survives_location_changes_and_enforces_multiplicity() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let baseline_path = root.join("baseline.json");
    let mut accepted = vec![finding(root, "steps/original.ts")];
    let update = update_baseline(&mut accepted, &baseline_path).unwrap();
    assert_eq!(update.added, 1);
    assert_eq!(update.removed, 0);
    assert_eq!(update.suppressed, 1);

    let mut current = vec![
        finding(root, "moved/renamed.ts"),
        finding(root, "steps/third-copy.ts"),
    ];
    current[0].primary.line = 200;
    let outcome = apply_baseline(&mut current, &baseline_path).unwrap();
    assert_eq!(outcome.suppressed, 1);
    assert_eq!(outcome.new_findings, 1);
    assert!(!current[0].is_active());
    assert!(current[1].is_active());

    let baseline: BaselineFile =
        serde_json::from_str(&fs::read_to_string(&baseline_path).unwrap()).unwrap();
    assert_eq!(baseline.schema_version, BASELINE_SCHEMA_VERSION);
    assert_eq!(baseline.fingerprints.values().sum::<usize>(), 1);
}

/// Checks changed mode rejects invalid revisions, failed Git commands, ignored roots, and non-repository roots.
#[test]
fn changed_mode_validates_revisions_repositories_and_ignored_roots() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("tracked.ts"), "initial").unwrap();
    initialize_repository(directory.path());
    assert!(ensure_changed_root_is_trackable(&directory.path().canonicalize().unwrap()).is_ok());
    let git = ChangedGit::new(directory.path()).unwrap();
    for revision in ["", " ", "--all", "missing-revision"] {
        assert!(
            resolve_commit(git.metadata_command(directory.path()), revision).is_err(),
            "{revision}"
        );
    }
    let query = |command, arguments: &[&str], context: &str| {
        git_paths(
            command,
            arguments,
            context,
            directory.path(),
            &mut ChangedPathBudget {
                records: 4,
                bytes: 1024,
            },
        )
    };
    let failed_query = query(
        git.worktree_command(directory.path()),
        &["not-a-command"],
        "expected failure",
    )
    .unwrap_err();
    let message = failed_query.to_string();
    assert!(message.starts_with("expected failure: "), "{message}");
    assert!(message.contains("not-a-command"), "{message}");

    let failed_start = query(
        Command::new(directory.path().join("missing-git")),
        &[],
        "cannot start Git listing",
    )
    .unwrap_err();
    assert_eq!(failed_start.to_string(), "cannot start Git listing");
    assert_eq!(
        failed_start
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::NotFound
    );

    let ignored = directory.path().join("ignored");
    fs::create_dir(&ignored).unwrap();
    fs::write(directory.path().join(".gitignore"), "ignored/\n").unwrap();
    assert!(
        ensure_changed_root_is_trackable(&ignored.canonicalize().unwrap())
            .unwrap_err()
            .to_string()
            .contains("ignored by Git")
    );

    let outside = tempfile::tempdir().unwrap();
    assert!(ensure_changed_root_is_trackable(outside.path()).is_err());
}

/// Checks SHA-256 comparisons with absent and valueless worktree settings.
#[test]
fn changed_files_accept_sha256_and_implicit_git_settings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let initialized = fixture_git()
        .current_dir(root)
        .args(["init", "--quiet", "--object-format=sha256"])
        .output()
        .unwrap();
    assert!(initialized.status.success(), "{:?}", initialized.stderr);
    let source = root.join("tracked.ts");
    fs::write(&source, "before").unwrap();
    initialize_repository(root);
    fs::write(&source, "after").unwrap();
    let config =
        "[core]\nrepositoryformatversion = 1\nbare = false\n[extensions]\nobjectformat = sha256\n";
    for settings in [config.to_owned(), format!("{config}[core]\nfilemode\n")] {
        fs::write(root.join(".git/config"), settings).unwrap();
        assert_eq!(
            git_changed_files(root, "HEAD").unwrap(),
            BTreeSet::from([source.clone()])
        );
    }
}

/// Checks Git metadata copies preserve contents and reject invalid sources before writing.
#[test]
fn git_comparison_metadata_copy_enforces_file_and_size_limits() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let input = root.join("input");
    let output = root.join("scratch/info");
    copy_git_info(&input, &output).unwrap();
    assert!(!output.exists());
    fs::write(&input, "*.tmp\n").unwrap();
    copy_git_info(&input, &output).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let symlink = root.join("symlink");
        std::os::unix::fs::symlink(&input, &symlink).unwrap();
        let error = copy_git_info(&symlink, &output).unwrap_err();
        assert!(
            error.to_string().contains("regular file of at most 1 MiB"),
            "{error}"
        );

        let permissions = fs::metadata(&input).unwrap().permissions();
        fs::set_permissions(&input, fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&input).is_err() {
            let error = copy_git_info(&input, &output).unwrap_err();
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }
        fs::set_permissions(&input, permissions).unwrap();
    }
    fs::write(&input, vec![b'a'; (1024 * 1024) + 1]).unwrap();
    for invalid in [input.as_path(), root] {
        let error = copy_git_info(invalid, &output).unwrap_err();
        assert!(
            error.to_string().contains("regular file of at most 1 MiB"),
            "{error}"
        );
    }
    let error = copy_git_info(&root.join("invalid\0path"), &output).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(fs::read(&output).unwrap(), b"*.tmp\n");
}

/// Creates a fixture repository with a single committed file.
fn repository_with_tracked_file() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let source = root.join("tracked.ts");
    fs::write(&source, "before").unwrap();
    initialize_repository(root);
    (directory, source)
}

/// Gives an index fixture a known old modification time.
fn set_index_test_timestamp(path: &Path) {
    let timestamp = fs::FileTimes::new().set_modified(
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_200_000_000),
    );
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(timestamp)
        .unwrap();
}

/// Checks Git comparison preserves the index timestamp and reports genuine input errors.
#[test]
fn changed_files_use_an_independent_index_snapshot() {
    let (directory, source) = repository_with_tracked_file();
    let root = directory.path();
    let original = root.join(".git/index");
    let snapshot = root.join(".git/fixture-snapshot");
    set_index_test_timestamp(&original);
    copy_git_index(&original, &snapshot).unwrap();
    assert_eq!(
        fs::metadata(&snapshot).unwrap().modified().unwrap(),
        fs::metadata(&original).unwrap().modified().unwrap()
    );
    assert_eq!(fs::read(&snapshot).unwrap(), fs::read(&original).unwrap());

    let occupied = root.join(".git/occupied");
    fs::create_dir(&occupied).unwrap();
    let prior = occupied.join("prior");
    fs::write(&prior, b"existing destination").unwrap();
    let error = copy_git_index(&original, &occupied).unwrap_err();
    assert!(
        matches!(
            error.kind(),
            std::io::ErrorKind::IsADirectory
                | std::io::ErrorKind::PermissionDenied
                | std::io::ErrorKind::AlreadyExists
                | std::io::ErrorKind::DirectoryNotEmpty
        ),
        "{error}"
    );
    assert_eq!(fs::read(&prior).unwrap(), b"existing destination");

    let absent_git = root.join("missing-git");
    let error = snapshot_shared_index(
        absent_git.as_os_str(),
        &original,
        &root.join(".git"),
        &root.join(".git/objects"),
        40,
    )
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("failed to inspect the private Git index"));

    fs::write(&original, b"invalid test index").unwrap();
    let error = git_changed_files(root, "HEAD").unwrap_err();
    assert!(error
        .to_string()
        .contains("failed to inspect the private Git index"));

    fs::remove_file(&original).unwrap();
    assert_eq!(
        git_changed_files(root, "HEAD").unwrap(),
        BTreeSet::from([source])
    );
    fs::create_dir(&original).unwrap();
    let error = git_changed_files(root, "HEAD").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("failed to snapshot the Git index"),
        "{error}"
    );
}

/// Checks the index snapshot copy admits the byte limit and leaves no file on refusal.
#[test]
fn changed_files_bound_index_snapshot_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source-index");
    let destination = directory.path().join("index-copy");
    let index = File::create(&source).unwrap();
    let limit = 64 * 1024 * 1024;

    index.set_len(limit + 1).unwrap();
    let error = copy_git_index(&source, &destination).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{error}");
    assert!(!destination.exists());

    index.set_len(limit).unwrap();
    copy_git_index(&source, &destination).unwrap();
    assert_eq!(fs::metadata(&destination).unwrap().len(), limit);

    for (input, expected, fits) in [
        (b"ab".as_slice(), b"ab".as_slice(), true),
        (b"abc".as_slice(), b"abc".as_slice(), true),
        (b"abcd".as_slice(), b"abc".as_slice(), false),
    ] {
        let mut output = Vec::new();
        let result = copy_git_bytes(input, &mut output, 3, "test input exceeds the limit");
        assert_eq!(result.is_ok(), fits);
        if let Err(error) = result {
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert_eq!(error.to_string(), "test input exceeds the limit");
        }
        assert_eq!(output, expected);
    }
}

/// Checks worktree settings are bounded before Git receives a partial or oversized argument list.
#[test]
fn changed_files_bound_worktree_settings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let tracked = root.join("tracked.ts");
    fs::write(&tracked, "before\n").unwrap();
    initialize_repository(root);
    fs::write(&tracked, "after\n").unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let executable = git_executable(root).unwrap();
    let config_path = root.join(".git/config");
    let original = fs::read_to_string(&config_path).unwrap();
    fs::write(&config_path, format!("{original}\n[core]\neol = lf\n")).unwrap();
    let settings = worktree_settings(executable.as_os_str(), root, scratch.path()).unwrap();
    assert!(settings.contains(&OsString::from("core.eol=lf")));
    assert_eq!(
        git_changed_files(root, "HEAD").unwrap(),
        BTreeSet::from([tracked])
    );

    let repeated = "eol = lf\n".repeat(24_000);
    fs::write(&config_path, format!("{original}\n[core]\n{repeated}")).unwrap();
    let Err(error) = worktree_settings(executable.as_os_str(), root, scratch.path()) else {
        panic!("oversized worktree settings were accepted");
    };
    assert!(
        error.to_string().contains("worktree settings exceed"),
        "{error:#}"
    );
    let error = git_changed_files(root, "HEAD").unwrap_err();
    assert!(
        error.to_string().contains("worktree settings exceed"),
        "{error:#}"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let program = scratch.path().join("fixture-git");
        let repository = canonical_platform_path(root).unwrap();
        let excludes = b"# ordinary ignore rules\n";
        fs::write(root.join("fixture-excludes"), excludes).unwrap();
        let script = r#"#!/bin/sh
case "$*" in
    *--show-scope*)
        printf 'read\n' >> "${0%/*}/queries"
        printf 'local\000core.eol\nlf\000local\000core.filemode\000local\000core.excludesfile\nfixture-excludes\000'
        ;;
    *--file*) printf 'fixture-excludes\000' ;;
    *) exit 2 ;;
esac
"#;
        fs::write(&program, script).unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
        let settings = worktree_settings(program.as_os_str(), &repository, scratch.path()).unwrap();
        let snapshot = scratch.path().join("core.excludesfile");
        assert_eq!(
            settings,
            [
                OsString::from("core.eol=lf"),
                OsString::from("core.filemode"),
                git_path_setting("core.excludesfile", &snapshot),
            ]
        );
        assert_eq!(fs::read(&snapshot).unwrap(), excludes);
        assert_eq!(fs::read(scratch.path().join("queries")).unwrap(), b"read\n");

        fs::write(root.join("selected"), "# settings\n").unwrap();
        let cases = [
            (
                r"printf 'local\000core.excludesfile\nsettings\000short\000'",
                "invalid Git worktree settings",
            ),
            (
                r"printf 'unknown\000core.excludesfile\nsettings\000'",
                "cannot verify the Git configuration scope",
            ),
            (
                r#"case "$*" in
                    *--show-scope*) printf 'global\000core.excludesfile\nselected/child\000' ;;
                    *) printf 'selected/child\000' ;;
                esac"#,
                "Git setting core.excludesfile",
            ),
        ];
        for (script, expected) in cases {
            fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
            fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
            let error =
                worktree_settings(program.as_os_str(), &repository, scratch.path()).unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }
}

/// Checks changed-file comparison reads the split index through owned snapshots.
#[test]
fn changed_files_do_not_refresh_the_original_shared_index() {
    let (directory, source) = repository_with_tracked_file();
    let root = directory.path();
    assert!(fixture_git()
        .current_dir(root)
        .args(["update-index", "--split-index"])
        .status()
        .unwrap()
        .success());
    let output = fixture_git()
        .current_dir(root)
        .args(["rev-parse", "--shared-index-path"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    let shared_index = root.join(git_output_path(&output.stdout).unwrap());
    set_index_test_timestamp(&shared_index);
    let before = fs::metadata(&shared_index).unwrap().modified().unwrap();

    fs::write(&source, "after").unwrap();
    assert_eq!(
        git_changed_files(root, "HEAD").unwrap(),
        BTreeSet::from([source])
    );
    assert_eq!(
        fs::metadata(&shared_index).unwrap().modified().unwrap(),
        before
    );

    fs::write(&shared_index, b"invalid test index").unwrap();
    let error = git_changed_files(root, "HEAD").unwrap_err();
    assert!(error
        .to_string()
        .contains("failed to inspect the private Git shared index"));
}

/// Checks only an unambiguous, valid index filename in the private Git directory is admitted.
#[test]
fn shared_index_hint_requires_a_private_path_and_matching_digest() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let name = format!("sharedindex.{}", "a".repeat(40));
    let hint = |path: &Path| {
        format!(
            "fatal: {}: index file open failed: missing\n",
            path.display()
        )
    };
    for path in [PathBuf::from(&name), root.join(&name)] {
        assert_eq!(
            shared_index_hint(hint(&path).as_bytes(), root, 40),
            Some(OsString::from(&name))
        );
    }
    for path in [
        root.join("other").join(&name),
        PathBuf::from("..").join(&name),
        root.join(format!("sharedindex.{}", "g".repeat(40))),
        root.join(format!("sharedindex.{}", "a".repeat(64))),
    ] {
        assert_eq!(shared_index_hint(hint(&path).as_bytes(), root, 40), None);
    }
    assert_eq!(
        shared_index_hint(hint(&root.join(&name)).repeat(2).as_bytes(), root, 40),
        None
    );
}

/// Checks configured Git inputs are bounded regular files relative to the worktree root.
#[test]
fn changed_files_validate_configured_git_input_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let scope = root.join("nested");
    fs::create_dir(&scope).unwrap();
    fs::write(scope.join("tracked.ts"), "before\n").unwrap();
    initialize_repository(root);
    fs::write(scope.join("ignored.ts"), "ignore this\n").unwrap();
    let visible = scope.join("visible.ts");
    fs::write(&visible, "include this\n").unwrap();
    fs::write(root.join("comparison-excludes"), "ignored.ts\n").unwrap();
    fs::write(root.join("comparison-attributes"), "*.ts text eol=lf\n").unwrap();
    fs::create_dir(root.join("comparison-directory")).unwrap();
    fs::write(
        root.join("comparison-oversized"),
        vec![b' '; 1024 * 1024 + 1],
    )
    .unwrap();

    let set = |key: &str, value: &str| {
        assert!(fixture_git()
            .current_dir(root)
            .args(["config", key, value])
            .status()
            .unwrap()
            .success());
    };
    set("core.excludesfile", "comparison-excludes");
    set("core.attributesfile", "comparison-attributes");
    assert_eq!(
        git_changed_files(&scope, "HEAD").unwrap(),
        BTreeSet::from([visible.clone()])
    );

    for absent in ["", "comparison-missing"] {
        set("core.excludesfile", absent);
        assert_eq!(
            git_changed_files(&scope, "HEAD").unwrap(),
            BTreeSet::from([scope.join("ignored.ts"), visible.clone()])
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let literal = ":(optional)comparison-literal";
        fs::write(root.join(literal), "ignored.ts\n").unwrap();
        set("core.excludesfile", &format!(":(optional){literal}"));
        assert_eq!(
            git_changed_files(&scope, "HEAD").unwrap(),
            BTreeSet::from([visible])
        );

        let inaccessible = root.join("comparison-inaccessible");
        fs::write(&inaccessible, "# normal settings\n").unwrap();
        fs::set_permissions(&inaccessible, fs::Permissions::from_mode(0o000)).unwrap();
        if File::open(&inaccessible).is_err() {
            set("core.excludesfile", "comparison-inaccessible");
            let error = git_changed_files(&scope, "HEAD").unwrap_err();
            assert!(error
                .to_string()
                .contains("failed to open Git setting core.excludesfile"));
        }
    }

    set("core.excludesfile", "comparison-excludes");
    for (key, original) in [
        ("core.excludesfile", "comparison-excludes"),
        ("core.attributesfile", "comparison-attributes"),
    ] {
        for path in ["comparison-oversized", "comparison-directory"] {
            set(key, path);
            let error = git_changed_files(&scope, "HEAD").unwrap_err();
            let message = format!("{error:#}");
            assert!(
                message.contains(key) && message.contains("regular file of at most 1 MiB"),
                "{message}"
            );
        }
        set(key, original);
    }
}

/// Checks local and worktree Git settings stay within the comparison checkout.
#[test]
fn changed_files_confine_repository_owned_git_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("checkout");
    let external = directory.path().join("operator-settings");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("tracked.ts"), "before\n").unwrap();
    initialize_repository(&root);
    fs::write(root.join("visible.ts"), "ordinary work\n").unwrap();
    fs::write(root.join("repository-settings"), "# empty settings\n").unwrap();
    fs::write(&external, "# ordinary operator settings\n").unwrap();

    let set = |scope: &str, key: &str, path: &str| {
        assert!(fixture_git()
            .current_dir(&root)
            .args(["config", scope, key, path])
            .status()
            .unwrap()
            .success());
    };
    set("--local", "extensions.worktreeConfig", "true");
    for (scope, key) in [
        ("--local", "core.excludesfile"),
        ("--worktree", "core.attributesfile"),
    ] {
        let contained = root.join("repository-settings");
        for path in ["repository-settings", contained.to_str().unwrap()] {
            set(scope, key, path);
            let files = git_changed_files(&root, "HEAD").unwrap();
            assert!(files.iter().any(|file| file.ends_with("visible.ts")));
        }

        for path in [
            external.to_str().unwrap(),
            "../operator-settings",
            ":(optional)../absent-settings",
        ] {
            set(scope, key, path);
            let error = git_changed_files(&root, "HEAD").unwrap_err();
            let message = format!("{error:#}");
            assert!(
                message.contains(key) && message.contains("outside the checkout"),
                "{message}"
            );
        }

        set(scope, key, ":(optional)absent-settings");
        let files = git_changed_files(&root, "HEAD").unwrap();
        assert!(files.iter().any(|file| file.ends_with("visible.ts")));

        #[cfg(unix)]
        {
            let alias = root.join(format!("{key}-alias"));
            std::os::unix::fs::symlink(&external, &alias).unwrap();
            set(scope, key, alias.to_str().unwrap());
            let error = git_changed_files(&root, "HEAD").unwrap_err();
            assert!(format!("{error:#}").contains("outside the checkout"));
        }

        set(scope, key, "repository-settings");
    }

    #[cfg(unix)]
    {
        let key = "core.excludesfile";
        let invalid_parent = root.join("repository-settings").join("child");
        let error = verify_repository_git_path(&root, &invalid_parent, key).unwrap_err();
        assert!(error.to_string().contains("cannot verify Git setting"));

        let error = verify_repository_git_path(&root, Path::new(""), key).unwrap_err();
        assert!(error
            .to_string()
            .contains("cannot verify the path for Git setting"));
    }
}

/// Checks Windows filesystem path admission lexically without opening the supplied paths.
#[cfg(windows)]
#[test]
fn changed_files_configured_input_requires_a_windows_filesystem_path() {
    let root = Path::new(r"C:\fixture");
    for path in [
        r"rules\attributes.txt",
        r"C:\fixture\rules.txt",
        r"\\?\C:\fixture\rules.txt",
        r"\\fixture.invalid\files\rules.txt",
        r"\\?\UNC\fixture.invalid\files\rules.txt",
        "auxiliary.txt",
        "com10.txt",
    ] {
        assert!(
            configured_git_input_source(root, Path::new(path)).is_ok(),
            "{path}"
        );
    }
    for path in [
        r"\\.\fixture",
        r"\\fixture.invalid\pipe\fixture",
        r"\\?\UNC\fixture.invalid\PIPE\fixture",
        "Aux.txt",
        "LPT².txt",
        r"C:rules.txt",
    ] {
        assert!(
            configured_git_input_source(root, Path::new(path)).is_err(),
            "{path}"
        );
    }
}

/// Checks the private Git command retains the checkout's long-path setting.
#[test]
fn changed_files_read_git_long_path_setting() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("tracked.ts"), "before").unwrap();
    initialize_repository(root);
    for value in ["true", "false"] {
        assert!(fixture_git()
            .current_dir(root)
            .args(["config", "core.longpaths", value])
            .status()
            .unwrap()
            .success());
        let git = ChangedGit::new(root).unwrap();
        let setting = checked_git_output(
            git.worktree_command(root)
                .args(["config", "--type=bool", "--get", "core.longpaths"]),
            "failed to read the comparison long-path setting",
        )
        .unwrap();
        assert_eq!(String::from_utf8(setting).unwrap().trim(), value);
    }
}

/// Checks Git can retain a compact index when comparison only touches in-cone paths.
#[test]
fn changed_files_keep_sparse_indexes_compact() {
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path();
    for path in ["inside", "outside"] {
        fs::create_dir(root.join(path)).unwrap();
        fs::write(root.join(path).join("step.ts"), "before").unwrap();
    }
    initialize_repository(root);
    let execute = |args: &[&str]| fixture_git().current_dir(root).args(args).output().unwrap();
    let help = execute(&["sparse-checkout", "init", "-h"]);
    let sparse_support = help
        .stdout
        .iter()
        .chain(&help.stderr)
        .copied()
        .collect::<Vec<_>>();
    if !String::from_utf8_lossy(&sparse_support).contains("sparse-index") {
        return;
    }

    for arguments in [
        &["sparse-checkout", "init", "--cone", "--sparse-index"][..],
        &["sparse-checkout", "set", "inside"],
    ] {
        let result = execute(arguments);
        assert!(result.status.success(), "{:?}", result.stderr);
    }
    let native = execute(&["ls-files", "--sparse"]);
    assert!(native.status.success(), "{:?}", native.stderr);
    assert_eq!(native.stdout, b"inside/step.ts\noutside/\n");
    let git = ChangedGit::new(root).unwrap();
    let private = checked_git_output(
        git.worktree_command(root).args(["ls-files", "--sparse"]),
        "failed to list the private sparse index",
    )
    .unwrap();
    assert_eq!(private, native.stdout);

    let edit = root.join("inside/step.ts");
    fs::write(&edit, "after").unwrap();
    let revision = resolve_commit(git.metadata_command(root), "HEAD").unwrap();
    let diff = git
        .worktree_command(root)
        .env("GIT_TRACE2_EVENT", "1")
        .args(["diff", "--name-only", &revision, "--"])
        .output()
        .unwrap();
    let trace = String::from_utf8_lossy(&diff.stderr);
    assert!(diff.status.success(), "{trace}");
    assert_eq!(diff.stdout, b"inside/step.ts\n");
    assert!(trace.contains(r#""event":"#), "{trace}");
    assert!(!trace.contains(r#""label":"ensure_full_index""#));
    assert_eq!(
        git_changed_files(root, "HEAD").unwrap(),
        BTreeSet::from([edit])
    );
}

/// Checks a changed related location retains a finding whose primary file is unchanged.
#[test]
fn changed_mode_keeps_findings_with_changed_related_locations() {
    let root = Path::new("/repo");
    let mut findings = vec![finding(root, "steps/old.ts")];
    let changed = BTreeSet::from([root.join("steps/shared.ts")]);
    retain_changed_findings(&mut findings, &changed);
    assert_eq!(findings.len(), 1);
}

/// Checks baseline updates remove absent active entries and reject unsupported schemas or malformed JSON.
#[test]
fn baseline_validation_counts_removals_and_ignores_suppressed_findings() {
    let directory = tempfile::tempdir().unwrap();
    let baseline_path = directory.path().join("nested/baseline.json");
    let mut initial = vec![
        finding(directory.path(), "one.ts"),
        finding(directory.path(), "two.ts"),
    ];
    initial[1].message = "second duplicate".to_owned();
    update_baseline(&mut initial, &baseline_path).unwrap();

    let mut current = vec![finding(directory.path(), "one.ts")];
    current[0].suppression = Some(Suppression {
        reason: "already accepted elsewhere".to_owned(),
    });
    let outcome = update_baseline(&mut current, &baseline_path).unwrap();
    assert_eq!(outcome.added, 0);
    assert_eq!(outcome.removed, 2);
    assert_eq!(outcome.suppressed, 0);

    fs::write(&baseline_path, r#"{"schemaVersion":2,"fingerprints":{}}"#).unwrap();
    let error = apply_baseline(&mut [], &baseline_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unsupported baseline schema version `2` (expected `3`)"));
    assert!(error.contains("regenerate it with --update-baseline"));
    fs::write(&baseline_path, r#"{"schemaVersion":99,"fingerprints":{}}"#).unwrap();
    assert!(apply_baseline(&mut [], &baseline_path)
        .unwrap_err()
        .to_string()
        .contains("unsupported baseline schema"));
    fs::write(&baseline_path, "not json").unwrap();
    assert!(apply_baseline(&mut [], &baseline_path)
        .unwrap_err()
        .to_string()
        .contains("failed to parse baseline"));
}

/// Checks comparison fingerprints ignore pair order and decreasing occurrence counts add no baseline entries.
#[test]
fn comparison_fingerprints_are_independent_of_pair_order() {
    let root = Path::new("/repo");
    let comparison = DefinitionComparison {
        left_fingerprint: "left".to_owned(),
        right_fingerprint: "right".to_owned(),
        left_matcher: String::new(),
        right_matcher: String::new(),
        left_handler: String::new(),
        right_handler: String::new(),
        matcher_diff: crate::model::MatcherDiff {
            prefix: String::new(),
            left_change: String::new(),
            right_change: String::new(),
            suffix: String::new(),
        },
    };
    let mut reversed = comparison.clone();
    std::mem::swap(
        &mut reversed.left_fingerprint,
        &mut reversed.right_fingerprint,
    );
    let mut original_finding = finding(root, "steps/original.ts");
    original_finding.evidence.comparison = Some(comparison);
    let mut reversed_finding = finding(root, "steps/reversed.ts");
    reversed_finding.evidence.comparison = Some(reversed);
    assert_eq!(
        finding_fingerprint(&original_finding),
        finding_fingerprint(&reversed_finding)
    );
    assert_eq!(
        count_added(
            &BTreeMap::from([("a".to_owned(), 3)]),
            &BTreeMap::from([("a".to_owned(), 1)])
        ),
        0
    );
}

/// Updating an oversized baseline fails before replacement and preserves its original bytes.
#[test]
fn oversized_baseline_update_preserves_the_original_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("baseline.json");
    let file = fs::File::create(&path).unwrap();
    file.set_len(crate::resource_limits::MAX_BASELINE_INPUT_BYTES + 1)
        .unwrap();
    drop(file);
    let before = fs::metadata(&path).unwrap().len();
    assert!(update_baseline(&mut [], &path).is_err());
    assert_eq!(fs::metadata(&path).unwrap().len(), before);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

/// Changed mode either returns both listings or fails at the combined record or byte boundary.
#[test]
fn source_enumeration_git_shares_records_and_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let tracked = root.join("tracked.ts");
    let untracked = root.join("new file.ts");
    fs::write(&tracked, "initial").unwrap();
    initialize_repository(&root);
    fs::write(&tracked, "changed").unwrap();
    fs::write(&untracked, "new").unwrap();
    let bytes = tracked.as_os_str().len() + untracked.as_os_str().len() + 2;
    for (records, bytes, accepted) in [(2, bytes, true), (1, bytes, false), (2, bytes - 1, false)] {
        let result = git_changed_files_with_limits(&root, "HEAD", records, bytes);
        if accepted {
            assert_eq!(
                result.unwrap(),
                BTreeSet::from([tracked.clone(), untracked.clone()])
            );
        } else {
            let message = result.unwrap_err().to_string();
            assert_eq!(
                message,
                "Git changed-path listing exceeds the record or byte limit"
            );
        }
    }
}

/// Only empty and fully terminated relative paths are valid native Git listings.
#[test]
fn source_enumeration_git_rejects_malformed_listing() {
    for (data, expected) in [
        (b"".as_slice(), Ok(Vec::new())),
        (
            b"filename\n.ts\0".as_slice(),
            Ok(vec![PathBuf::from("filename\n.ts")]),
        ),
        (b"unfinished".as_slice(), Err("invalid Git path listing")),
        (
            b"\0".as_slice(),
            Err("invalid relative path in Git listing"),
        ),
        (
            b"../other\0".as_slice(),
            Err("invalid relative path in Git listing"),
        ),
    ] {
        let mut budget = ChangedPathBudget {
            records: 2,
            bytes: 256,
        };
        let result = read_git_paths(std::io::Cursor::new(data), Path::new(""), &mut budget);
        assert_eq!(
            result.map_err(|error| error.to_string()),
            expected.map_err(str::to_owned)
        );
    }
}

/// The update counts the trailing newline, preserves old bytes and does not suppress on refusal.
#[test]
fn retained_output_baseline_is_replaced_only_at_or_under_the_write_ceiling() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("baseline.json");
    let initial = br#"{"schemaVersion":3,"fingerprints":{}}"#;
    let mut offered = [finding(root.path(), "steps/current.ts")];
    update_baseline(&mut offered, &path).unwrap();
    let expected = fs::read(&path).unwrap();
    assert_eq!(expected.last(), Some(&b'\n'));

    for existing in [false, true] {
        if existing {
            fs::write(&path, initial).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        let mut offered = [finding(root.path(), "steps/current.ts")];
        let error =
            update_baseline_with_limit(&mut offered, &path, expected.len() - 1, None).unwrap_err();
        assert!(format!("{error:#}").contains("byte limit exceeded"));
        assert!(offered[0].is_active());
        if existing {
            assert_eq!(fs::read(&path).unwrap(), initial);
        } else {
            assert!(!path.exists());
        }
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            usize::from(existing)
        );

        let outcome =
            update_baseline_with_limit(&mut offered, &path, expected.len(), None).unwrap();
        assert_eq!(fs::read(&path).unwrap(), expected);
        assert_eq!(outcome.suppressed, 1);
    }
}
