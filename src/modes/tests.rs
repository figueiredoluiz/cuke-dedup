use super::*;
use crate::model::{
    DefinitionCluster, DefinitionComparison, FindingEvidence, Rule, Severity, SourceLocation,
};

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
    for variable in INHERITED_GIT_VARIABLES {
        command.env_remove(variable);
    }
    command
}

fn finding(root: &Path, file: &str) -> Finding {
    Finding {
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

#[cfg(unix)]
#[test]
fn git_path_decoder_preserves_non_utf8_unix_bytes() {
    use std::os::unix::ffi::OsStrExt;

    let bytes = b"steps/non-utf8-\xff.ts";
    let path = path_from_git_bytes(bytes).unwrap();
    assert_eq!(path.as_os_str().as_bytes(), bytes);
}

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

#[test]
fn changed_mode_validates_revisions_repositories_and_ignored_roots() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("tracked.ts"), "initial").unwrap();
    initialize_repository(directory.path());
    assert!(ensure_changed_root_is_trackable(&directory.path().canonicalize().unwrap()).is_ok());
    for revision in ["", " ", "--all", "missing-revision"] {
        assert!(
            resolve_commit(directory.path(), revision).is_err(),
            "{revision}"
        );
    }
    assert!(git_paths(directory.path(), &["not-a-command"], "expected failure").is_err());

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

#[test]
fn changed_mode_keeps_findings_with_changed_related_locations() {
    let root = Path::new("/repo");
    let mut findings = vec![finding(root, "steps/old.ts")];
    let changed = BTreeSet::from([root.join("steps/shared.ts")]);
    retain_changed_findings(&mut findings, &changed);
    assert_eq!(findings.len(), 1);
}

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
