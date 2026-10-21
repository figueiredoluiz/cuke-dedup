use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::process::Command as ProcessCommand;

/// Git exports `GIT_DIR`, `GIT_INDEX_FILE`, and friends to the processes it spawns. When this
/// suite runs from a Git hook, those variables outrank the `current_dir` and `-C` of a nested
/// invocation, so a fixture's `git add` would stage temporary paths into the real repository's
/// index and mark every tracked file deleted. Strip that environment instead.
fn fixture_git() -> ProcessCommand {
    let mut command = ProcessCommand::new("git");
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_CEILING_DIRECTORIES",
    ] {
        command.env_remove(variable);
    }
    command
}

/// Initializes a Git repository at `root` with a stable identity and a single initial commit.
fn init_repository(root: &Path) {
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
        vec!["add", "."],
        vec!["commit", "-qm", "initial"],
    ] {
        assert!(fixture_git()
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success());
    }
}

/// Writes a fixture file beneath the root, creating its parent directories.
fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Checks historical comparison uses current policy, gates new findings, and preserves checkout and Git state.
#[test]
fn baseline_from_ref_compares_history_without_mutating_the_checkout() {
    let sandbox = tempfile::tempdir().unwrap();
    // Trailing spaces are valid on Unix and must not be trimmed from Git's output.
    #[cfg(not(target_os = "linux"))]
    let repository_name = if cfg!(unix) {
        " repository "
    } else {
        "repository"
    };
    // Linux also permits non-UTF-8 bytes in repository roots.
    #[cfg(target_os = "linux")]
    let repository_name = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(b" repository-\xff ")
    };
    let root = sandbox.path().join(repository_name);
    fs::create_dir(&root).unwrap();
    let git = |args: &[&str]| {
        let output = fixture_git()
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "Test"]);
    let scope = root.join("packages/suite");
    write(&scope, "README.md", "Synthetic suite\n");
    git(&["add", "."]);
    git(&["commit", "-qm", "empty suite"]);
    git(&["tag", "empty-suite"]);
    write(
        &scope,
        "steps.ts",
        "Given('shared', () => first());\nGiven('shared', () => second());\n",
    );
    write(
        &scope,
        "example.feature",
        "Feature: Example\n  Scenario: Example\n    Given shared\n",
    );
    // The old policy disables all rules. The current explicit overrides must govern BOTH scans.
    let rules: serde_json::Map<String, Value> = cuke_dedup::model::Rule::ALL
        .iter()
        .map(|rule| (rule.to_string(), Value::from("off")))
        .collect();
    write(
        &scope,
        ".cuke-dedup.json",
        &serde_json::json!({"rules": rules}).to_string(),
    );
    write(&root, ".gitattributes", "*.ts filter=unsafe\n");
    git(&["add", "."]);
    git(&["commit", "-qm", "base"]);
    // These would break checkout if the snapshot inherited repository config or hooks.
    write(&root, "hooks/post-checkout", "#!/bin/sh\nexit 99\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            root.join("hooks/post-checkout"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    git(&["config", "core.hooksPath", "hooks"]);
    git(&["config", "filter.unsafe.required", "true"]);
    git(&["config", "filter.unsafe.smudge", "nonexistent-cuke-filter"]);
    let before_head = git(&["rev-parse", "HEAD"]);
    let before_index = git(&["ls-files", "--stage"]);
    let before_worktrees = git(&["worktree", "list", "--porcelain"]);
    fs::rename(scope.join("steps.ts"), scope.join("renamed café.ts")).unwrap();
    let run = |allowance: &str, expected: i32| {
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(&scope)
            .env("GIT_DIR", sandbox.path().join("not-a-repository"))
            .env("GIT_CONFIG_GLOBAL", root.join(".git/config"))
            .env("git_config_count", "2")
            .env("gIt_cOnFiG_kEy_0", "filter.unsafe.required")
            .env("git_config_value_0", "true")
            .env("git_config_key_1", "filter.unsafe.smudge")
            .env("gIt_cOnFiG_vAlUe_1", "nonexistent-cuke-filter")
            .args([
                ".",
                "--baseline-from-ref",
                "HEAD",
                "--fail-on-new",
                allowance,
                "--rule",
                "duplicate-matcher=warning",
                "--reporters",
                "json",
                "--output",
            ])
            .arg(sandbox.path().join("reports"))
            .assert()
            .code(expected);
    };
    run("0", 0);
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(&scope)
        .args([
            ".",
            "--baseline-from-ref",
            "empty-suite",
            "--fail-on-new=0",
            "--rule",
            "duplicate-matcher=warning",
            "--output",
        ])
        .arg(sandbox.path().join("empty-report"))
        .assert()
        .code(1);
    write(&scope, "third.ts", "Given('shared', () => third());\n");
    run("0", 1);
    run("1", 0);
    let report: Value =
        serde_json::from_slice(&fs::read(sandbox.path().join("reports/cuke-dedup.json")).unwrap())
            .unwrap();
    let findings = report["findings"].as_array().unwrap();
    assert_eq!(
        findings
            .iter()
            .filter(|finding| finding["suppression"].is_null())
            .count(),
        1
    );
    assert_eq!(
        findings
            .iter()
            .filter(|finding| !finding["suppression"].is_null())
            .count(),
        1
    );
    // A current-source parse failure is tolerated by default, but `--fail-on-unparseable` keeps it
    // fatal even under a generous new-finding allowance.
    write(&scope, "broken.ts", "Given('bad', () => {");
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(&scope)
        .args([
            ".",
            "--baseline-from-ref",
            "HEAD",
            "--fail-on-new",
            "99",
            "--fail-on-unparseable",
            "--rule",
            "duplicate-matcher=warning",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "source contains JavaScript/TypeScript syntax errors",
        ));
    fs::remove_file(scope.join("broken.ts")).unwrap();
    assert_eq!(git(&["rev-parse", "HEAD"]), before_head);
    assert_eq!(git(&["ls-files", "--stage"]), before_index);
    assert_eq!(git(&["worktree", "list", "--porcelain"]), before_worktrees);
    assert!(scope.join("renamed café.ts").exists());
    assert!(scope.join("third.ts").exists());
    assert!(!scope.join(".cuke-dedup-baseline.json").exists());
    let report: Value =
        serde_json::from_slice(&fs::read(sandbox.path().join("reports/cuke-dedup.json")).unwrap())
            .unwrap();
    assert!(report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .all(|finding| {
            !finding["primary"]["path"]
                .as_str()
                .unwrap_or("")
                .contains("checkout")
        }));
    fs::create_dir(scope.join("new-directory")).unwrap();
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(scope.join("new-directory"))
        .args([".", "--baseline-from-ref", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "does not contain the analysis directory",
        ));
    for (args, message) in [
        (vec!["--baseline-from-ref", "missing-ref"], "baseline"),
        (vec!["--baseline-from-ref=--help"], "baseline"),
        (
            vec!["--baseline-from-ref", "HEAD", "--baseline", "baseline.json"],
            "cannot be used",
        ),
        (
            vec!["--baseline-from-ref", "HEAD", "--update-baseline"],
            "cannot be used",
        ),
        (vec!["--fail-on-new"], "required"),
    ] {
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(&scope)
            .arg(".")
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains(message));
    }
}

/// Checks strict historical comparison rejects an incomplete base despite a valid current corpus.
#[test]
fn baseline_from_ref_rejects_incomplete_history_even_when_current_files_are_valid() {
    let valid_feature = "Feature: Example\n  Scenario: Example\n    Given shared\n";
    for (bad_source, feature) in [
        ("Given('shared', () => {", Some(valid_feature)),
        (
            "import { Given } from './missing'; Given('shared', () => work());",
            Some(valid_feature),
        ),
        ("const helper = 42;", Some(valid_feature)),
        ("Given('shared', () => work());", None),
        ("Given('shared', () => work());", Some("invalid Gherkin")),
    ] {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "steps.ts", bad_source);
        if let Some(feature) = feature {
            write(root.path(), "example.feature", feature);
        }
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "base",
            ],
        ] {
            assert!(fixture_git()
                .current_dir(root.path())
                .args(args)
                .output()
                .unwrap()
                .status
                .success());
        }
        write(root.path(), "steps.ts", "Given('shared', () => work());");
        write(root.path(), "example.feature", valid_feature);
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(root.path())
            .args([
                ".",
                "--baseline-from-ref",
                "HEAD",
                "--fail-on-new=0",
                "--fail-on-incomplete",
            ])
            .assert()
            .code(2)
            .stderr(
                predicate::str::contains("baseline").and(predicate::str::contains("incomplete")),
            );
    }
}

/// Checks incomplete historical extraction warns by default and fails when completeness is required.
#[test]
fn baseline_from_ref_tolerates_incomplete_history_by_default_and_reports_it() {
    let valid_feature = "Feature: Example\n  Scenario: Example\n    Given shared\n";
    let root = tempfile::tempdir().unwrap();
    // Base revision: a real definition plus an unparseable sibling. The baseline still extracts the
    // real definition, so it can be subtracted; the parse failure only makes it incomplete, which
    // is the tolerated case (distinct from the hard failures the rejection test above pins, where
    // the base extracts no definitions at all).
    write(root.path(), "steps.ts", "Given('shared', () => work());");
    write(root.path(), "broken.ts", "Given('broken', () => {");
    write(root.path(), "example.feature", valid_feature);
    init_repository(root.path());
    // Working tree: the unparseable sibling is gone, so the current corpus is complete.
    fs::remove_file(root.path().join("broken.ts")).unwrap();

    // Default: the incomplete baseline is tolerated and the comparison proceeds, with a warning
    // that a finding the baseline could not extract may surface as new.
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root.path())
        .args([".", "--baseline-from-ref", "HEAD"])
        .assert()
        .success()
        .stderr(
            predicate::str::contains("baseline revision `HEAD` is incomplete")
                .and(predicate::str::contains("--fail-on-incomplete")),
        );

    // `--fail-on-incomplete` restores the rejection.
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root.path())
        .args([".", "--baseline-from-ref", "HEAD", "--fail-on-incomplete"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("incomplete"));
}

/// Checks strict comparison rejects truncated history and snapshot submodules regardless of Git replacements.
#[test]
fn baseline_from_ref_rejects_truncated_base_and_unmaterialized_submodules() {
    let root = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = fixture_git()
            .current_dir(root.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    git(&["init", "-q"]);
    let source: String = (0..10)
        .map(|index| format!("Given('operation label {index}', () => perform({index}));\n"))
        .collect();
    write(root.path(), "steps.ts", &source);
    write(
        root.path(),
        "example.feature",
        "Feature: Example\n  Scenario: Example\n    Given operation label 0\n",
    );
    git(&["add", "."]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "base",
    ]);
    write(
        root.path(),
        "steps.ts",
        "Given('operation label 0', () => perform(0));",
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root.path())
        .args([
            ".",
            "--baseline-from-ref",
            "HEAD",
            "--max-structural-class-comparisons",
            "2",
            "--fail-on-incomplete",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "baseline revision `HEAD` is incomplete",
        ));
    let oid = git(&["rev-parse", "HEAD"]);
    git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        "160000",
        oid.trim(),
        "vendor",
    ]);
    git(&[
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "commit",
        "-qm",
        "gitlink",
    ]);
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root.path())
        .args([".", "--baseline-from-ref", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unsupported submodules"));
    let gitlink = git(&["rev-parse", "HEAD"]);
    let gitlink_tree = git(&["rev-parse", "HEAD^{tree}"]);
    let clean_tree = git(&["rev-parse", &format!("{}^{{tree}}", oid.trim())]);
    for (original, replacement) in [
        (gitlink.trim(), oid.trim()),
        (gitlink_tree.trim(), clean_tree.trim()),
    ] {
        // A replacement must not hide a real submodule from the snapshot pre-check.
        git(&["replace", original, replacement]);
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(root.path())
            .args([".", "--baseline-from-ref", gitlink.trim()])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unsupported submodules"));
        git(&["replace", "-d", original]);
        // Conversely, a replacement must not inject a submodule into a clean base.
        git(&["replace", replacement, original]);
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(root.path())
            .args([".", "--baseline-from-ref", oid.trim()])
            .assert()
            .code(0);
        git(&["replace", "-d", replacement]);
    }
}

/// Checks snapshot metadata limits, concurrent stderr draining, environment isolation, and Git child cleanup.
#[cfg(unix)]
#[test]
fn baseline_tree_output_is_bounded_before_git_exit_and_children_are_reaped() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path();
    let real_git = ProcessCommand::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(real_git.status.success());
    let real_git = String::from_utf8(real_git.stdout).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "base",
        ],
    ] {
        assert!(fixture_git()
            .current_dir(root)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
    write(
        root,
        "shim/git",
        r#"#!/bin/sh
# Also exercise Windows-style case variants on case-sensitive Unix hosts.
if [ -n "${git_config_count-}${gIt_cOnFiG_kEy_0-}" ]; then exit 99; fi
for argument do
    if [ "$argument" = ls-tree ]; then
        echo "$$" > "$BASELINE_TEST_PID"
        cat "$BASELINE_TEST_STDERR" >&2
        cat "$BASELINE_TEST_TREE"
        exit "$BASELINE_TEST_EXIT"
    fi
done
exec "$BASELINE_TEST_REAL_GIT" "$@"
"#,
    );
    fs::set_permissions(root.join("shim/git"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut paths = vec![root.join("shim")];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let path = std::env::join_paths(paths).unwrap();
    let limit = 28 * 100_000;
    for (name, listing, status, diagnostic) in [
        ("empty", String::new(), "0", ""),
        ("file-limit", "100644 0\n".repeat(100_000), "0", ""),
        (
            "too-many-files",
            "100644 0\n".repeat(100_001),
            "0",
            "snapshot limit",
        ),
        (
            "byte-limit",
            "x".repeat(limit),
            "7",
            "baseline Git tree enumeration failed",
        ),
        (
            "overflow",
            "x".repeat(limit + 1),
            "7",
            "bounded snapshot metadata limit",
        ),
        (
            "large-overflow",
            "100644 0\n".repeat(400_000),
            "7",
            "bounded snapshot metadata limit",
        ),
        (
            "size-limit",
            "100644 536870913\n".into(),
            "0",
            "snapshot limit",
        ),
        (
            "submodule",
            "160000 -\n".into(),
            "0",
            "unsupported submodules",
        ),
    ] {
        write(root, "tree-output", &listing);
        // More than a pipe buffer: stderr must not deadlock stdout consumption.
        write(root, "stderr-output", &"diagnostic\n".repeat(10_000));
        let result = Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(root)
            .args([".", "--baseline-from-ref", "HEAD"])
            .env("PATH", &path)
            .env("BASELINE_TEST_REAL_GIT", real_git.trim())
            .env("BASELINE_TEST_PID", root.join("git-pid"))
            .env("BASELINE_TEST_TREE", root.join("tree-output"))
            .env("BASELINE_TEST_STDERR", root.join("stderr-output"))
            .env("BASELINE_TEST_EXIT", status)
            .env("git_config_count", "1")
            .env("gIt_cOnFiG_kEy_0", "filter.unused.smudge")
            .env("git_config_value_0", "nonexistent-cuke-filter")
            .timeout(std::time::Duration::from_secs(10))
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(if diagnostic.is_empty() { 0 } else { 2 }),
            "{name}"
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(diagnostic),
            "{name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let pid = fs::read_to_string(root.join("git-pid")).unwrap();
        assert!(
            !ProcessCommand::new("kill")
                .args(["-0", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success(),
            "unreaped Git: {name}"
        );
    }
}

/// Writes a file of `bytes` length whose content is ordinary authored text.
///
/// Size is the property under test, so the filler has to be something the analyzer would
/// otherwise accept: real UTF-8, no NUL bytes, and authored line geometry. A sparse file of NUL
/// bytes would instead be excluded as binary content and never reach the size limit at all.
fn write_sized(root: &Path, relative: &str, bytes: u64) {
    use std::io::Write;

    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .unwrap();
    let mut writer = std::io::BufWriter::new(file);
    let line = b"// filler line of authored text\n";
    let mut written = 0_u64;
    while written + line.len() as u64 <= bytes {
        writer.write_all(line).unwrap();
        written += line.len() as u64;
    }
    while written < bytes {
        writer.write_all(b"/").unwrap();
        written += 1;
    }
    writer.flush().unwrap();
}

/// Recursively copies a fixture directory into an isolated destination.
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

/// Checks copied corpus cases match manifest exit codes, diagnostics, rule counts, and duplication summaries.
#[test]
fn fixture_corpus_matches_the_versioned_manifest() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/corpus");
    let manifest: Value =
        serde_json::from_str(&fs::read_to_string(corpus.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["schemaVersion"], 1);

    for case in manifest["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let source_root = corpus.join(case["path"].as_str().unwrap());
        let sandbox = tempfile::tempdir().unwrap();
        let case_root = sandbox.path().join("case");
        copy_tree(&source_root, &case_root);
        if let Some(entries) = case["materialize"].as_array() {
            for entry in entries {
                let source = case_root.join(entry["source"].as_str().unwrap());
                let destination = case_root.join(entry["destination"].as_str().unwrap());
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                fs::copy(source, destination).unwrap();
            }
        }
        let report_directory = tempfile::tempdir().unwrap();
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command.current_dir(&case_root).arg(".");
        for argument in case["arguments"].as_array().unwrap() {
            command.arg(argument.as_str().unwrap());
        }
        let mut assertion = command
            .args(["--reporters", "json", "--output"])
            .arg(report_directory.path())
            .assert()
            .code(case["expectedExit"].as_i64().unwrap() as i32);
        if let Some(messages) = case["stderrIncludes"].as_array() {
            for message in messages {
                assertion = assertion.stderr(predicate::str::contains(message.as_str().unwrap()));
            }
        }

        let report: Value = serde_json::from_str(
            &fs::read_to_string(report_directory.path().join("cuke-dedup.json")).unwrap(),
        )
        .unwrap();
        let expected = &case["expected"];
        let summary = &report["summary"];
        let duplication = &summary["duplication"];
        assert_eq!(
            summary["definitionsAnalyzed"], expected["definitions"],
            "{name}: definition count"
        );
        assert_eq!(
            summary["featureStepsAnalyzed"], expected["featureSteps"],
            "{name}: feature-step count"
        );
        assert_eq!(
            duplication["duplicatedDefinitions"], expected["duplicatedDefinitions"],
            "{name}: duplicated definition count"
        );
        assert_eq!(
            duplication["percentage"], expected["percentage"],
            "{name}: duplication percentage"
        );
        assert_eq!(
            duplication["threshold"], expected["threshold"],
            "{name}: threshold"
        );
        assert_eq!(
            duplication["passed"], expected["passed"],
            "{name}: threshold result"
        );
        assert_eq!(summary["byRule"], expected["byRule"], "{name}: rules");
    }
}

/// Checks a passing duplication threshold still fails on an ambiguous feature step.
#[test]
fn threshold_does_not_tolerate_non_duplication_errors() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('same step', () => first());\nGiven('same step', () => second());\n",
    );
    write(
        directory.path(),
        "example.feature",
        "Feature: Ambiguous\n  Scenario: One\n    Given same step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--threshold", "100"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("threshold 100.00% — PASS"))
        .stdout(predicate::str::contains("ambiguous-step"));
}

/// Checks non-finite and out-of-range thresholds return an actionable configuration error.
#[test]
fn invalid_thresholds_are_configuration_errors() {
    let directory = tempfile::tempdir().unwrap();
    for threshold in ["-0.01", "100.01", "NaN", "inf"] {
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command
            .current_dir(directory.path())
            .args([".", "--threshold", threshold])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "threshold must be a finite percentage from 0 through 100",
            ));
    }
}

/// Checks an invalid exclusion glob returns exit code 2 with the offending pattern.
#[test]
fn invalid_exclusion_patterns_are_operational_errors() {
    let directory = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--exclude", "["])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid exclude glob pattern `[`"));
}

/// Checks valid inline suppression permits success and a missing reason produces an operational error.
#[test]
fn inline_suppressions_require_a_rule_and_reason_and_affect_exit_status() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "// cuke-dedup:ignore duplicate-matcher -- intentionally separate setup\nGiven('same', () => first());\nGiven('same', () => second());\n",
    );
    let mut valid = Command::cargo_bin("cuke-dedup").unwrap();
    valid
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 suppressed"));

    write(
        directory.path(),
        "broken.ts",
        "// cuke-dedup:ignore duplicate-handler\nGiven('broken', () => work());\n",
    );
    let mut malformed = Command::cargo_bin("cuke-dedup").unwrap();
    malformed
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cuke-dedup:ignore RULE -- REASON"));
}

/// Checks printed configuration combines project settings with CLI overrides without discovery diagnostics.
#[test]
fn print_config_reports_merged_values_without_running_discovery() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"threshold":4,"exclude":["generated/**"],"reporters":["json"],"maxCandidateComparisons":0,"maxStructuralClassComparisons":0}"#,
    );

    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([
            ".",
            "--print-config",
            "--threshold",
            "7",
            "--require-definitions",
            "--max-candidate-comparisons",
            "42",
            "--max-structural-class-comparisons",
            "7",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let config: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(config["threshold"], 7.0);
    assert_eq!(config["configSource"], ".cuke-dedup.json");
    assert_eq!(config["reporters"], serde_json::json!(["json"]));
    assert_eq!(config["requireDefinitions"], true);
    assert_eq!(config["maxCandidateComparisons"], 42);
    assert_eq!(config["maxStructuralClassComparisons"], 7);
    assert!(config["exclude"]
        .as_array()
        .unwrap()
        .iter()
        .any(|pattern| pattern == "generated/**"));
}

/// Checks configuration and CLI candidate limits reject values above the ceilings and accept the ceilings.
#[test]
fn candidate_limits_cannot_exceed_hard_safety_ceilings_from_config_or_cli() {
    let directory = tempfile::tempdir().unwrap();
    let config_path = directory.path().join(".cuke-dedup.json");

    for (config, expected) in [
        (
            r#"{"maxCandidateComparisons":2000001}"#,
            "maxCandidateComparisons must not exceed the hard safety limit of 2000000",
        ),
        (
            r#"{"maxStructuralClassComparisons":250001}"#,
            "maxStructuralClassComparisons must not exceed the hard safety limit of 250000",
        ),
    ] {
        fs::write(&config_path, config).unwrap();
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(directory.path())
            .args([".", "--print-config"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(expected));
    }

    fs::write(&config_path, "{}").unwrap();
    for (flag, value, expected) in [
        (
            "--max-candidate-comparisons",
            "2000001",
            "maxCandidateComparisons must not exceed the hard safety limit of 2000000",
        ),
        (
            "--max-structural-class-comparisons",
            "250001",
            "maxStructuralClassComparisons must not exceed the hard safety limit of 250000",
        ),
    ] {
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(directory.path())
            .args([".", "--print-config", flag, value])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(expected));
    }

    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([
            ".",
            "--print-config",
            "--max-candidate-comparisons",
            "2000000",
            "--max-structural-class-comparisons",
            "250000",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let config: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(config["maxCandidateComparisons"], 2_000_000);
    assert_eq!(config["maxStructuralClassComparisons"], 250_000);
}

/// Checks implicit analysis reports duplicate and ambiguous steps with consistent machine-report census data.
#[test]
fn zero_config_implicit_check_reports_duplicates_and_writes_machine_reports() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "features/steps/checkout.ts",
        r#"
import { Then } from '@cucumber/cucumber';
Then('the receipt is visible', async () => { await expect(receipt).toBeVisible(); });
Then('the receipt is visible', async () => { await expect(otherReceipt).toBeVisible(); });
"#,
    );
    write(
        directory.path(),
        "features/checkout.feature",
        "Feature: Checkout\n  Scenario: Receipt\n    Then the receipt is visible\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg(".")
        .args(["--reporters", "terminal,json,html,sarif"])
        .args(["--output", "artifacts"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("duplicate-matcher"))
        .stdout(predicate::str::contains(
            "Analyzed 2 definitions and 1 feature step",
        ));

    let json_path = directory.path().join("artifacts/cuke-dedup.json");
    let report: Value = serde_json::from_str(&fs::read_to_string(json_path).unwrap()).unwrap();
    assert_eq!(report["schemaVersion"], "3");
    assert_eq!(report["summary"]["errors"], 2); // duplicate matcher + ambiguity
    assert_eq!(report["metrics"]["definitionFiles"], 1);
    assert_eq!(report["corpus"]["definitionFiles"], 1);
    assert_eq!(report["corpus"]["definitionFilesWithDefinitions"], 1);
    assert_eq!(report["corpus"]["definitionsExtracted"], 2);
    assert_eq!(report["metrics"]["featureFiles"], 1);
    assert_eq!(report["corpus"]["featureFiles"], 1);
    assert_eq!(report["corpus"]["featureFilesParsed"], 1);
    assert_eq!(report["metrics"]["filesDiscovered"], 2);
    for phase in ["discoveryMs", "parsingMs", "analysisMs"] {
        assert!(
            report["metrics"][phase]
                .as_f64()
                .is_some_and(|value| value >= 0.0),
            "missing non-negative {phase}"
        );
    }
    assert!(directory.path().join("artifacts/cuke-dedup.html").is_file());
    let html = fs::read_to_string(directory.path().join("artifacts/cuke-dedup.html")).unwrap();
    assert!(html.contains("\"definitionFilesWithDefinitions\":1"));
    let sarif: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("artifacts/cuke-dedup.sarif")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["properties"]["corpus"]
            ["definitionFilesWithDefinitions"],
        1
    );
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        true
    );
}

/// Checks JSON and JSONL omit metrics while retaining definition census data.
#[test]
fn no_metrics_produces_reproducible_machine_report_content() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('one step', () => work());\n",
    );
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--reporters", "json", "--no-metrics"])
        .assert()
        .success();
    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert!(report.get("metrics").is_none());
    assert_eq!(report["corpus"]["definitionFiles"], 1);
    assert_eq!(report["corpus"]["definitionFilesWithDefinitions"], 1);
    assert_eq!(report["corpus"]["definitionsExtracted"], 1);

    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--reporters", "jsonl", "--no-metrics"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let summary: Value = serde_json::from_slice(
        output
            .stdout
            .split(|byte| *byte == b'\n')
            .find(|line| !line.is_empty())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary["type"], "summary");
    assert!(summary.get("metrics").is_none());
    assert_eq!(summary["corpus"]["definitionsExtracted"], 1);
}

/// Checks project configuration disables JSON metrics without removing the corpus census.
#[test]
fn project_config_can_disable_metrics() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"reporters":["json"],"noMetrics":true}"#,
    );
    write(directory.path(), "steps.ts", "Given('one', () => work());");
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .success();
    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert!(report.get("metrics").is_none());
    assert_eq!(report["corpus"]["definitionsExtracted"], 1);
}

/// Runs `root` without and with `flag FILE`, asserts stdout, stderr, exit status and the JSON
/// report are byte-identical, and returns the emitted JSON lines; a missing file panics.
fn emitted_records(root: &Path, args: &[&str], flag: &str) -> Vec<Value> {
    let sink = tempfile::tempdir().unwrap();
    let destination = sink.path().join("records.jsonl");
    let run = |destination: Option<&Path>| {
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command
            .arg(root)
            .args(args)
            .args(["--reporters", "terminal,json", "--no-metrics"]);
        if let Some(destination) = destination {
            command.arg(flag).arg(destination);
        }
        let output = command.output().unwrap();
        (
            output,
            fs::read(root.join("reports/cuke-dedup/cuke-dedup.json")).unwrap(),
        )
    };
    assert_eq!(run(None), run(Some(&destination)));
    fs::read_to_string(&destination)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Runs the built binary on `root` with `args` and returns its output.
fn run_on(root: &Path, args: &[&str]) -> std::process::Output {
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

/// Returns the records `--emit-fingerprints` wrote for `root`.
fn emitted_fingerprints(root: &Path, args: &[&str]) -> Vec<Value> {
    emitted_records(root, args, "--emit-fingerprints")
}

/// Projects each record to path, line, domain, framework, comparable and trivial.
fn fingerprint_outcomes(records: &[Value]) -> Vec<Value> {
    records
        .iter()
        .map(|record| {
            serde_json::json!([
                record["path"],
                record["line"],
                record["domain"],
                record["framework"],
                record["handler"]["comparable"],
                record["handler"]["trivial"]
            ])
        })
        .collect()
}

/// Checks the fingerprint oracle gives one record per JS/TS definition outcome (comparable,
/// trivial, uncomparable, parse-recovered, unknown framework), none for an excluded generated
/// source, and an empty file for an empty corpus, without changing any other output.
#[test]
fn emit_fingerprints_records_each_ecmascript_definition_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for (path, source) in [
        ("features/steps.ts", "import { Given, Then } from '@cucumber/cucumber';\nGiven('comparable', () => { work(); });\nGiven('pending', () => 'pending');\nThen('uncertain', ({state}) => register(() => expect(state).toBe(external)));\n"),
        ("features/broken.ts", "import { Given } from '@cucumber/cucumber';\nGiven('recovered', () => { work(); });\nconst = ;\n"),
        ("features/plain.js", "Given('unknown framework', function () { other(); });\n"),
        ("features/bundle.js", "Given('excluded', () => work());\0\n"),
        ("features/a.feature", "Feature: A\n  Scenario: A\n    Given comparable\n"),
    ] {
        write(root, path, source);
    }
    let ecmascript = |path, line, framework, comparable, trivial| {
        serde_json::json!([path, line, "ecmascript", framework, comparable, trivial])
    };
    assert_eq!(
        fingerprint_outcomes(&emitted_fingerprints(root, &[])),
        [
            ecmascript("features/broken.ts", 2, "cucumber-js", true, false),
            ecmascript("features/plain.js", 1, "unknown", true, false),
            ecmascript("features/steps.ts", 2, "cucumber-js", true, false),
            ecmascript("features/steps.ts", 3, "cucumber-js", true, true),
            ecmascript("features/steps.ts", 4, "cucumber-js", false, false),
        ]
    );
    let empty = tempfile::tempdir().unwrap();
    write(empty.path(), "a.feature", "Feature: A\n");
    assert!(emitted_fingerprints(empty.path(), &[]).is_empty());
}

/// Checks the fingerprint oracle gives one record per Ruby definition outcome (comparable, pending
/// stub, uncomparable optional block parameter, parse-recovered) and none for an unselected file.
#[test]
fn emit_fingerprints_records_each_ruby_definition_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for (path, source) in [
        ("features/step_definitions/steps.rb", "Given('comparable') do\n  work(1)\nend\nGiven('pending') do\n  pending\nend\nGiven('uncertain') do |value = 1|\n  page.perform(value)\nend\n"),
        ("features/step_definitions/broken.rb", "Given('recovered') do\n  work(1)\nend end\n"),
        ("support/unselected.rb", "Given('unselected') do\n  work(1)\nend\n"),
        ("features/a.feature", "Feature: A\n  Scenario: A\n    Given comparable\n"),
    ] {
        write(root, path, source);
    }
    let ruby = |path, line, comparable, trivial| {
        serde_json::json!([path, line, "ruby", "cucumber-ruby", comparable, trivial])
    };
    assert_eq!(
        fingerprint_outcomes(&emitted_fingerprints(
            root,
            &["--definitions", "features/**/*.rb"]
        )),
        [
            ruby("features/step_definitions/broken.rb", 1, true, false),
            ruby("features/step_definitions/steps.rb", 1, true, false),
            ruby("features/step_definitions/steps.rb", 4, true, true),
            ruby("features/step_definitions/steps.rb", 7, false, false),
        ]
    );
}

/// Names the record fields, handler fields as `handler.<field>`, whose values differ.
fn differing_fields(left: &Value, right: &Value) -> Vec<String> {
    let flatten = |record: &Value| -> Vec<(String, Value)> {
        record
            .as_object()
            .unwrap()
            .iter()
            .flat_map(|(key, value)| match value.as_object() {
                Some(handler) => handler
                    .iter()
                    .map(|(field, value)| (format!("{key}.{field}"), value.clone()))
                    .collect(),
                None => vec![(key.clone(), value.clone())],
            })
            .collect()
    };
    flatten(left)
        .into_iter()
        .zip(flatten(right))
        .filter(|(left, right)| left != right)
        .map(|(left, _)| left.0)
        .collect()
}

/// Asserts each `(left, right, fields)` pair of records differs in `line` plus exactly `fields`.
fn assert_isolated(records: &[Value], pairs: &[(usize, usize, String)]) {
    for (left, right, fields) in pairs {
        let actual = differing_fields(&records[*left], &records[*right]).join(" ");
        assert_eq!(
            actual,
            format!("line {fields}").trim_end(),
            "{left}:{right}"
        );
    }
}

/// Checks each handler pair that differs in one dimension yields records differing in exactly the
/// fields that dimension feeds, and identical handlers under one matcher differ only by line.
#[test]
fn emit_fingerprints_isolate_one_dimension_per_handler_pair() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // One matcher throughout, so `definitionFingerprint` moves only with the alpha-normalized key.
    write(root, "steps.ts", "import { Given, When } from '@cucumber/cucumber';\nGiven('step', () => { work(1); });\nGiven('step', () => { work(1); });\nGiven('other', () => { work(1); });\nGiven('step', () => { /* note */ work(1); });\nGiven('step', () => { const x = 1; work(x); });\nGiven('step', () => { const y = 1; work(y); });\nGiven('step', () => { work(2); });\nWhen('step', () => { work(1); });\n");
    let ts = emitted_fingerprints(root, &[]);
    let alpha = "handler.exact handler.normalized handler.alphaNormalized";
    let pairs = [
        (0, 1, String::new()),
        (
            0,
            2,
            "matcher normalizedMatcher definitionFingerprint".into(),
        ),
        (0, 3, "handler.exact".into()),
        (4, 5, "handler.exact handler.normalized".into()),
        (0, 6, format!("definitionFingerprint {alpha}")),
        (0, 7, "registration".into()),
    ];
    assert_isolated(&ts, &pairs);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "steps.rb", "Given('step') do\n  work(1)\nend\nGiven('step') do\n  work(1)\nend\nGiven('step') do\n  work( 1 )\nend\nGiven('step') { work(1) }\nGiven('step') do\n  work(2)\nend\n");
    let ruby = emitted_fingerprints(root, &["--definitions", "*.rb"]);
    let pairs = [
        (0, 1, String::new()),
        (0, 2, "handler.exact".into()),
        (0, 3, "handler.exact".into()),
        (0, 4, format!("definitionFingerprint {alpha}")),
    ];
    assert_isolated(&ruby, &pairs);
}

/// Checks `--emit-fingerprints` fails closed with exit 2, without truncating or leaving files, when
/// the file cannot be created, is not a regular file, or aliases a report or the `--baseline` file,
/// rejects `--print-config`, and accepts a distinct path, also beside `--update-baseline`.
#[test]
fn emit_fingerprints_fails_closed_on_unusable_destinations() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(root, "steps.ts", "Given('one', () => work());\n");
    fs::create_dir_all(root.join("reports/cuke-dedup")).unwrap();
    let run = |args: &[&str]| run_on(root, args);
    let mut rows = vec![
        ("missing/fingerprints.jsonl", "failed to write"),
        ("reports", "not a regular file"),
        ("reports/cuke-dedup/cuke-dedup.json", "would overwrite"),
        (
            "reports/../reports/cuke-dedup/cuke-dedup.sarif",
            "would overwrite",
        ),
    ];
    if cfg!(unix) {
        // Under the test harness stdout is a pipe, so this would leak records into the report.
        rows.push(("/dev/stdout", "not a regular file"));
    }
    for (destination, message) in rows {
        let output = run(&["--emit-fingerprints", destination]);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(2), "{destination}: {stderr}");
        assert!(output.stdout.is_empty(), "{destination}");
        assert!(
            stderr.contains(message) && stderr.contains(destination),
            "{destination}: {stderr}"
        );
    }
    // A rejected alias leaves no file behind, and an existing report keeps its bytes.
    assert!(!root.join("reports/cuke-dedup/cuke-dedup.json").exists());
    assert!(!root.join("reports/cuke-dedup/cuke-dedup.sarif").exists());
    let report = root.join("reports/cuke-dedup/cuke-dedup.json");
    fs::write(&report, "previous report\n").unwrap();
    let output = run(&["--emit-fingerprints", "reports/cuke-dedup/cuke-dedup.json"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(fs::read_to_string(&report).unwrap(), "previous report\n");
    fs::remove_file(&report).unwrap();
    // A symlink to a report not yet written: opening creates the target, and the rejection removes
    // that target while the pre-existing link stays.
    #[cfg(unix)]
    {
        // Relative, so the target resolves against the link's directory, not the working one.
        std::os::unix::fs::symlink("reports/cuke-dedup/cuke-dedup.json", root.join("link"))
            .unwrap();
        let output = run(&["--emit-fingerprints", "link"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains("would overwrite"));
        assert!(!report.exists());
        assert!(fs::symlink_metadata(root.join("link")).is_ok());
        fs::remove_file(root.join("link")).unwrap();
    }
    // The `--baseline` file, read or rewritten, is rejected before the baseline changes; a missing
    // baseline fails after the destination opened, which removes a new file and keeps an old one.
    write(root, "base.json", "previous baseline\n");
    write(root, "kept.jsonl", "kept\n");
    fs::create_dir(root.join("x")).unwrap();
    // A hard link to the baseline has its own canonical path; only file identity reveals it.
    fs::hard_link(root.join("base.json"), root.join("linked.jsonl")).unwrap();
    // A write-only hard-linked report is still identified and refused (root reads it anyway).
    #[cfg(unix)]
    if !ProcessCommand::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|id| id.stdout == b"0\n")
    {
        use std::os::unix::fs::PermissionsExt;
        let report = root.join("reports/cuke-dedup/cuke-dedup.json");
        fs::write(&report, "protected report\n").unwrap();
        fs::hard_link(&report, root.join("oracle.jsonl")).unwrap();
        fs::set_permissions(&report, fs::Permissions::from_mode(0o200)).unwrap();
        let output = run(&[
            "--reporters",
            "terminal",
            "--emit-fingerprints",
            "oracle.jsonl",
        ]);
        fs::set_permissions(&report, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Identity comes from metadata, so the write-only report is recognised, not just unverifiable.
        assert!(String::from_utf8_lossy(&output.stderr).contains("would overwrite"));
        assert_eq!(fs::read_to_string(&report).unwrap(), "protected report\n");
        fs::remove_file(&report).unwrap();
        fs::remove_file(root.join("oracle.jsonl")).unwrap();
        // A write-only destination is accepted: identity comes from metadata, not a read.
        write(root, "write-only.jsonl", "");
        fs::set_permissions(
            root.join("write-only.jsonl"),
            fs::Permissions::from_mode(0o200),
        )
        .unwrap();
        let output = run(&[
            "--reporters",
            "terminal",
            "--emit-fingerprints",
            "write-only.jsonl",
        ]);
        fs::set_permissions(
            root.join("write-only.jsonl"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(root.join("write-only.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        fs::remove_file(root.join("write-only.jsonl")).unwrap();
        // A report directory that cannot be searched leaves the report's identity unknown: refused.
        let reports = root.join("reports/cuke-dedup");
        fs::set_permissions(&reports, fs::Permissions::from_mode(0o000)).unwrap();
        let output = run(&[
            "--reporters",
            "terminal",
            "--emit-fingerprints",
            "unverified.jsonl",
        ]);
        fs::set_permissions(&reports, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot verify"));
        assert!(!root.join("unverified.jsonl").exists());
    }
    // A FIFO at a report path is never opened, so checking against it cannot hang.
    #[cfg(unix)]
    {
        let report = root.join("reports/cuke-dedup/cuke-dedup.json");
        assert!(ProcessCommand::new("mkfifo")
            .arg(&report)
            .status()
            .unwrap()
            .success());
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .arg(root)
            .args([
                "--reporters",
                "terminal",
                "--emit-fingerprints",
                "fifo-safe.jsonl",
            ])
            .timeout(std::time::Duration::from_secs(20))
            .assert()
            .code(0);
        fs::remove_file(&report).unwrap();
        fs::remove_file(root.join("fifo-safe.jsonl")).unwrap();
    }
    // A FIFO destination opens non-blocking: without a reader it is refused instead of waiting.
    #[cfg(unix)]
    {
        assert!(ProcessCommand::new("mkfifo")
            .arg(root.join("pipe.jsonl"))
            .status()
            .unwrap()
            .success());
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .arg(root)
            .args(["--emit-fingerprints", "pipe.jsonl"])
            .timeout(std::time::Duration::from_secs(20))
            .assert()
            .code(2)
            .stderr(predicates::str::contains(
                "pipe.jsonl is not a regular file",
            ));
        fs::remove_file(root.join("pipe.jsonl")).unwrap();
    }
    for (baseline, update, destination, message) in [
        ("base.json", false, "base.json", "would overwrite"),
        ("base.json", false, "linked.jsonl", "would overwrite"),
        ("base.json", true, "x/../base.json", "would overwrite"),
        ("absent.json", false, "new.jsonl", "absent.json"),
        ("absent.json", false, "kept.jsonl", "absent.json"),
        ("fresh.json", true, "new.jsonl", ""),
    ] {
        let mut args = vec!["--baseline", baseline, "--emit-fingerprints", destination];
        args.extend(update.then_some("--update-baseline"));
        let output = run(&args);
        let stderr = String::from_utf8(output.stderr).unwrap();
        let code = if message.is_empty() { 0 } else { 2 };
        assert_eq!(output.status.code(), Some(code), "{args:?}: {stderr}");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
        if baseline == "absent.json" {
            assert!(!root.join("new.jsonl").exists());
        }
    }
    let read = |path: &str| fs::read_to_string(root.join(path)).unwrap();
    assert_eq!(read("base.json"), "previous baseline\n");
    assert_eq!(read("kept.jsonl"), "kept\n");
    assert!(root.join("fresh.json").is_file());
    assert_eq!(read("new.jsonl").lines().count(), 1);
    let output = run(&[
        "--print-config",
        "--emit-fingerprints",
        "fingerprints.jsonl",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("cannot be used with"));
    // An existing fingerprint file longer than the new output is replaced, not overwritten in place.
    fs::write(root.join("fingerprints.jsonl"), "stale\n".repeat(500)).unwrap();
    let output = run(&["--emit-fingerprints", "fingerprints.jsonl"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(root.join("fingerprints.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

/// Counts the lines of the JSON Lines file at `path`.
fn jsonl_lines(path: &Path) -> usize {
    fs::read_to_string(path).unwrap().lines().count()
}

/// Returns the records `--emit-pairs` wrote for `root`.
fn emitted_pairs(root: &Path, args: &[&str]) -> Vec<Value> {
    emitted_records(root, args, "--emit-pairs")
}

/// Projects each pair record to its line pair, candidate sources and rules.
fn pair_outcomes(records: &[Value]) -> Vec<Value> {
    records
        .iter()
        .map(|record| {
            serde_json::json!([
                record["left"]["line"],
                record["right"]["line"],
                record["candidateSources"],
                record["rules"]
            ])
        })
        .collect()
}

/// Checks `--emit-pairs` changes no other output and records one line per evaluated pair with its
/// sources, rules and scores. A pair no source proposed has no record, and turning the rule off
/// keeps the record but empties `rules` and the handler score.
#[test]
fn emit_pairs_records_each_evaluated_pair_and_changes_no_other_output() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "steps.ts",
        "import { Given } from '@cucumber/cucumber';\n\
         Given('I open the page', () => { openPage(); });\n\
         Given('I load the page', () => { openPage(); });\n\
         Given('the cart has {int} items', (n) => { cart.add(n); check(1); });\n\
         Given('the cart has {int} items', (n) => { cart.remove(n); check(2); });\n\
         Given('the user clicks the save button', () => { click('save'); wait(1); log('a'); });\n\
         Given('the user clicks the cancel button', () => { click('cancel'); fetch('x'); render(); });\n",
    );
    write(
        root,
        "a.feature",
        "Feature: A\n  Scenario: A\n    Given I open the page\n",
    );
    let records = emitted_pairs(root, &[]);
    // The click pair shares no source, so it has no record.
    assert_eq!(
        pair_outcomes(&records),
        [
            serde_json::json!([2, 3, ["identicalHandler"], ["duplicate-handler"]]),
            serde_json::json!([4, 5, ["normalizedMatcher"], ["duplicate-matcher"]]),
        ]
    );
    assert_eq!(records[0]["schemaVersion"], "1");
    assert_eq!(records[0]["left"]["path"], "steps.ts");
    assert_eq!(records[0]["relationships"]["sameHandler"], true);
    assert_eq!(records[0]["handlerSimilarity"], 1.0);
    assert_eq!(records[1]["relationships"]["exactMatcher"], true);
    assert_eq!(records[1]["relationships"]["sameHandler"], false);

    // Control: the same pair with its rule off is still evaluated, with no rule and no handler score.
    write(
        root,
        ".cuke-dedup.json",
        r#"{"rules":{"duplicate-handler":"off"}}"#,
    );
    let off = emitted_pairs(root, &[]);
    assert_eq!(
        pair_outcomes(&off)[0],
        serde_json::json!([2, 3, ["identicalHandler"], []])
    );
    assert!(off[0]["handlerSimilarity"].is_null());
    // The normalized-matcher shortcut score is also withheld when its rule is off.
    assert_eq!(off[1]["matcherSimilarity"], 1.0);
    write(
        root,
        ".cuke-dedup.json",
        r#"{"rules":{"duplicate-matcher":"off"}}"#,
    );
    let off = emitted_pairs(root, &[]);
    assert_eq!(
        pair_outcomes(&off)[1],
        serde_json::json!([4, 5, ["normalizedMatcher"], []])
    );
    assert!(off[1]["matcherSimilarity"].is_null());
}

/// Checks the pair oracle for Ruby: identical handlers under different matchers give one
/// `duplicate-handler` record, a distinct handler none, and `positional` stays false.
#[test]
fn emit_pairs_records_each_evaluated_ruby_pair() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "features/step_definitions/steps.rb",
        "Given('I open the page') do\n  open_page(1)\nend\nGiven('I load the page') do\n  open_page(1)\nend\nGiven('totally unrelated') do\n  other(2)\nend\n",
    );
    write(
        root,
        "features/a.feature",
        "Feature: A\n  Scenario: A\n    Given I open the page\n",
    );
    let records = emitted_pairs(root, &["--definitions", "features/**/*.rb"]);
    assert_eq!(
        pair_outcomes(&records),
        [serde_json::json!([
            1,
            4,
            ["identicalHandler"],
            ["duplicate-handler"]
        ])]
    );
    assert_eq!(records[0]["positional"], false);
}

/// Checks `--emit-pairs` fails closed with exit 2 on an uncreatable path, an alias of a report or of
/// the `--emit-fingerprints` file, and `--print-config`, and writes both files when they differ.
#[test]
fn emit_pairs_fails_closed_on_unusable_destinations() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "steps.ts",
        "Given('one', () => work());\nGiven('two', () => work());\n",
    );
    fs::create_dir(root.join("sub")).unwrap();
    let run = |args: &[&str]| run_on(root, args);
    for (args, message) in [
        (
            vec!["--emit-pairs", "missing/pairs.jsonl"],
            "failed to write pair file",
        ),
        (
            vec!["--emit-pairs", "reports/cuke-dedup/cuke-dedup.json"],
            "pair file",
        ),
        (
            vec![
                "--emit-fingerprints",
                "same.jsonl",
                "--emit-pairs",
                "same.jsonl",
            ],
            "pair file",
        ),
        (
            vec![
                "--emit-fingerprints",
                "same.jsonl",
                "--emit-pairs",
                "./sub/../same.jsonl",
            ],
            "would overwrite",
        ),
        (
            vec!["--print-config", "--emit-pairs", "pairs.jsonl"],
            "cannot be used with",
        ),
    ] {
        let output = run(&args);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
    }
    // The fingerprint file of a rejected run is not left behind either.
    assert!(!root.join("same.jsonl").exists());
    let output = run(&["--emit-fingerprints", "f.jsonl", "--emit-pairs", "p.jsonl"]);
    // Exit 1: the corpus holds a duplicate-handler finding.
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(jsonl_lines(&root.join("p.jsonl")), 1);
    assert_eq!(jsonl_lines(&root.join("f.jsonl")), 2);
    // An existing pair file longer than the new output is replaced, not overwritten in place.
    fs::write(root.join("p.jsonl"), "stale\n".repeat(500)).unwrap();
    assert_eq!(run(&["--emit-pairs", "p.jsonl"]).status.code(), Some(1));
    assert_eq!(jsonl_lines(&root.join("p.jsonl")), 1);
}

/// Checks missing definition sources warn by default and fail when definitions are required.
#[test]
fn empty_default_definition_discovery_is_visible() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "example.feature",
        "Feature: Empty\n  Scenario: No implementation\n    Given a missing step\n",
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "automatic discovery found no supported step-definition source files",
        ));

    let mut required = Command::cargo_bin("cuke-dedup").unwrap();
    required
        .current_dir(directory.path())
        .args([".", "--require-definitions"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "no step definitions were extracted because no definition source files were discovered",
        ));
}

/// Checks unresolved calls report their location and preserve extracted-file and feature census counts.
#[test]
fn unresolved_registration_calls_warn_and_expose_the_corpus_census() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@company/bdd';\nGiven('invisible step', () => work());\n",
    );
    write(
        directory.path(),
        "example.feature",
        "Feature: Completeness\n  Scenario: Missing\n    Given invisible step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--reporters", "json"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "steps.ts:2:1: unresolved step-registration calls: 1 call(s)",
        ))
        .stderr(predicate::str::contains(
            "definition extraction produced 0 definitions from 1 discovered definition source file(s)",
        ));

    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["corpus"]["definitionFiles"], 1);
    assert_eq!(report["corpus"]["definitionFilesWithDefinitions"], 0);
    assert_eq!(report["corpus"]["definitionsExtracted"], 0);
    assert_eq!(report["corpus"]["featureFiles"], 1);
    assert_eq!(report["corpus"]["featureFilesParsed"], 1);
    assert_eq!(report["corpus"]["featureFilesWithoutSteps"], 0);
}

/// Checks empty Markdown conversion suppresses unused findings, marks reports incomplete, and fails strict mode.
#[test]
fn empty_converted_gherkin_markdown_marks_the_corpus_incomplete() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@cucumber/cucumber';\nGiven('una cuenta activa', () => work());\n",
    );
    write(
        directory.path(),
        "features/account.feature.md",
        "# Característica: Cuenta\n\n## Escenario: Disponible\n\n* Dado una cuenta activa\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    let assertion = command
        .current_dir(directory.path())
        .args([".", "--reporters", "json,jsonl,html,sarif"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unused-definition").not())
        .stderr(predicate::str::contains(
            "Gherkin Markdown file features/account.feature.md parsed successfully but produced 0 feature steps",
        ));

    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        report["summary"]["byRule"]["unused-definition"],
        Value::Null
    );
    assert_eq!(report["corpus"]["featureFiles"], 1);
    assert_eq!(report["corpus"]["featureFilesParsed"], 1);
    assert_eq!(report["corpus"]["featureFilesWithoutSteps"], 1);
    assert_eq!(report["corpus"]["incomplete"], true);

    let jsonl = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let final_record: Value =
        serde_json::from_str(jsonl.lines().last().expect("JSONL summary record")).unwrap();
    assert_eq!(final_record["corpus"]["featureFilesWithoutSteps"], 1);
    assert_eq!(final_record["corpus"]["incomplete"], true);

    let html =
        fs::read_to_string(directory.path().join("reports/cuke-dedup/cuke-dedup.html")).unwrap();
    assert!(html.contains("Corpus is incomplete"));
    assert!(html.contains("\"featureFilesWithoutSteps\":1"));

    let sarif: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup/cuke-dedup.sarif")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["properties"]["corpus"]["featureFilesWithoutSteps"],
        1
    );
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        false
    );

    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--reporters", "json", "--fail-on-incomplete"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "input extraction did not fully represent every discovered definition or feature file",
        ));
}

/// Checks an empty classic feature leaves the corpus complete and retains unused findings from valid feature usage.
#[test]
fn valid_empty_classic_feature_does_not_hide_unused_definitions() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@cucumber/cucumber';\nGiven('used step', () => used());\nGiven('unused step', () => unused());\n",
    );
    write(
        directory.path(),
        "features/placeholder.feature",
        "Feature: Planned work\n",
    );
    write(
        directory.path(),
        "features/active.feature",
        "Feature: Active\n  Scenario: Current\n    Given used step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--reporters", "json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("produced 0 feature steps").not());

    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["summary"]["byRule"]["unused-definition"], 1);
    assert_eq!(report["corpus"]["featureFilesWithoutSteps"], 0);
    assert_eq!(report["corpus"]["incomplete"], false);
}

/// Checks partially resolved source files report unresolved calls and the actual extraction census.
#[test]
fn partial_definition_extraction_is_visible_in_the_corpus_census() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "resolved.ts",
        "Given('visible step', () => work());\n",
    );
    write(
        directory.path(),
        "unresolved.ts",
        "import { Then } from '@company/bdd';\nThen('invisible step', () => work());\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--reporters", "json"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "unresolved.ts:2:1: unresolved step-registration calls",
        ));
    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["corpus"]["definitionFiles"], 2);
    assert_eq!(report["corpus"]["definitionFilesWithDefinitions"], 1);
    assert_eq!(report["corpus"]["definitionsExtracted"], 1);
}

/// Checks changed mode still warns about unresolved registrations in unchanged corpus files.
#[test]
fn unresolved_registration_warnings_are_not_hidden_for_unchanged_corpus_files() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@company/bdd';\nGiven('invisible step', () => work());\n",
    );
    init_repository(directory.path());
    write(directory.path(), "changed.txt", "changed\n");

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "steps.ts:2:1: unresolved step-registration calls",
        ));
}

/// Checks required definitions fail on empty extraction while CLI and configured JSON reports are still written.
#[test]
fn empty_definition_extraction_can_be_required_and_still_writes_reports() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "export const helper = true;\n",
    );
    write(
        directory.path(),
        "example.feature",
        "Feature: Completeness\n  Scenario: Missing\n    Given invisible step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([
            ".",
            "--require-definitions",
            "--reporters",
            "json",
            "--output",
            "reports",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "definition extraction produced 0 definitions from 1 discovered definition source file(s)",
        ));
    assert!(directory.path().join("reports/cuke-dedup.json").is_file());

    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"requireDefinitions":true,"reporters":["json"],"output":"configured-report"}"#,
    );
    let mut configured = Command::cargo_bin("cuke-dedup").unwrap();
    configured
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(2);
    assert!(directory
        .path()
        .join("configured-report/cuke-dedup.json")
        .is_file());
}

/// Checks candidate truncation retains findings and report evidence, prevents baseline updates, and fails only under strict gating.
#[test]
fn candidate_limits_warn_and_still_report_without_failing_the_run() {
    let directory = tempfile::tempdir().unwrap();
    let mut source = String::new();
    for index in 0..10 {
        source.push_str(&format!(
            "Given('operation label {index}', () => perform({index}));\n"
        ));
    }
    write(directory.path(), "steps.ts", &source);
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{
          "reporters": ["json", "html", "sarif"],
          "output": "reports",
          "noMetrics": true,
          "maxCandidateComparisons": 100,
          "maxStructuralClassComparisons": 2
        }"#,
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "warning: analysis is incomplete: evaluated 2 candidate definition comparisons",
        ))
        .stderr(predicate::str::contains(
            "affected structural classes start at steps.ts:1:1",
        ));

    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert!(report.get("metrics").is_none());
    assert_eq!(report["analysis"]["truncated"], true);
    assert_eq!(report["analysis"]["candidateComparisonsEvaluated"], 2);
    assert_eq!(report["analysis"]["skippedCandidateComparisons"], 43);
    assert_eq!(report["analysis"]["truncatedStructuralClasses"], 1);
    assert_eq!(
        report["analysis"]["candidateSources"]["structuralHandler"]["evaluated"],
        2
    );
    assert!(!report["findings"].as_array().unwrap().is_empty());

    let html = fs::read_to_string(directory.path().join("reports/cuke-dedup.html")).unwrap();
    assert!(html.contains("Analysis is incomplete: 2 candidate comparisons were evaluated"));
    assert!(html.contains(r#"class="truncation-notice" role="alert""#));
    let sarif: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup.sarif")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["properties"]["analysis"]["truncated"],
        true
    );
    // Incomplete coverage is an unsuccessful invocation for SARIF consumers, even though the
    // findings it did produce are valid and the CLI does not fail by default.
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        false
    );

    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--reporters", "jsonl"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let summary: Value = serde_json::from_slice(
        output
            .stdout
            .split(|byte| *byte == b'\n')
            .rfind(|line| !line.is_empty())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["analysis"]["truncated"], true);
    assert!(summary.get("metrics").is_none());

    let baseline_path = directory.path().join("incomplete-baseline.json");
    let mut update = Command::cargo_bin("cuke-dedup").unwrap();
    update
        .current_dir(directory.path())
        .args([
            ".",
            "--baseline",
            "incomplete-baseline.json",
            "--update-baseline",
            "--reporters",
            "json",
        ])
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "was not updated because analysis is incomplete",
        ));
    assert!(!baseline_path.exists());

    // Gates that must refuse partial coverage opt in explicitly.
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--reporters", "json", "--fail-on-incomplete"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cuke-dedup: analysis is incomplete: evaluated 2 candidate definition comparisons",
        ));
}

/// Checks a CLI severity override makes duplicate matchers warnings and permits success.
#[test]
fn explicit_check_honors_cli_rule_severity_for_exit_status() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('same step', () => { first(); });\nGiven('same step', () => { second(); });\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args(["check", ".", "--rule", "duplicate-matcher=warning"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[warning]"));
}

/// Checks static Playwright-BDD discovery analyzes a definition and its feature without framework execution.
#[test]
fn zero_config_analyzes_playwright_bdd_without_loading_playwright() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "features/steps/home.ts",
        r#"
import { createBdd } from 'playwright-bdd';
const { Given: Setup } = createBdd(test);
Setup('the home page is open', async ({ page }) => { await page.goto('/'); });
"#,
    );
    write(
        directory.path(),
        "features/home.feature",
        "Feature: Home\n  Scenario: Visit\n    Given the home page is open\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args(["check", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Analyzed 1 definition and 1 feature step",
        ))
        .stdout(predicate::str::contains("0 errors, 0 warnings"));
}

/// Checks invalid Gherkin returns exit code 2 with a parse diagnostic.
#[test]
fn invalid_gherkin_is_an_operational_error() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "broken.feature",
        "Scenario: Missing feature\n  Given a step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("failed to parse Gherkin feature"));
}

/// Checks default discovery parses Markdown feature steps and explains their source format.
#[test]
fn default_discovery_parses_gherkin_markdown() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('a documented step', () => work());\n",
    );
    write(
        directory.path(),
        "features/documentation.feature.md",
        "# Feature: Documentation\n\n## Scenario: Markdown\n\n* Given a documented step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--explain-discovery"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Analyzed 1 definition and 1 feature step",
        ))
        .stderr(predicate::str::contains(
            "documentation.feature.md [gherkin-markdown]",
        ));
}

/// Checks an explicit feature pattern permits Gherkin files with a nonstandard extension.
#[test]
fn custom_patterns_allow_nonstandard_gherkin_extensions() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('a custom feature', () => work());\n",
    );
    write(
        directory.path(),
        "specs/example.spec",
        "Feature: Custom\n  Scenario: Extension\n    Given a custom feature\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--features", "specs/**/*.spec"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Analyzed 1 definition and 1 feature step",
        ));
}

/// Checks framework feature defaults apply until project configuration supplies an overriding pattern.
#[test]
fn framework_feature_paths_are_defaults_and_own_config_overrides_them() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "playwright.config.ts",
        "const testDir = defineBddConfig({ features: ['specs'] });\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "Given('from framework config', () => work());\n",
    );
    write(
        directory.path(),
        "specs/example.feature",
        "Feature: Framework\n  Scenario: Paths\n    Given from framework config\n",
    );

    let mut framework = Command::cargo_bin("cuke-dedup").unwrap();
    framework
        .current_dir(directory.path())
        .args([".", "--explain-discovery"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 feature step"))
        .stderr(predicate::str::contains("Playwright-BDD configuration"));

    write(
        directory.path(),
        "cuke-dedup.config.json",
        r#"{"features":["acceptance/**/*.feature"]}"#,
    );
    write(
        directory.path(),
        "acceptance/example.feature",
        "Feature: Override\n  Scenario: Paths\n    Given from framework config\n",
    );
    let mut own = Command::cargo_bin("cuke-dedup").unwrap();
    own.current_dir(directory.path())
        .args([".", "--explain-discovery"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 feature step"))
        .stderr(predicate::str::contains("cuke-dedup.config.json"))
        .stderr(predicate::str::contains("specs/example.feature").not());
}

/// Checks local ESM registration re-exports retain definitions and duplicate-matcher findings.
#[test]
fn local_barrel_reexports_are_recognized_as_registration_sources() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "support/world.ts",
        "export { Given, When, Then } from '@cucumber/cucumber';\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from './support/world';\nGiven('same barrel step', () => first());\nGiven('same barrel step', () => second());\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--threshold", "100"])
        .assert()
        .success()
        .stdout(predicate::str::contains("duplicate-matcher"))
        .stdout(predicate::str::contains("Analyzed 2 definitions"));
}

/// Checks an aliased CommonJS registration from a local barrel retains duplicate-matcher detection.
#[test]
fn commonjs_require_of_a_local_barrel_resolves_registrations() {
    let directory = tempfile::tempdir().unwrap();
    // A CommonJS barrel re-exporting a registration, imported through a project-local `require`
    // under a new name — the CommonJS mirror of the ESM case above. Resolving the local barrel is
    // the only evidence these register steps, so a regression drops both definitions.
    write(
        directory.path(),
        "support/world.js",
        "const { Given } = require('@cucumber/cucumber');\nmodule.exports = { Given };\n",
    );
    write(
        directory.path(),
        "steps.js",
        "const { Given: registerStep } = require('./support/world');\nregisterStep('cjs require barrel step', () => first());\nregisterStep('cjs require barrel step', () => second());\n",
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--threshold", "100"])
        .assert()
        .success()
        .stdout(predicate::str::contains("duplicate-matcher"))
        .stdout(predicate::str::contains("Analyzed 2 definitions"));
}

/// Checks requiring an ordinary helper module does not invent step definitions.
#[test]
fn commonjs_require_of_a_local_non_registration_module_registers_nothing() {
    let directory = tempfile::tempdir().unwrap();
    // The opposite-answer control: a project-local `require` whose module exports no registration
    // must not invent step definitions. Only a resolved registration binding counts.
    write(
        directory.path(),
        "support/helpers.js",
        "module.exports = { formatDate: (d) => String(d) };\n",
    );
    write(
        directory.path(),
        "steps.js",
        "const { formatDate } = require('./support/helpers');\nformatDate('not a step');\nformatDate('also not a step');\n",
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--threshold", "100"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Analyzed 0 definitions"));
}

/// Checks cyclic and over-depth project configurations warn and cannot supply registration aliases.
#[test]
fn project_config_extends_cycles_and_excessive_depth_never_resolve() {
    let cycle = tempfile::tempdir().unwrap();
    write(
        cycle.path(),
        "tsconfig.json",
        r#"{"extends":"./config/base.json"}"#,
    );
    write(
        cycle.path(),
        "config/base.json",
        r#"{"extends":"../tsconfig.json","compilerOptions":{"paths":{"@support/*":["../support/*"]}}}"#,
    );
    write(
        cycle.path(),
        "steps.ts",
        "import { Given as G } from '@support/world';\nG('cycle must not hide this definition', () => work());\n",
    );

    let mut cycle_command = Command::cargo_bin("cuke-dedup").unwrap();
    cycle_command
        .current_dir(cycle.path())
        .args([".", "--definitions", "steps.ts"])
        .assert()
        .code(0)
        .stderr(predicate::str::contains("project config extends cycle"))
        .stderr(predicate::str::contains(
            "definition extraction produced 0 definitions",
        ));

    let depth = tempfile::tempdir().unwrap();
    write(
        depth.path(),
        "tsconfig.json",
        r#"{"extends":"./config/0.json"}"#,
    );
    for index in 0..65 {
        let contents = if index == 64 {
            r#"{"compilerOptions":{"paths":{"@support/*":["../support/*"]}}}"#.to_owned()
        } else {
            format!(r#"{{"extends":"./{}.json"}}"#, index + 1)
        };
        write(depth.path(), &format!("config/{index}.json"), &contents);
    }
    write(
        depth.path(),
        "steps.ts",
        "import { Given as G } from '@support/world';\nG('depth must not hide this definition', () => work());\n",
    );

    let mut depth_command = Command::cargo_bin("cuke-dedup").unwrap();
    depth_command
        .current_dir(depth.path())
        .args([".", "--definitions", "steps.ts"])
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "project config extends exceeds the 16-file depth limit",
        ))
        .stderr(predicate::str::contains(
            "definition extraction produced 0 definitions",
        ));
}

/// Asserts an escaping path alias warns and extracts no definitions without aborting the default run.
fn assert_path_alias_escape_is_incomplete(root: &Path) {
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(root)
        .args([".", "--definitions", "steps.ts"])
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "resolves outside package or analysis root",
        ))
        .stderr(predicate::str::contains("warning: steps.ts:"))
        .stderr(predicate::str::contains(
            "definition extraction produced 0 definitions",
        ));
}

/// Checks a traversing path alias cannot resolve registrations outside the analysis root.
#[test]
fn path_alias_targets_cannot_escape_the_analysis_root() {
    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path().join("root");
    write(
        &root,
        "tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@support/*":["../outside/*"]}}}"#,
    );
    write(
        &root,
        "steps.ts",
        "import { Given as G } from '@support/world';\nG('escaped aliases are unsafe', () => work());\n",
    );
    write(
        sandbox.path(),
        "outside/world.ts",
        "export { Given } from '@cucumber/cucumber';\n",
    );

    assert_path_alias_escape_is_incomplete(&root);
}

/// Checks symlink resolution cannot bypass the registration alias containment boundary.
#[cfg(unix)]
#[test]
fn symlinked_path_alias_targets_cannot_escape_the_analysis_root() {
    use std::os::unix::fs::symlink;

    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path().join("root");
    let outside = sandbox.path().join("outside");
    fs::create_dir_all(&root).unwrap();
    write(
        &root,
        "tsconfig.json",
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@support/*":["escape/*"]}}}"#,
    );
    write(
        &root,
        "steps.ts",
        "import { Given as G } from '@support/world';\nG('symlink escapes are unsafe', () => work());\n",
    );
    write(
        &outside,
        "world.ts",
        "export { Given } from '@cucumber/cucumber';\n",
    );
    symlink(&outside, root.join("escape")).unwrap();

    assert_path_alias_escape_is_incomplete(&root);
}

/// Checks changed mode reports unsupported executable configuration for every reporter without executing it.
#[test]
fn static_project_config_errors_survive_changed_mode_and_every_reporter() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "tsconfig.json",
        r#"{"extends":"./execute-config.js"}"#,
    );
    write(
        directory.path(),
        "execute-config.js",
        "require('node:fs').writeFileSync('executed.txt', 'unsafe');\nmodule.exports = {};\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given as G } from '@support/world';\nG('static configuration only', () => work());\n",
    );
    init_repository(directory.path());
    write(directory.path(), "changed.txt", "changed\n");

    for reporter in ["terminal", "json", "jsonl", "html", "sarif"] {
        let output = tempfile::tempdir().unwrap();
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command
            .current_dir(directory.path())
            .args([
                ".",
                "--definitions",
                "steps.ts",
                "--changed-since",
                "HEAD",
                "--reporters",
                reporter,
                "--output",
            ])
            .arg(output.path())
            .assert()
            .code(0)
            .stderr(predicate::str::contains(
                "project config extends only supports static JSON",
            ));
    }
    assert!(!directory.path().join("executed.txt").exists());
}

/// Checks an oversized registration module emits its input-limit warning and empty-extraction diagnostic.
#[test]
fn oversized_registration_modules_cannot_silently_remove_definitions() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"definitions":["steps.ts"]}"#,
    );
    write_sized(directory.path(), "support/world.ts", 8 * 1024 * 1024 + 1);
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from './support/world';\nGiven('hidden by barrel', () => work());\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(0)
        .stderr(predicate::str::contains("registration module"))
        .stderr(predicate::str::contains("8388608-byte input limit"))
        .stderr(predicate::str::contains(
            "definition extraction produced 0 definitions",
        ));
}

/// Checks ambiguity membership and locations follow selected definitions and features across package boundaries.
#[test]
fn ambiguous_steps_follow_the_selected_corpus_not_package_boundaries() {
    for local_conflict in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        for (suite, import) in [
            (
                "alpha",
                "import { Given } from '@badeball/cypress-cucumber-preprocessor';",
            ),
            (
                "beta",
                "const { Given } = require('@badeball/cypress-cucumber-preprocessor');",
            ),
        ] {
            let root = directory.path().join(suite);
            write(&root, "package.json", &format!(r#"{{"name":"{suite}"}}"#));
            write(
                &root,
                ".cypress-cucumber-preprocessorrc.json",
                r#"{"stepDefinitions":["*.js"]}"#,
            );
            write(
                &root,
                "steps.js",
                &format!("{import}\nGiven('the lamp is on', () => {{}});\n"),
            );
            write(
                &root,
                "lamp.feature",
                &format!("@{suite}\nFeature: Lamp\n Scenario: Lit\n  Given the lamp is on\n"),
            );
        }
        if local_conflict {
            write(
                directory.path(),
                "alpha/extra.js",
                "import { Then } from '@badeball/cypress-cucumber-preprocessor';\nThen(/^the lamp is (on|off)$/, () => {});\n",
            );
        }
        let extra = u64::from(local_conflict);
        // root, definition filter, feature filter, definitions, steps, ambiguous steps
        for (root, definitions, features, definition_count, step_count, ambiguous_count) in [
            (".", "**/*.js", "**/*.feature", 2 + extra, 2, 2),
            ("alpha", "**/*.js", "**/*.feature", 1 + extra, 1, extra),
            ("beta", "**/*.js", "**/*.feature", 1, 1, 0),
            (".", "alpha/*.js", "alpha/*.feature", 1 + extra, 1, extra),
            (".", "beta/*.js", "beta/*.feature", 1, 1, 0),
            (".", "**/*.js", "alpha/*.feature", 2 + extra, 1, 1),
            (".", "alpha/*.js", "**/*.feature", 1 + extra, 2, 2 * extra),
        ] {
            let context = format!("{root}: {definitions}, {features}, conflict={local_conflict}");
            let output = Command::cargo_bin("cuke-dedup")
                .unwrap()
                .current_dir(directory.path())
                .args([
                    root,
                    "--definitions",
                    definitions,
                    "--features",
                    features,
                    "--threshold",
                    "100",
                    "--reporters",
                    "jsonl",
                    "--no-metrics",
                ])
                .assert()
                .code(i32::from(ambiguous_count > 0))
                .get_output()
                .stdout
                .clone();
            let records: Vec<Value> = String::from_utf8(output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let summary = records.last().unwrap();
            assert_eq!(summary["type"], "summary", "{context}");
            assert_eq!(
                summary["summary"]["definitionsAnalyzed"], definition_count,
                "{context}"
            );
            assert_eq!(
                summary["summary"]["featureStepsAnalyzed"], step_count,
                "{context}"
            );
            assert_eq!(summary["corpus"]["incomplete"], false, "{context}");
            let ambiguous: Vec<_> = records
                .iter()
                .filter(|record| record["rule"] == "ambiguous-step")
                .collect();
            assert_eq!(ambiguous.len() as u64, ambiguous_count, "{context}");
            let prefix = if root == "." { "alpha/" } else { "" };
            let mut expected_related = vec![format!("{prefix}steps.js")];
            if local_conflict && definitions != "beta/*.js" && root != "beta" {
                expected_related.push(format!("{prefix}extra.js"));
            }
            if root == "." && definitions == "**/*.js" {
                expected_related.push("beta/steps.js".to_string());
            }
            expected_related.sort();
            let mut primary_paths = Vec::new();
            for finding in ambiguous {
                assert_eq!(finding["active"], true, "{context}");
                assert_eq!(finding["primary"]["line"], 4, "{context}");
                primary_paths.push(finding["primary"]["path"].as_str().unwrap());
                let related = finding["related"].as_array().unwrap();
                assert_eq!(related.len() as u64, definition_count, "{context}");
                assert!(
                    related.iter().all(|location| location["line"] == 2),
                    "{context}"
                );
                let mut paths: Vec<_> = related
                    .iter()
                    .map(|location| location["path"].as_str().unwrap())
                    .collect();
                paths.sort();
                assert_eq!(paths, expected_related, "{context}");
            }
            primary_paths.sort();
            let expected_primary = match (ambiguous_count, root) {
                (0, _) => vec![],
                (2, _) => vec!["alpha/lamp.feature", "beta/lamp.feature"],
                (_, ".") => vec!["alpha/lamp.feature"],
                _ => vec!["lamp.feature"],
            };
            assert_eq!(primary_paths, expected_primary, "{context}");
        }
    }
}

/// Checks witnessed matcher overlap is reported without features and yields to proven feature ambiguity.
#[test]
fn matcher_overlap_is_reported_without_a_feature_corpus() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "import { Given, When } from '@cucumber/cucumber';\n\
Given(/^I have (\\d+) items$/, async () => { await a(); });\n\
Given('I have {int} items', async () => { await b(); });\n\
When('I open the cart', async () => { await c(); });\n\
When('I close the cart', async () => { await d(); });\n",
    );

    // No feature file exists, so `ambiguous-step` has nothing to prove the overlap with.
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("overlapping-matcher"))
        .stdout(predicate::str::contains(
            "both accept the step `I have 1 items`",
        ))
        // Distinct matchers must not be flagged.
        .stdout(predicate::str::contains("I open the cart").not());

    // Once a feature proves the ambiguity, the stronger rule reports it and overlap steps aside.
    write(
        directory.path(),
        "x.feature",
        "Feature: F\n  Scenario: S\n    Given I have 3 items\n",
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("ambiguous-step"))
        .stdout(predicate::str::contains("overlapping-matcher").not());
}

/// Checks overlap truncation exposes evaluated and skipped work and fails when completeness is required.
#[test]
fn matcher_overlap_limit_is_machine_visible_and_can_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('value {first}', () => {});\n\
         Given('value {second}', () => {});\n\
         Given('value {third}', () => {});\n",
    );
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{
          "threshold": 100,
          "reporters": ["json"],
          "output": "reports",
          "noMetrics": true,
          "maxCandidateComparisons": 1,
          "parameterTypes": {
            "first": "red|green",
            "second": "red|green",
            "third": "red|green"
          }
        }"#,
    );

    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "matcher-overlap analysis is incomplete",
        ));
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(directory.path().join("reports/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["analysis"]["truncated"], true);
    assert_eq!(
        report["analysis"]["candidateSources"]["matcherOverlap"]["evaluated"],
        1
    );
    assert_eq!(
        report["analysis"]["candidateSources"]["matcherOverlap"]["skipped"],
        1
    );

    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--fail-on-incomplete"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "matcher-overlap analysis is incomplete",
        ));
}

/// Checks declared parameter patterns enable exact unused and overlap findings and reject invalid regexes.
#[test]
fn declared_parameter_types_make_usage_analysis_exact() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@cucumber/cucumber';\n\
Given('the {colour} light is on', async () => { await a(); });\n\
Given(/^the (red|green) light is on$/, async () => { await b(); });\n",
    );
    write(
        directory.path(),
        "x.feature",
        "Feature: F\n  Scenario: S\n    Given the bright blue light is on\n",
    );

    // Undeclared, `{colour}` degrades to a permissive pattern: it swallows `bright blue`, so the
    // definition looks used, and it is not authoritative enough to prove the overlap.
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("ambiguous-step").not())
        .stdout(predicate::str::contains("the {colour} light is on` is not used").not());

    // Declared, the same matcher is exact: `bright blue` no longer matches, and the genuine
    // overlap with the regular expression becomes provable.
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"parameterTypes":{"colour":"red|green|amber"}}"#,
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "Step definition `the {colour} light is on` is not used",
        ))
        .stdout(predicate::str::contains("overlapping-matcher"));

    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"parameterTypes":{"colour":"red("}}"#,
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "invalid regular expression for parameter type `colour`",
        ));
}

/// Checks unresolved imports mark reports incomplete, prevent baseline updates, and fail explicit completeness gates.
#[test]
fn an_unresolved_registration_import_marks_the_whole_run_incomplete() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"definitions":["steps.ts"],"reporters":["json","html","sarif"],"output":"reports","noMetrics":true}"#,
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@absent/world';\nGiven('hidden by an unresolved alias', () => work());\n",
    );

    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "unresolved step-registration calls",
        ));

    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["corpus"]["incomplete"], true);

    let html = fs::read_to_string(directory.path().join("reports/cuke-dedup.html")).unwrap();
    assert!(html.contains("Corpus is incomplete"), "{html}");

    let sarif: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup.sarif")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        false
    );

    // An incomplete corpus must not be recorded as an accepted baseline.
    let baseline_path = directory.path().join("corpus-baseline.json");
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([
            ".",
            "--baseline",
            "corpus-baseline.json",
            "--update-baseline",
            "--reporters",
            "json",
        ])
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "was not updated because analysis is incomplete",
        ));
    assert!(!baseline_path.exists());

    // Both the flag and its project-configuration equivalent turn it into a failure.
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--reporters", "json", "--fail-on-incomplete"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "input extraction did not fully represent every discovered definition or feature file",
        ));

    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"definitions":["steps.ts"],"reporters":["json"],"output":"reports","failOnIncomplete":true}"#,
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "input extraction did not fully represent every discovered definition or feature file",
        ));
}

/// Checks a filesystem-denied registration module remains a fatal read error.
#[cfg(unix)]
#[test]
fn unreadable_registration_modules_remain_operational_failures() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"definitions":["steps.ts"]}"#,
    );
    write(
        directory.path(),
        "support/world.ts",
        "export { Given } from '@cucumber/cucumber';\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from './support/world';\nGiven('hidden by an unreadable barrel', () => work());\n",
    );
    let barrel = directory.path().join("support/world.ts");
    fs::set_permissions(&barrel, fs::Permissions::from_mode(0o000)).unwrap();

    let assertion = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert();
    fs::set_permissions(&barrel, fs::Permissions::from_mode(0o644)).unwrap();

    // A module the filesystem refuses is a read failure, not a static-resolution limitation, so
    // it must stay fatal rather than degrade to an "unresolved module" warning.
    assertion
        .code(2)
        .stderr(predicate::str::contains("failed to read"))
        .stderr(predicate::str::contains("support/world.ts"));
}

/// Checks registration graph limits accumulate across multiple selected definition files.
#[test]
fn registration_module_budget_is_shared_across_definition_files() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"definitions":["steps-a.ts","steps-b.ts"]}"#,
    );
    for group in ['a', 'b'] {
        let mut imports = String::new();
        for index in 0..520 {
            write(
                directory.path(),
                &format!("support/{group}-{index}.ts"),
                "export { Given } from '@cucumber/cucumber';\n",
            );
            imports.push_str(&format!(
                "import {{ Given as G{index} }} from './support/{group}-{index}';\n"
            ));
        }
        write(directory.path(), &format!("steps-{group}.ts"), &imports);
    }

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "registration module graph exceeds the 1024-module resolution limit",
        ))
        .stderr(predicate::str::contains(
            "definition extraction produced 0 definitions",
        ));
}

/// Checks erased exports and over-depth barrels cannot create runtime step definitions.
#[test]
fn type_only_and_over_depth_barrels_do_not_create_runtime_registrations() {
    for setup in ["type-only", "over-depth"] {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            ".cuke-dedup.json",
            r#"{"definitions":["steps.ts"]}"#,
        );
        write(
            directory.path(),
            "steps.ts",
            "import { Given } from './support/0';\nGiven('not registered', () => work());\n",
        );
        if setup == "type-only" {
            write(
                directory.path(),
                "support/0.ts",
                "export type { Given } from '@cucumber/cucumber';\n",
            );
        } else {
            for depth in 0..16 {
                write(
                    directory.path(),
                    &format!("support/{depth}.ts"),
                    &format!("export {{ Given }} from './{}';\n", depth + 1),
                );
            }
            write(
                directory.path(),
                "support/16.ts",
                "export { Given } from '@cucumber/cucumber';\n",
            );
        }

        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command
            .current_dir(directory.path())
            .args([".", "--threshold", "100"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Analyzed 0 definitions"));
    }
}

/// Checks configuration and baseline input-size limits produce fatal, source-specific diagnostics.
#[test]
fn oversized_config_and_baseline_inputs_are_fatal() {
    let cases = [
        (
            "package.json",
            1024 * 1024 + 1,
            Vec::new(),
            "package configuration",
        ),
        (
            ".cuke-dedup.json",
            1024 * 1024 + 1,
            Vec::new(),
            "CukeDedup configuration",
        ),
        (
            "playwright.config.ts",
            1024 * 1024 + 1,
            Vec::new(),
            "framework configuration",
        ),
        (
            "baseline.json",
            8 * 1024 * 1024 + 1,
            vec!["--baseline", "baseline.json"],
            "baseline",
        ),
    ];

    for (relative, bytes, arguments, kind) in cases {
        let directory = tempfile::tempdir().unwrap();
        write_sized(directory.path(), relative, bytes);
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command
            .current_dir(directory.path())
            .arg(".")
            .args(arguments)
            .assert()
            .code(2)
            .stderr(predicate::str::contains(kind))
            .stderr(predicate::str::contains("input limit"));
    }
}

/// Commits an oversized feature fixture and adds an unrelated change for changed-mode checks.
fn oversized_unchanged_feature_repository(root: &Path) {
    write_sized(root, "features/example.feature", 8 * 1024 * 1024 + 1);
    init_repository(root);
    write(root, "changed.txt", "changed\n");
}

/// Checks changed analysis rejects oversized unchanged definitions and features together.
#[test]
fn oversized_unchanged_sources_and_features_fail_changed_analysis_closed() {
    let directory = tempfile::tempdir().unwrap();
    write_sized(directory.path(), "steps.ts", 8 * 1024 * 1024 + 1);
    oversized_unchanged_feature_repository(directory.path());

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("definition source"))
        .stderr(predicate::str::contains("feature file"));
}

/// Checks an oversized unchanged feature remains a fatal input error in changed mode.
#[test]
fn an_oversized_unchanged_feature_alone_fails_changed_analysis_closed() {
    let directory = tempfile::tempdir().unwrap();
    oversized_unchanged_feature_repository(directory.path());

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("feature file"))
        .stderr(predicate::str::contains("input limit"));
}

/// Checks invalid UTF-8 in an unchanged definition source still fails changed analysis.
#[test]
fn an_unreadable_unchanged_definition_cannot_make_changed_analysis_pass() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("steps.ts"), [0xff]).unwrap();
    init_repository(directory.path());
    write(directory.path(), "changed.txt", "changed\n");

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not valid UTF-8"));
}

/// Checks unchanged syntax errors warn in changed mode and become fatal under the unparseable-source gate.
#[test]
fn a_malformed_unchanged_definition_is_reported_in_changed_mode_and_gated_by_the_flag() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('broken step', () => {\n",
    );
    init_repository(directory.path());
    write(directory.path(), "changed.txt", "changed\n");

    // Default: the malformed unchanged definition is tolerated, not aborted, but its parse failure
    // is a completeness signal so it is still reported even though it is an unchanged file. It
    // cannot silently make the run pass — the warning surfaces the incompleteness.
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "source contains JavaScript/TypeScript syntax errors",
        ));

    // `--fail-on-unparseable` restores the hard stop for exactly this case.
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD", "--fail-on-unparseable"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "source contains JavaScript/TypeScript syntax errors",
        ));
}

/// Checks parse failures retain valid sibling findings by default and fail either strict source or completeness gating.
#[test]
fn an_unparseable_file_is_tolerated_and_reported_by_default_and_gated_by_flags() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"rules":{"duplicate-matcher":"warning"},"threshold":100}"#,
    );
    write(
        directory.path(),
        "valid.ts",
        "Given('an analyzed step', () => first());\nGiven('an analyzed step', () => second());\n",
    );
    write(directory.path(), "broken.ts", "Given('broken', () => {\n");

    // Default: the unparseable file does not abort the scan. The valid file's definition is still
    // analyzed and the parse failure is reported, so the run is visibly incomplete, not silently
    // clean.
    analyze_root_with(directory.path(), &["--reporters", "json"])
        .success()
        .stderr(predicate::str::contains(
            "source contains JavaScript/TypeScript syntax errors",
        ));
    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("reports/cuke-dedup/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["corpus"]["incomplete"], true);
    assert_eq!(report["summary"]["byRule"]["duplicate-matcher"], 1);

    // `--fail-on-unparseable` turns exactly this case into a hard failure.
    analyze_root_with(directory.path(), &["--fail-on-unparseable"])
        .code(2)
        .stderr(predicate::str::contains(
            "source contains JavaScript/TypeScript syntax errors",
        ));

    // `--fail-on-incomplete` also rejects it, as one kind of corpus incompleteness.
    analyze_root_with(directory.path(), &["--fail-on-incomplete"])
        .code(2)
        .stderr(predicate::str::contains("incomplete"));
}

/// Checks valid sources stay free of syntax warnings and pass the unparseable-source gate.
#[test]
fn a_corpus_of_only_parseable_files_reports_no_syntax_incompleteness() {
    // Opposite-answer control for the tolerance case above: with no unparseable file the run is
    // clean and silent, so the warning there is caused by the parse failure rather than emitted
    // unconditionally, and `--fail-on-unparseable` does not fire.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "valid.ts",
        "Given('an analyzed step', () => work());\n",
    );
    analyze_root(directory.path()).success().stderr(
        predicate::str::contains("source contains JavaScript/TypeScript syntax errors").not(),
    );
    analyze_root_with(directory.path(), &["--fail-on-unparseable"]).success();
}

/// Checks unchanged unsupported matchers do not emit definition-scoped usage warnings in changed mode.
#[test]
fn changed_mode_keeps_non_fatal_warnings_scoped_to_changed_definitions() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given(/foo(?=bar)/, () => work());\n",
    );
    init_repository(directory.path());
    write(directory.path(), "changed.txt", "changed\n");

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD", "--threshold", "100"])
        .assert()
        .success()
        .stderr(predicate::str::contains("unsupported by static usage analysis").not());
}

/// Checks oversized definition inputs fail consistently for every reporter.
#[test]
fn oversized_source_exit_behavior_is_reporter_independent() {
    let directory = tempfile::tempdir().unwrap();
    write_sized(directory.path(), "steps.ts", 8 * 1024 * 1024 + 1);

    for reporter in ["terminal", "json", "jsonl", "html", "sarif"] {
        let output = tempfile::tempdir().unwrap();
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command
            .current_dir(directory.path())
            .args([".", "--reporters", reporter, "--output"])
            .arg(output.path())
            .assert()
            .code(2)
            .stderr(predicate::str::contains("definition source"))
            .stderr(predicate::str::contains("input limit"));
    }
}

/// Checks explicit exclusion removes oversized sources before input-limit enforcement.
#[test]
fn explicitly_excluded_oversized_inputs_are_outside_the_analysis_corpus() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"exclude":["generated/**"]}"#,
    );
    write_sized(
        directory.path(),
        "generated/oversized.ts",
        8 * 1024 * 1024 + 1,
    );
    write(
        directory.path(),
        "steps.ts",
        "Given('included step', () => work());\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--threshold", "100"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Analyzed 1 definition"))
        .stderr(predicate::str::contains("input limit").not());
}

/// Checks regex resource and Gherkin errors are both reported while extracted definitions remain in JSON output.
#[test]
fn pathological_matcher_programs_fail_with_an_actionable_resource_error() {
    let directory = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given(/a{1000000}/, () => work());\n",
    );
    write(
        directory.path(),
        "broken.feature",
        "Scenario: Missing feature\n  Given a step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--reporters", "json", "--output"])
        .arg(output.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("regex resource limit"))
        .stderr(predicate::str::contains("simplify the matcher"))
        .stderr(predicate::str::contains("failed to parse Gherkin feature"));
    let report: Value =
        serde_json::from_str(&fs::read_to_string(output.path().join("cuke-dedup.json")).unwrap())
            .unwrap();
    assert_eq!(report["summary"]["definitionsAnalyzed"], 1);
}

/// Checks an oversized expression warns about regex resources and retains its definition in the default report.
#[test]
fn oversized_cucumber_expression_fails_closed_and_still_writes_a_report() {
    let directory = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        &format!("Given('{}', () => work());\n", "a".repeat(1024 * 1024 + 1)),
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--reporters", "json", "--output"])
        .arg(output.path())
        .assert()
        .code(0)
        .stderr(predicate::str::contains("warning: step matcher at"))
        .stderr(predicate::str::contains("regex resource limit"));
    let report: Value =
        serde_json::from_str(&fs::read_to_string(output.path().join("cuke-dedup.json")).unwrap())
            .unwrap();
    assert_eq!(report["summary"]["definitionsAnalyzed"], 1);
}

/// Checks a local function named Given is not promoted into a framework registration.
#[test]
fn local_domain_functions_are_not_mistaken_for_registration_barrels() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "support/domain.ts",
        "export function Given(name: string, handler: () => void) { return [name, handler]; }\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from './support/domain';\nGiven('not a step registration', () => work());\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--threshold", "100"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Analyzed 0 definitions"));
}

/// Checks chained registration re-exports retain renamed and namespace access forms.
#[test]
fn chained_local_barrel_reexports_preserve_aliases_and_namespaces() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "support/framework.ts",
        "export { Given as Setup, Then } from '@cucumber/cucumber';\n",
    );
    write(
        directory.path(),
        "support/index.ts",
        "export { Setup, Then } from './framework';\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "import * as bdd from './support';\nbdd.Setup('first step', () => first());\nbdd.Then('second step', () => second());\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--threshold", "100"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Analyzed 2 definitions"));
}

/// Checks an explicit config supplies threshold and feature settings ahead of autodiscovered configuration.
#[test]
fn explicit_config_flag_overrides_auto_discovery() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"threshold":0,"features":["missing/**/*.feature"]}"#,
    );
    write(
        directory.path(),
        "team.json",
        r#"{"threshold":100,"features":["features/**/*.feature"]}"#,
    );
    write(
        directory.path(),
        "steps.ts",
        "Given('same', () => first());\nGiven('same', () => second());\n",
    );
    write(
        directory.path(),
        "features/example.feature",
        "Feature: Config\n  Scenario: Explicit\n    Given same\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--config", "team.json"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("threshold 100.00% — PASS"));
}

/// Checks static Cypress configuration supplies a feature pattern and discovery explanation.
#[test]
fn cypress_project_config_supplies_feature_discovery() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "package.json",
        r#"{"devDependencies":{"@badeball/cypress-cucumber-preprocessor":"1.0.0"}}"#,
    );
    write(
        directory.path(),
        "cypress.config.ts",
        "export default defineConfig({ e2e: { specPattern: 'acceptance/**/*.spec' } });\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@badeball/cypress-cucumber-preprocessor';\nGiven('Cypress discovery works', () => cy.visit('/'));\n",
    );
    write(
        directory.path(),
        "acceptance/example.spec",
        "Feature: Cypress\n  Scenario: Configuration\n    Given Cypress discovery works\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--explain-discovery"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Analyzed 1 definition and 1 feature step",
        ))
        .stderr(predicate::str::contains("Cypress Cucumber configuration"));
}

/// Checks missing features disable unused findings by default and fail when features are required.
#[test]
fn missing_feature_corpus_is_safe_by_default_and_can_be_required() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('not necessarily unused', () => work());\n",
    );

    let mut safe = Command::cargo_bin("cuke-dedup").unwrap();
    safe.current_dir(directory.path())
        .arg(".")
        .assert()
        .success()
        .stdout(predicate::str::contains("unused-definition").not())
        .stderr(predicate::str::contains(
            "unused-definition findings are disabled",
        ));

    let mut required = Command::cargo_bin("cuke-dedup").unwrap();
    required
        .current_dir(directory.path())
        .args([".", "--require-features"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no feature files matched"));
}

/// Checks requiring features reports the count of unparseable discovered feature files.
#[test]
fn partial_feature_parse_failure_is_summarized_when_features_are_required() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('working step', () => work());\n",
    );
    write(
        directory.path(),
        "valid.feature",
        "Feature: Valid\n  Scenario: One\n    Given working step\n",
    );
    write(directory.path(), "invalid.feature", "Feature broken\n");

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--require-features"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "1 discovered feature file(s) could not be parsed",
        ));
}

/// Checks unmatched definition patterns and suppressions both emit diagnostics.
#[test]
fn unmatched_definition_patterns_and_suppressions_are_visible() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{
  "definitions": ["missing/**/*.ts"],
  "suppressions": [{
    "rule": "duplicate-matcher",
    "matcher": "missing step",
    "reason": "migration exception"
  }]
}"#,
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "definition pattern `missing/**/*.ts` matched no files",
        ))
        .stderr(predicate::str::contains(
            "suppression 1 for duplicate-matcher matched no step definitions",
        ));
}

/// Checks baseline fingerprints survive file relocation and fail the new-finding gate after a new duplicate appears.
#[test]
fn semantic_baseline_can_be_updated_moved_and_gated_by_new_findings() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('same step', () => { first(); });\nGiven('same step', () => { second(); });\n",
    );
    let mut update = Command::cargo_bin("cuke-dedup").unwrap();
    update
        .current_dir(directory.path())
        .args([
            ".",
            "--baseline",
            ".cuke-dedup-baseline.json",
            "--update-baseline",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("updated baseline"));
    let baseline: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join(".cuke-dedup-baseline.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(baseline["schemaVersion"], 3);
    assert_eq!(
        baseline["fingerprints"]
            .as_object()
            .unwrap()
            .values()
            .map(Value::as_u64)
            .sum::<Option<u64>>(),
        Some(1)
    );

    fs::create_dir(directory.path().join("moved")).unwrap();
    fs::rename(
        directory.path().join("steps.ts"),
        directory.path().join("moved/renamed.ts"),
    )
    .unwrap();

    let mut with_baseline = Command::cargo_bin("cuke-dedup").unwrap();
    with_baseline
        .current_dir(directory.path())
        .args([
            ".",
            "--baseline",
            ".cuke-dedup-baseline.json",
            "--reporters",
            "terminal",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 suppressed"))
        .stdout(predicate::str::contains(
            "Duplication: 0 of 2 definitions (0.00%), threshold 0.00% — PASS",
        ));

    write(
        directory.path(),
        "moved/third.ts",
        "Given('same step', () => { first(); });\n",
    );
    let mut gated = Command::cargo_bin("cuke-dedup").unwrap();
    gated
        .current_dir(directory.path())
        .args([
            ".",
            "--baseline",
            ".cuke-dedup-baseline.json",
            "--fail-on-new",
        ])
        .assert()
        .code(1);
}

/// Checks baseline updates migrate supported older schemas and reject future schema versions.
#[test]
fn baseline_update_replaces_older_versions_but_refuses_unknown_future_schemas() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('same step', () => first());\nGiven('same step', () => second());\n",
    );
    let baseline_path = directory.path().join("baseline.json");
    for version in [1, 2] {
        fs::write(
            &baseline_path,
            format!(r#"{{"schemaVersion":{version},"fingerprints":{{"legacy":1}}}}"#),
        )
        .unwrap();

        let mut migrate = Command::cargo_bin("cuke-dedup").unwrap();
        migrate
            .current_dir(directory.path())
            .args([".", "--baseline", "baseline.json", "--update-baseline"])
            .assert()
            .success()
            .stderr(predicate::str::contains("updated baseline"));
        let migrated: Value =
            serde_json::from_str(&fs::read_to_string(&baseline_path).unwrap()).unwrap();
        assert_eq!(migrated["schemaVersion"], 3);
    }

    fs::write(&baseline_path, r#"{"schemaVersion":99,"fingerprints":{}}"#).unwrap();
    let mut future = Command::cargo_bin("cuke-dedup").unwrap();
    future
        .current_dir(directory.path())
        .args([".", "--baseline", "baseline.json", "--update-baseline"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("newer than supported"));
}

/// Checks missing baseline arguments and changed-mode baseline updates are rejected.
#[test]
fn baseline_workflow_rejects_unsafe_or_incomplete_flag_combinations() {
    let directory = tempfile::tempdir().unwrap();
    for arguments in [
        vec![".", "--update-baseline"],
        vec![".", "--fail-on-new"],
        vec![
            ".",
            "--baseline",
            "baseline.json",
            "--update-baseline",
            "--changed-since",
            "HEAD",
        ],
    ] {
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command
            .current_dir(directory.path())
            .args(arguments)
            .assert()
            .code(2);
    }
}

/// Checks a reporter flag before the target preserves argument parsing and announces the written report.
#[test]
fn reporter_flag_does_not_swallow_the_target_and_announces_file_output() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('a step', () => { run(); });\n",
    );
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args(["--reporters", "json", "."])
        .assert()
        .success()
        .stdout(predicate::str::contains("Reports written to:"))
        .stdout(predicate::str::contains("cuke-dedup.json"));
}

/// Checks a Unicode changed path is compared with unchanged definitions and honors duplication thresholds.
#[test]
fn changed_since_compares_a_unicode_changed_source_against_the_full_corpus() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps/existing.ts",
        "Given('shared step', () => { existing(); });\n",
    );
    init_repository(directory.path());
    write(
        directory.path(),
        "steps/café changed.ts",
        "Given('shared step', () => { changed(); });\n",
    );
    assert!(fixture_git()
        .args(["add", "steps/café changed.ts"])
        .current_dir(directory.path())
        .status()
        .unwrap()
        .success());

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("duplicate-matcher"))
        .stdout(predicate::str::contains("steps/café changed.ts"));

    let mut tolerated = Command::cargo_bin("cuke-dedup").unwrap();
    tolerated
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD", "--threshold", "100"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Duplication: 2 of 2 definitions (100.00%), threshold 100.00% — PASS",
        ));
}

/// Checks option-like changed revisions are rejected before they can alter Git filtering.
#[test]
fn changed_since_rejects_option_like_revisions_instead_of_weakening_the_gate() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('shared step', () => first());\nGiven('shared step', () => second());\n",
    );
    init_repository(directory.path());

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg(".")
        .arg("--changed-since=--diff-filter=X")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid --changed-since revision"));
}

/// Checks changed analysis finds Unicode untracked definitions beneath an analysis subdirectory.
#[test]
fn changed_since_handles_a_unicode_untracked_file_from_a_repo_subdirectory() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "packages/e2e/steps/existing.ts",
        "Given('shared step', () => { existing(); });\n",
    );
    init_repository(directory.path());
    write(
        directory.path(),
        "packages/e2e/steps/café step.ts",
        "Given('shared step', () => { changed(); });\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args(["packages/e2e", "--changed-since", "HEAD"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("duplicate-matcher"))
        .stdout(predicate::str::contains("steps/café step.ts"));
}

/// Checks newline-containing untracked paths participate in changed-mode duplicate detection.
#[cfg(unix)]
#[test]
fn changed_since_handles_an_untracked_file_with_a_newline() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps/existing.ts",
        "Given('shared step', () => { existing(); });\n",
    );
    init_repository(directory.path());
    write(
        directory.path(),
        "steps/line\nbreak.ts",
        "Given('shared step', () => { changed(); });\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("duplicate-matcher"));
}

/// Checks Linux non-UTF-8 untracked paths participate in changed-mode duplicate detection.
#[cfg(target_os = "linux")]
#[test]
fn changed_since_handles_an_untracked_non_utf8_file() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps/existing.ts",
        "Given('shared step', () => { existing(); });\n",
    );
    init_repository(directory.path());
    let path = directory
        .path()
        .join("steps")
        .join(OsString::from_vec(b"non-utf8-\xff.ts".to_vec()));
    fs::write(path, "Given('shared step', () => { changed(); });\n").unwrap();

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("duplicate-matcher"));
}

/// Checks changed analysis rejects a root ignored by Git.
#[test]
fn changed_since_rejects_a_gitignored_analysis_root() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), ".gitignore", "ignored/\n");
    write(
        directory.path(),
        "ignored/steps.ts",
        "Given('ignored step', () => work());\n",
    );
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
        vec!["add", ".gitignore"],
        vec!["commit", "-qm", "initial"],
    ] {
        assert!(fixture_git()
            .args(args)
            .current_dir(directory.path())
            .status()
            .unwrap()
            .success());
    }

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args(["ignored", "--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ignored by Git"));
}

/// Checks changed mode disables unused findings but still fails when no discovered feature parses.
#[test]
fn changed_since_disables_unused_findings_but_fails_when_no_feature_parses() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "vendor/broken.feature",
        "Scenario: Missing feature\n  Given a step\n",
    );
    init_repository(directory.path());
    write(
        directory.path(),
        "steps/new.ts",
        "Given('new step', () => work());\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([
            ".",
            "--changed-since",
            "HEAD",
            "--rule",
            "unused-definition=error",
        ])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("unused-definition").not())
        .stderr(predicate::str::contains(
            "no discovered feature file was parsed successfully",
        ))
        .stderr(predicate::str::contains("failed to parse Gherkin feature"));
}

/// Checks an early-closing JSONL pipe terminates successfully under shell pipefail.
#[cfg(unix)]
#[test]
fn closed_jsonl_consumer_is_a_clean_termination() {
    let directory = tempfile::tempdir().unwrap();
    let mut definitions = String::new();
    for index in 0..120 {
        definitions.push_str(&format!("Given('same step', () => handler_{index}());\n"));
    }
    write(directory.path(), "steps.ts", &definitions);
    let binary = assert_cmd::cargo::cargo_bin!("cuke-dedup");
    let output = ProcessCommand::new("bash")
        .args([
            "-o",
            "pipefail",
            "-c",
            "\"$CUKE_BINARY\" \"$CUKE_ROOT\" --reporters jsonl --threshold 100 | head -n 1 >/dev/null",
        ])
        .env("CUKE_BINARY", binary)
        .env("CUKE_ROOT", directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Checks large handler classes emit bounded findings with complete cluster membership and no report truncation.
#[test]
fn large_equivalence_class_emits_a_bounded_spanning_finding_set() {
    const PAIR_RULE_COUNT: usize = 5;

    let directory = tempfile::tempdir().unwrap();
    let mut definitions = String::new();
    let definition_count = 150;
    for index in 0..definition_count {
        definitions.push_str(&format!(
            "Given('legacy operation {index}', () => sharedImplementation());\n"
        ));
    }
    write(directory.path(), "steps.ts", &definitions);
    let report_directory = tempfile::tempdir().unwrap();

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--reporters", "json", "--threshold", "100", "--output"])
        .arg(report_directory.path())
        .assert()
        .success();

    let report: Value = serde_json::from_str(
        &fs::read_to_string(report_directory.path().join("cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["summary"]["definitionsAnalyzed"], definition_count);
    assert!(
        report["summary"]["findings"].as_u64().unwrap()
            <= (PAIR_RULE_COUNT * (definition_count - 1)) as u64
    );
    let duplicate_handler_cluster = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["rule"] == "duplicate-handler")
        .unwrap();
    assert_eq!(
        duplicate_handler_cluster["related"]
            .as_array()
            .unwrap()
            .len(),
        definition_count - 1
    );
    assert_eq!(
        duplicate_handler_cluster["evidence"]["cluster"]["definitionFingerprints"]
            .as_array()
            .unwrap()
            .len(),
        definition_count
    );
    assert_eq!(
        duplicate_handler_cluster["evidence"]["cluster"]["pairFindingsCollapsed"],
        definition_count - 1
    );
    assert_eq!(report["findingsTruncated"], 0);
}

/// Checks package configuration controls discovery, output, disabled rules, and reported suppression reasons.
#[test]
fn package_json_configuration_and_suppression_apply_end_to_end() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "package.json",
        r#"{
  "cukeDedup": {
    "definitions": ["specs/**/*.ts"],
    "features": ["specs/**/*.feature"],
    "reporters": ["json"],
    "output": "configured-reports",
    "rules": {"ambiguous-step": "off"},
    "suppressions": [{
      "rule": "duplicate-matcher",
      "matcher": "same step",
      "reason": "intentional compatibility alias"
    }]
  }
}"#,
    );
    write(
        directory.path(),
        "specs/steps.ts",
        "Given('same step', () => first());\nGiven('same step', () => second());\n",
    );
    write(
        directory.path(),
        "specs/example.feature",
        "Feature: Config\n  Scenario: One\n    Given same step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "configured-reports/cuke-dedup.json",
        ));

    let report: Value = serde_json::from_str(
        &fs::read_to_string(directory.path().join("configured-reports/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(report["summary"]["suppressed"], 1);
    assert_eq!(
        report["findings"][0]["suppression"]["reason"],
        "intentional compatibility alias"
    );
}

/// Checks explicit discovery flags include hidden sources and normally excluded dependency directories.
#[test]
fn discovery_flags_can_include_hidden_and_default_excluded_sources() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".steps/hidden.ts",
        "Given('hidden step', () => hidden());\n",
    );
    write(
        directory.path(),
        "node_modules/workspace/nested.ts",
        "Given('nested step', () => nested());\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([
            ".",
            "--include-hidden",
            "--no-default-excludes",
            "--definitions",
            ".steps/**/*.ts,node_modules/workspace/**/*.ts",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Analyzed 2 definitions"));
}

/// Checks JSON output is written beneath the default report directory.
#[test]
fn json_report_uses_the_documented_default_output_directory() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('one step', () => work());\n",
    );
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--reporters", "json"])
        .assert()
        .success();
    assert!(directory
        .path()
        .join("reports/cuke-dedup/cuke-dedup.json")
        .is_file());
}

/// Checks JSONL stdout contains finding records followed by a summary, without terminal output announcements.
#[test]
fn jsonl_report_streams_only_machine_records_to_stdout() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('same step', () => first());\nGiven('same step', () => second());\n",
    );
    write(
        directory.path(),
        "example.feature",
        "Feature: Example\n  Scenario: One\n    Given same step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    let output = command
        .current_dir(directory.path())
        .args([".", "--reporters", "jsonl"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let records = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert!(records.len() > 1);
    assert!(records[..records.len() - 1]
        .iter()
        .all(|record| record["type"] == "finding"));
    assert_eq!(records.last().unwrap()["type"], "summary");
    assert!(records.last().unwrap()["metrics"]["filesDiscovered"]
        .as_u64()
        .is_some_and(|count| count == 2));
    assert_eq!(
        records.last().unwrap()["corpus"]["definitionFilesWithDefinitions"],
        1
    );
    assert_eq!(records.last().unwrap()["corpus"]["definitionsExtracted"], 2);
    assert_eq!(records.last().unwrap()["corpus"]["featureFilesParsed"], 1);
    assert!(!stdout.contains("Reports written to:"));
}

/// Checks an empty changed selection does not hide fatal unchanged-feature parse errors.
#[test]
fn changed_since_fails_closed_on_unchanged_feature_parse_errors() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "broken.feature",
        "Scenario: Missing feature\n  Given a step\n",
    );
    init_repository(directory.path());

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "found no tracked or untracked files",
        ))
        .stderr(predicate::str::contains("failed to parse Gherkin feature"));
}

/// Checks the explicit check subcommand rejects a missing target path.
#[test]
fn bare_check_subcommand_requires_an_explicit_path() {
    let directory = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg("check")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "required arguments were not provided",
        ));
}

/// Checks unsupported matcher and invalid escape diagnostics retain independently extracted definitions.
#[test]
fn source_diagnostics_are_visible_without_discarding_valid_definitions() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.js",
        r#"
Given(/value (?=ahead)/, () => advanced());
Given('broken \xZZ', () => broken());
Given('valid step', () => valid());
"#,
    );
    write(
        directory.path(),
        "example.feature",
        "Feature: Partial\n  Scenario: Valid\n    Given valid step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(2)
        .stdout(predicate::str::contains("Analyzed 2 definitions"))
        .stderr(predicate::str::contains(
            "unsupported by static usage analysis",
        ))
        .stderr(predicate::str::contains("invalid JavaScript escape"));
}

/// Checks JSX extension guidance survives tolerant and strict parsing and renaming to TSX restores extraction.
#[test]
fn malformed_definition_sources_are_reported_with_actionable_extension_guidance() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('JSX step', () => <section>ready</section>);\n",
    );

    // By default the parse failure is tolerated but reported, and the report still carries the
    // actionable extension guidance rather than aborting the run.
    let mut malformed = Command::cargo_bin("cuke-dedup").unwrap();
    malformed
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .success()
        .stderr(predicate::str::contains("analysis is incomplete"))
        .stderr(predicate::str::contains("use a .tsx extension"));

    // `--fail-on-unparseable` restores the hard stop, keeping the same guidance.
    let mut strict = Command::cargo_bin("cuke-dedup").unwrap();
    strict
        .current_dir(directory.path())
        .args([".", "--fail-on-unparseable"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("use a .tsx extension"));

    fs::rename(
        directory.path().join("steps.ts"),
        directory.path().join("steps.tsx"),
    )
    .unwrap();
    let mut corrected = Command::cargo_bin("cuke-dedup").unwrap();
    corrected
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .success()
        .stdout(predicate::str::contains("Analyzed 1 definition"));
}

/// Checks misplaced check arguments are rejected and explicit discovery filters select the intended corpus.
#[test]
fn path_before_check_is_rejected_and_discovery_flags_are_applied() {
    let directory = tempfile::tempdir().unwrap();
    let mut invalid = Command::cargo_bin("cuke-dedup").unwrap();
    invalid
        .current_dir(directory.path())
        .args(["somewhere", "check", "."])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "do not place a path before `check`",
        ));

    write(
        directory.path(),
        "selected/steps.ts",
        "Given('selected step', () => selected());\n",
    );
    write(
        directory.path(),
        "ignored/steps.ts",
        "Given('ignored step', () => ignored());\n",
    );
    write(
        directory.path(),
        "selected/example.feature",
        "Feature: Selected\n  Scenario: One\n    Given selected step\n",
    );
    write(
        directory.path(),
        "ignored/example.feature",
        "Feature: Ignored\n  Scenario: One\n    Given ignored step\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([
            ".",
            "--definitions",
            "selected/**/*.ts",
            "--features",
            "selected/**/*.feature",
            "--exclude",
            "ignored/**",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Analyzed 1 definition and 1 feature step",
        ));
}

/// Git hooks receive `GIT_DIR` and friends pointing at the repository that launched them. Those
/// variables outrank `-C`, so a leaked environment would silently make changed-files mode describe
/// the hook's repository instead of the analyzed root.
#[test]
fn changed_files_mode_ignores_an_inherited_git_environment() {
    let decoy = tempfile::tempdir().unwrap();
    write(decoy.path(), "sentinel.txt", "sentinel\n");
    init_repository(decoy.path());

    // The analyzed root is deliberately not a repository, so the only way this can succeed is by
    // resolving the decoy from the environment.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('shared step', () => work());\n",
    );

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .env("GIT_DIR", decoy.path().join(".git"))
        .env("GIT_WORK_TREE", decoy.path())
        .env("GIT_INDEX_FILE", decoy.path().join(".git/index"))
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "changed-files mode requires a Git repository",
        ));

    // The decoy's index must be untouched by the analysis.
    let tracked = fixture_git()
        .args(["ls-files"])
        .current_dir(decoy.path())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&tracked.stdout), "sentinel.txt\n");
}

/// The recall corpus filters suppressed findings before evaluating expectations, so a suppressed
/// `ambiguous-step` finding is indistinguishable there from one that was never generated. Severity
/// and suppression therefore have to be pinned here instead.
#[test]
fn ambiguous_step_severity_and_suppression_are_distinguishable_from_absence() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('the gauge reads {word}', () => parameterized());\nGiven('the gauge reads high', () => literal());\n",
    );
    write(
        directory.path(),
        "features/gauge.feature",
        "Feature: Gauge\n  Scenario: One\n    Given the gauge reads high\n",
    );

    // At `warning` the finding is still reported, and the run succeeds: `off` alone would not show
    // that severity is read rather than treated as a boolean.
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"rules": {"ambiguous-step": "warning"}, "reporters": ["json", "terminal"], "output": "reports"}"#,
    );
    let mut warned = Command::cargo_bin("cuke-dedup").unwrap();
    warned
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .success()
        .stdout(predicate::str::contains("[warning]"))
        .stdout(predicate::str::contains("ambiguous-step"));

    // Suppressed: the finding is retained in the report carrying its reason, not dropped.
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"reporters": ["json", "terminal"], "output": "reports", "suppressions": [{"rule": "ambiguous-step", "matcher": "the gauge reads high", "reason": "the literal wins at runtime"}]}"#,
    );
    let mut suppressed = Command::cargo_bin("cuke-dedup").unwrap();
    suppressed
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 suppressed"));

    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(directory.path().join("reports/cuke-dedup.json")).unwrap(),
    )
    .unwrap();
    let ambiguities: Vec<_> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["rule"] == "ambiguous-step")
        .collect();
    assert_eq!(ambiguities.len(), 1);
    assert_eq!(
        ambiguities[0]["suppression"]["reason"],
        "the literal wins at runtime"
    );
    assert_eq!(report["summary"]["findings"], 0);
}

/// Analyzes `root` with default options and returns the assertion handle.
fn analyze_root(root: &Path) -> assert_cmd::assert::Assert {
    analyze_root_with(root, &[])
}

/// Analyzes `root` with extra arguments and returns the assertion handle.
fn analyze_root_with(root: &Path, arguments: &[&str]) -> assert_cmd::assert::Assert {
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command.current_dir(root).arg(".").args(arguments).assert()
}

/// Checks minified and binary exclusions are named while remaining authored definitions are analyzed.
#[test]
fn generated_bundles_are_excluded_and_reported_without_failing_the_run() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('an analyzed step', () => work());\n",
    );
    write(
        directory.path(),
        "public/vendor.js",
        &format!("!function(n){{{}}}(0);\n", "var a=1,b=2,c=3;".repeat(500)),
    );
    let mut binary = b"var a = 1;".to_vec();
    binary.push(0);
    fs::write(directory.path().join("public/blob.js"), binary).unwrap();

    analyze_root(directory.path())
        .code(0)
        .stderr(predicate::str::contains("public/vendor.js (minified)"))
        .stderr(predicate::str::contains("public/blob.js (binary)"))
        // The run still analyzes what remains: an exclusion is not a failure, and the surviving
        // source must not be lost alongside the generated one.
        .stderr(predicate::str::contains("produced 0 definitions").not());
}

/// Checks bounded bundle classification excludes minified input before whole-file size enforcement.
#[test]
fn an_oversized_bundle_is_excluded_rather_than_failing_the_input_limit() {
    // Classification reads a bounded prefix precisely so that a bundle larger than the input
    // limit is cut instead of ending the run. Reading the whole file first would make the
    // largest vendored bundles — the ones most worth excluding — a hard error instead.
    let directory = tempfile::tempdir().unwrap();
    let line = "var a=1,b=2,c=3;".repeat(500);
    let mut bundle = String::from("/*! vendored bundle */\n");
    while bundle.len() < 9 * 1024 * 1024 {
        bundle.push_str(&line);
    }
    write(directory.path(), "public/huge.js", &bundle);
    write(
        directory.path(),
        "steps.ts",
        "Given('an analyzed step', () => work());\n",
    );

    analyze_root(directory.path())
        .code(0)
        .stderr(predicate::str::contains("public/huge.js (minified)"))
        .stderr(predicate::str::contains("input limit").not());
}

/// Checks recognizable step registrations prevent minified-source exclusion.
#[test]
fn a_bundle_that_registers_steps_is_analyzed_rather_than_excluded() {
    // The opposite-answer control for exclusion: identical geometry to the bundle above, but it
    // registers steps, so it must be analyzed and its duplicate reported.
    let directory = tempfile::tempdir().unwrap();
    let filler = "var a=1,b=2,c=3;".repeat(500);
    write(
        directory.path(),
        "bundled.js",
        &format!(
            "{filler}Given('a bundled step', () => work());Given('a bundled step', () => work());{filler}"
        ),
    );

    analyze_root(directory.path()).stderr(predicate::str::contains("bundled.js (minified)").not());
}

/// Checks heuristic minified exclusion marks the corpus incomplete and fails strict gating.
#[test]
fn a_minified_exclusion_makes_the_corpus_incomplete_so_strict_gates_fail_closed() {
    // A minified exclusion is a heuristic decision, so it must never let a gate pass while a
    // discovered file went unanalyzed. This file registers steps through a renaming import whose
    // call site is a bare `G(...)`, then has its module path stripped, leaving nothing the prefix
    // scan can recognize — the residual uncertainty the exclusion cannot rule out.
    let directory = tempfile::tempdir().unwrap();
    let filler = "var a=1,b=2,c=3;".repeat(500);
    write(
        directory.path(),
        "bundle.js",
        &format!("{filler}G('a step', () => work());G('a step', () => work());{filler}"),
    );
    write(
        directory.path(),
        "features/example.feature",
        "Feature: f\n  Scenario: s\n    Given a step\n",
    );

    analyze_root_with(directory.path(), &["--fail-on-incomplete"])
        .code(2)
        .stderr(predicate::str::contains("bundle.js (minified)"))
        .stderr(predicate::str::contains("corpus is incomplete"));
}

/// Checks an authored corpus without exclusions passes strict completeness without exclusion warnings.
#[test]
fn a_run_with_nothing_excluded_stays_complete() {
    // The opposite-answer control for the case above. Every content signal is a heuristic, so any
    // exclusion marks the corpus incomplete; without this control, a rule that marked every run
    // incomplete would pass the case above and look correct.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "Given('an analyzed step', () => work());\n",
    );
    write(
        directory.path(),
        "features/example.feature",
        "Feature: f\n  Scenario: s\n    Given an analyzed step\n",
    );

    analyze_root_with(directory.path(), &["--fail-on-incomplete"])
        .code(0)
        .stderr(predicate::str::contains("excluded").not())
        .stderr(predicate::str::contains("corpus is incomplete").not());
}

/// Checks binary exclusion remains incomplete even when a NUL appears only in a source comment.
#[test]
fn a_binary_exclusion_also_makes_the_corpus_incomplete() {
    // A NUL byte is not proof that a file holds no definitions: JavaScript permits one inside a
    // comment, and such a file extracts normally once the exclusion is lifted. So a binary
    // exclusion cannot report a complete corpus either.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "features/example.feature",
        "Feature: f\n  Scenario: s\n    Given a step\n",
    );
    let mut source = b"const { Given } = require('@cucumber/cucumber');\n// ".to_vec();
    source.push(0);
    source.extend_from_slice(b"\nGiven('a step', () => work());\n");
    fs::write(directory.path().join("steps.js"), source).unwrap();

    analyze_root_with(directory.path(), &["--fail-on-incomplete"])
        .code(2)
        .stderr(predicate::str::contains("steps.js (binary)"))
        .stderr(predicate::str::contains("corpus is incomplete"));
}

/// Checks exclusion diagnostics bound the listed paths and count the remaining entries.
#[test]
fn a_long_exclusion_list_is_summarized_rather_than_printed_in_full() {
    // A repository can vendor more generated files than a diagnostic should name, so the list is
    // bounded and the remainder counted.
    let directory = tempfile::tempdir().unwrap();
    let bundle = format!("!function(n){{{}}}(0);\n", "var a=1,b=2,c=3;".repeat(500));
    for index in 0..12 {
        write(
            directory.path(),
            &format!("public/vendor{index}.js"),
            &bundle,
        );
    }
    write(
        directory.path(),
        "steps.ts",
        "Given('an analyzed step', () => work());\n",
    );

    analyze_root(directory.path())
        .code(0)
        .stderr(predicate::str::contains("excluded 12 discovered"))
        .stderr(predicate::str::contains(", and 2 more"));
}

// ===========================================================================================
// KNOWN-ISSUE REGRESSIONS
//
// Each shipped bug and each accepted reviewer finding earns one permanent case here, exercised
// through the built binary end to end (not only a unit test), and named
// `regression_<pr-or-issue>_<slug>`. A unit test confirms what its author thought to check; this
// tier is what stops a known class from silently returning through the whole tool. Add a case here
// as part of fixing the finding, and confirm it fails before the fix and passes after.
// ===========================================================================================

/// Checks ambient, configured, aliased, and forwarded registrations preserve complete handler findings across frameworks.
#[test]
fn regression_framework_review_fallback_registrations_preserve_outcomes() {
    for package in ["jest-cucumber", "vitest-cucumber-plugin"] {
        for (callee, setup) in [
            ("Given", ""),
            ("step", ""),
            ("alias", "const alias = Given;"),
            ("wrapper", "function wrapper(p, h) { Given(p, h); }"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            write(
                directory.path(),
                ".cuke-dedup.json",
                r#"{"registrations":["step"],"threshold":100}"#,
            );
            write(directory.path(), "steps.ts", &format!("import '{package}'; {setup} {callee}('first action', () => work()); {callee}('second action', () => work());"));
            write(directory.path(), "steps.feature", "Feature: fallback\n Scenario: controls\n  Given first action\n  Then second action\n");
            analyze_root_with(directory.path(), &["--fail-on-incomplete"])
                .code(0)
                .stdout(predicate::str::contains("Analyzed 2 definitions"))
                .stdout(predicate::str::contains("duplicate-handler"));
        }
    }
}

/// Checks configured registration imports preserve assertion provenance and distinguish ordinary literal variation from trusted conflicts.
#[test]
fn regression_framework_review_configured_import_does_not_trust_custom_assertions() {
    for module in ["./helpers", "@jest/globals"] {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            ".cuke-dedup.json",
            r#"{"registrations":["step"],"threshold":100}"#,
        );
        write(
            directory.path(),
            "helpers.ts",
            "export const step = customStep; export const expect = customExpect;",
        );
        write(directory.path(), "steps.ts", &format!("import 'vitest-cucumber-plugin'; import {{step}} from './helpers'; import {{expect}} from '{module}'; step('first action', () => expect(value).toBe(1)); step('second action', () => expect(value).toBe(2));"));
        write(
            directory.path(),
            "steps.feature",
            "Feature: trust\n Scenario: control\n  Given first action\n  Then second action\n",
        );
        let result = analyze_root_with(directory.path(), &["--fail-on-incomplete"])
            .code(0)
            .stdout(predicate::str::contains("Analyzed 2 definitions"))
            .stdout(predicate::str::contains("duplicate-handler").not());
        if module == "./helpers" {
            result.stdout(predicate::str::contains("parameterization-candidate"));
        } else {
            result.stdout(predicate::str::contains("parameterization-candidate").not());
        }
    }
}

/// Checks transparent callee syntax preserves forwarding identity without accepting reversed operands or conflicting assertions.
#[test]
fn regression_framework_review_wrapped_callees_keep_registration_identity() {
    for callee in [
        "given",
        "(given)",
        "(given as typeof given)",
        "(given satisfies typeof given)",
        "given!",
        "((given as typeof given)!)",
    ] {
        for framework in [
            "@cucumber/cucumber",
            "vitest-cucumber-plugin",
            "jest-cucumber",
        ] {
            for (arguments, second_handler, definitions, duplicate) in [
                ("p, h", "() => { prepare(); work(); }", 2, true),
                ("p, h", "() => expect(value).toBe(2)", 2, false),
                ("h, p", "() => { prepare(); work(); }", 0, false),
            ] {
                let directory = tempfile::tempdir().unwrap();
                let first_handler = if second_handler.contains("expect") {
                    "() => expect(value).toBe(1)"
                } else {
                    second_handler
                };
                let body = format!("function step(p, h) {{ {callee}({arguments}); }} step('first action', {first_handler}); step('second action', {second_handler});");
                let source = if framework == "jest-cucumber" {
                    format!("import {{defineFeature}} from '{framework}'; defineFeature(feature, test => test('one', ({{given}}) => {{ {body} }}));")
                } else {
                    format!("import {{Given as given}} from '{framework}'; {body}")
                };
                write(
                    directory.path(),
                    "steps.ts",
                    &format!("import {{expect}} from '@jest/globals'; {source}"),
                );
                write(directory.path(), "steps.feature", "Feature: callee\n Scenario: controls\n  Given first action\n  Then second action\n");
                analyze_root_with(
                    directory.path(),
                    &[
                        "--threshold",
                        "100",
                        "--reporters",
                        "json",
                        "--output",
                        "report",
                    ],
                )
                .code(0);
                let report: Value = serde_json::from_slice(
                    &fs::read(directory.path().join("report/cuke-dedup.json")).unwrap(),
                )
                .unwrap();
                let context = format!("{framework}: {callee}({arguments}), {second_handler}");
                assert_eq!(
                    report["summary"]["definitionsAnalyzed"], definitions,
                    "{context}"
                );
                assert_eq!(
                    report["summary"]["byRule"]["duplicate-handler"]
                        .as_u64()
                        .unwrap_or(0),
                    u64::from(duplicate),
                    "{context}"
                );
                assert!(
                    report["summary"]["byRule"]["parameterization-candidate"].is_null(),
                    "{context}"
                );
                assert!(
                    report["summary"]["byRule"]["near-duplicate-step"].is_null(),
                    "{context}"
                );
                if definitions > 0 {
                    assert_eq!(report["corpus"]["incomplete"], false, "{context}");
                }
            }
        }
    }
}

/// Checks local CommonJS registration re-exports retain both definitions and their duplicate-matcher finding.
#[test]
fn regression_pr56_cjs_barrel_require_resolves_registrations_end_to_end() {
    // PR #55/#56: a project-local CommonJS barrel that re-exports a registration, imported by
    // `require`, must resolve so its definitions are analyzed. If barrel resolution regresses the
    // two `Given` calls register nothing — 0 definitions, no finding, exit 0 — so this exact
    // observable (2 definitions collapse to one duplicate-matcher finding) is the discriminator.
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "package.json", "{}\n");
    write(
        directory.path(),
        "barrel.js",
        "const { Given } = require('@cucumber/cucumber');\nmodule.exports = { Given };\n",
    );
    write(
        directory.path(),
        "steps.js",
        "const { Given } = require('./barrel');\nGiven('the same step', () => first());\nGiven('the same step', () => second());\n",
    );

    analyze_root(directory.path())
        .code(1)
        .stdout(predicate::str::contains("Analyzed 2 definitions"))
        .stdout(predicate::str::contains("duplicate-matcher"));
}

/// Checks unmodeled CommonJS exports warn by default and fail strict completeness.
#[test]
fn regression_pr55_unmodeled_cjs_export_fails_closed_end_to_end() {
    // PR #55: a CommonJS barrel whose export form the analyzer cannot model (here a call result)
    // must FAIL CLOSED — mark the corpus incomplete and name the unmodeled form — rather than
    // resolve silently to nothing. A regression would drop the registration with no warning and let
    // `--fail-on-incomplete` pass. The discriminator is the "CommonJS form the analyzer does not
    // model" warning plus exit 2 under `--fail-on-incomplete`; a silent resolution shows neither.
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "package.json", "{}\n");
    write(
        directory.path(),
        "barrel.js",
        "module.exports = makeApi();\n",
    );
    write(
        directory.path(),
        "steps.js",
        "const { Given } = require('./barrel');\nGiven('a step', () => work());\n",
    );

    analyze_root(directory.path())
        .code(0)
        .stderr(predicate::str::contains(
            "CommonJS form the analyzer does not model",
        ));
    analyze_root_with(directory.path(), &["--fail-on-incomplete"])
        .code(2)
        .stderr(predicate::str::contains("corpus is incomplete"));
}

/// Checks unavailable package config bases do not erase locally declared registration path aliases.
#[test]
fn regression_corpus_tsconfig_package_extends_keeps_path_aliases() {
    // Real-repo corpus: 11 projects' tsconfigs extend a package base (`@tsconfig/recommended/…`,
    // `expo/tsconfig.base`, …) that is not installed where the analyzer runs. That used to reject the
    // whole config, so no `paths` alias resolved and registrations behind an aliased barrel vanished
    // — here 0 definitions and a clean exit. Skipping the unavailable base keeps the project's own
    // alias working: both definitions resolve and the duplicate is reported.
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "package.json", "{}\n");
    write(
        directory.path(),
        "tsconfig.json",
        r#"{"extends":"@tsconfig/recommended/tsconfig.json","compilerOptions":{"baseUrl":".","paths":{"@/*":["./*"]}}}"#,
    );
    write(
        directory.path(),
        "support/bdd.ts",
        "export { Given } from '@cucumber/cucumber';\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@/support/bdd';\nGiven('the same step', () => first());\nGiven('the same step', () => second());\n",
    );

    analyze_root(directory.path())
        .code(1)
        .stdout(predicate::str::contains("Analyzed 2 definitions"))
        .stdout(predicate::str::contains("duplicate-matcher"));
}

/// Checks workspace tsconfig fields resolve a suffixless JSON base and its registration aliases.
#[test]
fn regression_pr62_workspace_tsconfig_field_names_a_suffixless_base() {
    // Review finding: a workspace package's `tsconfig` field names its base the way `extends` does,
    // so `./configs/base` means `configs/base.json`. Only the literal path was tried, the base
    // failed to resolve, and its alias with it — 0 definitions. The alias lives only in the base, so
    // the finding appears only when the field resolves with the `.json` suffix.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "package.json",
        r#"{"name":"root","workspaces":["packages/*"]}"#,
    );
    write(
        directory.path(),
        "packages/config/package.json",
        r#"{"name":"shared-config","tsconfig":"./configs/base"}"#,
    );
    write(
        directory.path(),
        "packages/config/configs/base.json",
        r#"{"compilerOptions":{"paths":{"@steps/*":["./support/*"]}}}"#,
    );
    write(
        directory.path(),
        "packages/config/configs/support/bdd.ts",
        "export { Given } from '@cucumber/cucumber';\n",
    );
    write(
        directory.path(),
        "tsconfig.json",
        r#"{"extends":"shared-config"}"#,
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@steps/bdd';\nGiven('the same step', () => first());\nGiven('the same step', () => second());\n",
    );

    analyze_root(directory.path())
        .code(1)
        .stdout(predicate::str::contains("Analyzed 2 definitions"))
        .stdout(predicate::str::contains("duplicate-matcher"));
}

/// Checks workspace config subpaths cannot escape their package to import unrelated aliases.
#[test]
fn regression_pr62_workspace_package_base_stays_inside_its_package() {
    // Security review finding: a known workspace package named by `extends` could reach any JSON in
    // the root through a traversing subpath, including an in-root `node_modules` base, and import
    // its aliases. The alias here exists only in that leaked base, so without the boundary both
    // definitions resolve and a duplicate is reported; with it the base is refused and reported.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "package.json",
        r#"{"name":"root","workspaces":["packages/*"]}"#,
    );
    write(
        directory.path(),
        "packages/config/package.json",
        r#"{"name":"shared-config"}"#,
    );
    write(
        directory.path(),
        "node_modules/leak/tsconfig.json",
        r#"{"compilerOptions":{"paths":{"@steps/*":["../../support/*"]}}}"#,
    );
    write(
        directory.path(),
        "tsconfig.json",
        r#"{"extends":"shared-config/../../node_modules/leak/tsconfig.json"}"#,
    );
    write(
        directory.path(),
        "support/bdd.ts",
        "export { Given } from '@cucumber/cucumber';\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@steps/bdd';\nGiven('the same step', () => first());\nGiven('the same step', () => second());\n",
    );

    analyze_root(directory.path())
        .stdout(predicate::str::contains("Analyzed 0 definitions"))
        .stderr(predicate::str::contains(
            "resolves outside package or uses an invalid segment",
        ));
}

/// Checks runtime default imports shadow ambient registrations while erased type-only imports do not.
#[test]
fn regression_corpus_default_import_does_not_become_an_ambient_registration() {
    // A default import binds its own local name, so `Given` here is the helper module's default
    // export, not the ambient registration global. Only named imports used to shadow the global,
    // so this invented a definition. The type-only control is erased at runtime, so there the
    // global does apply and the same call registers.
    for (import, definitions) in [
        ("import Given from './helpers';\n", "Analyzed 0 definitions"),
        (
            "import type Given from './helpers';\n",
            "Analyzed 1 definition",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "package.json", "{}\n");
        write(
            directory.path(),
            "helpers.js",
            "module.exports = { Given: (text, fn) => text };\n",
        );
        write(
            directory.path(),
            "steps.ts",
            &format!("{import}Given('a phantom step', () => work());\n"),
        );
        analyze_root(directory.path()).stdout(predicate::str::contains(definitions));
    }
}

/// Checks inert CommonJS helpers stay quiet while escaped or registration-capable exports warn.
#[test]
fn regression_corpus_inert_commonjs_export_is_not_an_incomplete_barrel() {
    // Real-repo corpus: 27 projects required local helpers written as `module.exports = async
    // function …`, a class, or a data array. Only object literals and `require` were modeled, so
    // each helper marked the corpus incomplete. A helper that names no registration is inert and
    // stays quiet; the control forwards to `Given`, a possible wrapper, and must still fail closed.
    // Review finding: an object that escapes into a call, even wrapped in another object, can be
    // given a registration there, so it must fail closed as well, and so must a function that
    // reaches a registration through a bracket key, a name assigned one later, or a member it
    // picks at runtime.
    for (helper, warns) in [
        (
            "module.exports = async function act(value) { return value; };\n",
            false,
        ),
        (
            "const { Given } = require('@cucumber/cucumber');\nmodule.exports = (text, fn) => Given(text, fn);\n",
            true,
        ),
        (
            "const api = {};\nattach({ api });\nmodule.exports = api;\n",
            true,
        ),
        (
            "module.exports = (bdd, text, fn) => bdd['Given'](text, fn);\n",
            true,
        ),
        (
            "let register;\nregister = Given;\nmodule.exports = { step: (t, f) => register(t, f) };\n",
            true,
        ),
        (
            "module.exports = (bdd, name, t, f) => bdd[name](t, f);\n",
            true,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "package.json", "{}\n");
        write(directory.path(), "action.js", helper);
        write(
            directory.path(),
            "steps.js",
            "const { Given } = require('@cucumber/cucumber');\nconst act = require('./action');\nGiven('a step', () => act(1));\n",
        );
        let assert = analyze_root(directory.path())
            .stdout(predicate::str::contains("Analyzed 1 definition"));
        let unmodeled = predicate::str::contains("CommonJS form the analyzer does not model");
        if warns {
            assert.stderr(unmodeled);
        } else {
            assert.stderr(unmodeled.not());
        }
    }
}

/// Checks passing registrations into helpers warns without confusing unrelated fixture parameters.
#[test]
fn regression_pr63_registration_passed_to_a_helper_is_reported() {
    // Review finding: a helper that calls a function it receives is inert on the export side, so
    // `helper(Given, …)` registered a step the analyzer never saw, with no warning. The call site
    // now reports it as incomplete, whether the registration is a direct argument or nested in an
    // object. The control passes a fixture parameter that is merely named `Given`, which is a local
    // value and must stay quiet.
    for (steps, reported) in [
        (
            "const { Given } = require('@cucumber/cucumber');\nconst helper = require('./helper');\nhelper(Given, 'a step', () => work());\n",
            true,
        ),
        (
            "const { Given } = require('@cucumber/cucumber');\nconst helper = require('./helper');\nhelper({ register: Given }, 'a step', () => work());\n",
            true,
        ),
        (
            "const { Given } = require('@cucumber/cucumber');\nconst fixtures = { When: [({ Given }, use) => use(Given)] };\nmodule.exports = fixtures;\n",
            false,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "package.json", "{}\n");
        write(
            directory.path(),
            "helper.js",
            "module.exports = (register, text, fn) => register(text, fn);\n",
        );
        write(directory.path(), "steps.js", steps);
        let assert = analyze_root(directory.path());
        let passed = predicate::str::contains("pass a step registration to a function");
        if reported {
            assert.stderr(passed);
        } else {
            assert.stderr(passed.not());
        }
    }
}

/// Runs selected TypeScript registrations and returns the final JSONL summary, rejecting operational failures.
fn registration_jsonl_report(root: &Path) -> Value {
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .args([
            root.to_str().unwrap(),
            "--definitions",
            "steps.ts",
            "--reporters",
            "jsonl",
            "--no-metrics",
        ])
        .output()
        .unwrap();
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(report["type"], "summary");
    report
}

/// Checks loader shadowing and mutation remove registration trust while unrelated and erased bindings preserve it.
#[test]
fn registration_loader_provenance_respects_runtime_scopes() {
    let body = "('first operation', () => { start(); finish(); }); step('second operation', () => { start(); finish(); });";
    for (prefix, suffix, trusted) in [
        ("", "", true),
        ("function require() {}", "", false),
        ("", "function require() {}", false),
        ("const require = other;", "", false),
        ("", "let require;", false),
        ("if (flag) { var require; }", "", false),
        ("class require {}", "", false),
        ("enum require { Other }", "", false),
        ("namespace require { export const other = 1; }", "", false),
        ("import require from './other';", "", false),
        ("require = other;", "", false),
        ("({require} = other);", "", false),
        ("function later() { require = other; }", "", false),
        ("function scope(require) {", "}", false),
        ("const scope = (require) => {", "};", false),
        ("function* scope(require) {", "}", false),
        ("{", "const require = other; }", false),
        ("try {} catch (require) {", "}", false),
        ("for (const require of loaders) {", "}", false),
        ("function sibling(require) { require = other; }", "", true),
        ("function sibling(require) { const {Given: step} = require('@cucumber/cucumber'); step('ghost', () => { start(); finish(); }); }", "", true),
        ("{ const require = other; }", "", true),
        ("type require = string; interface require {}", "", true),
        ("import type { require } from './other';", "", true),
        ("declare var require: any;", "", true),
        ("namespace scope { declare const require: any;", "}", true),
    ] {
        for local in [false, true] {
            for (module, binding) in [
                ("@cucumber/cucumber", "const { Given: step } = require(MODULE);"),
                ("@cucumber/cucumber", "const api = require(MODULE); const {Given: step} = api;"),
                ("playwright-bdd", "const {createBdd} = require(MODULE); const {Given: step} = createBdd();"),
            ] {
                let directory = tempfile::tempdir().unwrap();
                write(directory.path(), "barrel.cjs", &format!("module.exports = require('{module}');"));
                write(directory.path(), "other.ts", "export default function other() {};");
                let module = if local { "./barrel.cjs" } else { module };
                let binding = binding.replace("MODULE", &format!("'{module}'"));
                write(directory.path(), "steps.ts", &format!("{prefix} {binding} step{body} {suffix}"));
                let context = format!("{prefix} {binding} ... {suffix}");
                let report = registration_jsonl_report(directory.path());
                let summary = &report["summary"];
                assert_eq!(summary["definitionsAnalyzed"], if trusted {2} else {0}, "{context}");
                assert_eq!(summary["byRule"]["duplicate-handler"].as_u64().unwrap_or(0), u64::from(trusted), "{context}");
            }
        }
    }
}

/// Checks nested registration aliases retain lexical binding identity and do not invent sibling definitions.
#[test]
fn registration_aliases_follow_the_nearest_runtime_binding() {
    let cases = [
        (
            "const {Given:step}=require('@cucumber/cucumber'); function sibling(require){const {Given:step}=require('@cucumber/cucumber');step('ghost',()=>{start();finish();});} step('first',()=>{start();finish();});step('second',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "function nested(){const {Given:step}=require('@cucumber/cucumber');step('first',()=>{start();finish();});step('second',()=>{start();finish();});} function sibling(){step('ghost',()=>{start();finish();});} step('outside',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "const {Given:step}=require('@cucumber/cucumber');function nested(require){step('first',()=>{start();finish();});step('second',()=>{start();finish();});}",
            2,
            1,
        ),
        (
            "const {Given:step}=require('@cucumber/cucumber');function nested(){const step=unrelated;step('ghost',()=>{start();finish();});}step('first',()=>{start();finish();});step('second',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "const api=require('@cucumber/cucumber');function nested(require){const api=require('@cucumber/cucumber');api.Given('ghost',()=>{start();finish();});}api.Given('first',()=>{start();finish();});api.Given('second',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "function nested(){const api=require('@cucumber/cucumber');api.Given('first',()=>{start();finish();});api.Given('second',()=>{start();finish();});}function sibling(){api.Given('ghost',()=>{start();finish();});}",
            2,
            1,
        ),
        (
            "const api=require('@cucumber/cucumber');const {Given:step}=api;function nested(require){const api=require('@cucumber/cucumber');const {Given:step}=api;step('ghost',()=>{start();finish();});}step('first',()=>{start();finish();});step('second',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "const {Given:g}=require('@cucumber/cucumber');const step=g;function nested(require){const {Given:g}=require('@cucumber/cucumber');const step=g;step('ghost',()=>{start();finish();});}step('first',()=>{start();finish();});step('second',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "const {createBdd}=require('playwright-bdd');const {Given:step}=createBdd();function nested(require){const {createBdd}=require('playwright-bdd');const {Given:step}=createBdd();step('ghost',()=>{start();finish();});}step('first',()=>{start();finish();});step('second',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "function nested(){const {createBdd}=require('playwright-bdd');const {Given:step}=createBdd();step('first',()=>{start();finish();});step('second',()=>{start();finish();});}function sibling(){step('ghost',()=>{start();finish();});}",
            2,
            1,
        ),
        (
            "import {defineFeature} from 'jest-cucumber';function nested(){const {Given:step}=require('@cucumber/cucumber');step('first',()=>{start();finish();});step('second',()=>{start();finish();});}",
            2,
            1,
        ),
        (
            "import {defineFeature} from 'jest-cucumber';const {Given:step}=require('@cucumber/cucumber');function nested(require){const {Given:step}=require('@cucumber/cucumber');step('ghost',()=>{start();finish();});}step('first',()=>{start();finish();});step('second',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "const bdd=require('playwright-bdd');const {createBdd:factory}=bdd;factory('ghost',()=>{start();finish();});const {Given:step}=require('@cucumber/cucumber');step('first',()=>{start();finish();});step('second',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "const bdd=require('playwright-bdd');function nested(require){const bdd=require('playwright-bdd');const {createBdd:factory}=bdd;factory('ghost',()=>{start();finish();});}const {Given:step}=require('@cucumber/cucumber');step('first',()=>{start();finish();});step('second',()=>{start();finish();});",
            2,
            1,
        ),
        (
            "const {Given:step}=require('@cucumber/cucumber');function nested(){type step=number;step('first',()=>{start();finish();});step('second',()=>{start();finish();});}",
            2,
            1,
        ),
        (
            "import {Given as step} from '@cucumber/cucumber';function nested(require){step('first',()=>{start();finish();});step('second',()=>{start();finish();});}",
            2,
            1,
        ),
        (
            "import {expect} from '@jest/globals';const {Given:step}=require('@cucumber/cucumber');function nested(){const step=unrelated;step('ghost',()=>expect(1).toBe(1));}step('first',()=>expect(1).toBe(1));step('second',()=>expect(1).toBe(2));",
            2,
            0,
        ),
    ];
    for (source, definitions, duplicates) in cases {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "steps.ts", source);
        let report = registration_jsonl_report(directory.path());
        let summary = &report["summary"];
        assert_eq!(summary["definitionsAnalyzed"], definitions, "{source}");
        assert_eq!(
            summary["byRule"]["duplicate-handler"].as_u64().unwrap_or(0),
            duplicates,
            "{source}"
        );
    }
}

/// Checks configured registration names retain explicit global matching while escaped values mark discovery incomplete.
#[test]
fn configured_registration_name_keeps_its_explicit_global_policy() {
    for configured in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        if configured {
            write(
                directory.path(),
                ".cuke-dedup.json",
                r#"{"registrations":["step"],"threshold":100}"#,
            );
        }
        write(directory.path(), "steps.ts", "function nested(){function step(text,handler){return handler;}pass({step});step('first',()=>{start();finish();});step('second',()=>{start();finish();});}");
        let report = registration_jsonl_report(directory.path());
        assert_eq!(report["corpus"]["incomplete"], configured);
        let summary = &report["summary"];
        assert_eq!(
            summary["definitionsAnalyzed"],
            if configured { 2 } else { 0 }
        );
        assert_eq!(
            summary["byRule"]["duplicate-handler"].as_u64().unwrap_or(0),
            u64::from(configured)
        );
    }
}

/// Checks CommonJS barrel export forms require an unshadowed loader to retain registrations and completeness.
#[test]
fn registration_barrels_cannot_reexport_a_shadowed_loader() {
    for export in [
        "module.exports = require('@cucumber/cucumber');",
        "const api = require('@cucumber/cucumber'); module.exports = api;",
        "exports.Given = require('@cucumber/cucumber').Given;",
        "module.exports = {...require('@cucumber/cucumber')};",
        "Object.assign(module.exports, require('@cucumber/cucumber'));",
        "const {Given} = require('@cucumber/cucumber'); module.exports = {Given};",
        "const {createBdd} = require('playwright-bdd'); const {Given} = createBdd(); module.exports = {Given};",
    ] {
        for shadowed in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let prefix = if shadowed { "function require() {}" } else { "" };
            write(directory.path(), "barrel.ts", &format!("{prefix} {export}"));
            write(directory.path(), "steps.ts", "import {Given as step} from './barrel'; step('first operation', () => {start(); finish();}); step('second operation', () => {start(); finish();});");
            write(directory.path(), "steps.feature", "Feature: operations\n  Scenario: two steps\n    Given first operation\n    Given second operation\n");
            let report = registration_jsonl_report(directory.path());
            let summary = &report["summary"];
            let trusted = !shadowed;
            assert_eq!(summary["definitionsAnalyzed"], if trusted {2} else {0}, "{prefix} {export}");
            assert_eq!(summary["byRule"]["duplicate-handler"].as_u64().unwrap_or(0), u64::from(trusted), "{prefix} {export}");
            assert_eq!(report["corpus"]["incomplete"], shadowed, "{prefix} {export}");
        }
    }
}

/// Checks uncertainty follows exported shadowed-loader values without tainting unrelated helper exports.
#[test]
fn shadowed_loader_barrel_completeness_follows_exported_local_values() {
    for (barrel, incomplete) in [
        ("function require() {} const {Given} = require('@cucumber/cucumber'); const alias = Given; module.exports = {alias};", true),
        ("function require() {} const {Given} = (require)('@cucumber/cucumber'); const alias = Given; module.exports = {alias};", true),
        ("function require() {} const {Given} = (require as any)('@cucumber/cucumber'); const alias = Given; module.exports = {alias};", true),
        ("function require() {} const ignored = require('@cucumber/cucumber'); const helper = () => {}; module.exports = {helper};", false),
        ("function require() {} const ignored = require('@cucumber/cucumber'); module.exports = {helper: () => {}};", false),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "barrel.ts", barrel);
        write(directory.path(), "steps.ts", "import {alias as step} from './barrel'; step('first', () => {});");
        write(directory.path(), "steps.feature", "Feature: probe\n  Scenario: one\n    Given first\n");
        let report = registration_jsonl_report(directory.path());
        let result = report;
        assert_eq!(result["summary"]["definitionsAnalyzed"], 0, "{barrel}");
        assert_eq!(result["corpus"]["incomplete"], incomplete, "{barrel}");
    }
}

/// Checks loader-based imports respect shadowing and all import forms reject conflicting assertion handler findings.
#[test]
fn registration_loader_import_forms_preserve_conflicting_assertions() {
    for (binding, callee, uses_loader) in [
        (
            "import {Given as step} from '@cucumber/cucumber';",
            "step",
            false,
        ),
        (
            "import api = require('@cucumber/cucumber');",
            "api.Given",
            true,
        ),
        (
            "const api = require('@cucumber/cucumber');",
            "api.Given",
            true,
        ),
        ("import {Given as step} from './barrel';", "step", false),
    ] {
        for shadowed in [false, true] {
            for conflicting in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                write(
                    directory.path(),
                    "barrel.ts",
                    "export {Given} from '@cucumber/cucumber';",
                );
                let prefix = if shadowed {
                    "function require() {}"
                } else {
                    ""
                };
                let value = if conflicting { 2 } else { 1 };
                write(directory.path(), "steps.ts", &format!("import {{expect}} from '@jest/globals'; {prefix} {binding} {callee}('first operation', () => expect(1).toBe(1)); {callee}('second operation', () => expect(1).toBe({value}));"));
                let report = registration_jsonl_report(directory.path());
                let summary = &report["summary"];
                let trusted = !uses_loader || !shadowed;
                let context = format!("{prefix} {binding} conflicting={conflicting}");
                assert_eq!(
                    summary["definitionsAnalyzed"],
                    if trusted { 2 } else { 0 },
                    "{context}"
                );
                for rule in [
                    "duplicate-handler",
                    "near-duplicate-step",
                    "parameterization-candidate",
                ] {
                    let expected =
                        u64::from(trusted && !conflicting && rule == "duplicate-handler");
                    assert_eq!(
                        summary["byRule"][rule].as_u64().unwrap_or(0),
                        expected,
                        "{context} {rule}"
                    );
                }
            }
        }
    }
}

/// Losing a matcher must affect strictness and reports without discarding independent findings.
#[test]
fn unsupported_matchers_mark_partial_extraction_incomplete() {
    let uncertain_wrappers = [
        "function step(p, h) { given(p, h); } given = other; step('other', () => work());",
        "function step(p, h) { given(p, h); } step = other; step('other', () => work());",
        "function step(p, h) { given(p, h); } function outer(p,h) { step(p,h); } given = other; outer('other', () => work());",
        "function step(p, h) { given(p, h); } given ||= other; step('other', () => work());",
        "function step(p, h) { given(p, h); } given++; step('other', () => work());",
        "function step(p, h) { given(p, h); } ({given} = other); step('other', () => work());",
        "function step(p, h) { given(p, h); } function change() { given = other; } step('other', () => work());",
        "function step(given, h) { given(given, h); } step('other', () => work());",
        "function step(p, h) { given(p, h); } for (given of others) {} step('other', () => work());",
        "const alias = given; function step(p,h) { alias(p,h); } given = other; step('other', () => work());",
        "function step(p,h) { given(p,h); } ({key: given = other} = source); step('other', () => work());",
        "function step(p,h) { given(p,h); } [given] = source; step('other', () => work());",
    ].map(|call| ("import { Given } from '@cucumber/cucumber'; let given = Given;", call, false, false));
    for (prefix, call, complete, severity_error) in [
        (
            "import { Given } from '@cucumber/cucumber';",
            "Given(pattern, () => work());",
            false,
            false,
        ),
        (
            "import { Given as step } from '@cucumber/cucumber';",
            "step(buildPattern(), function() { work(); });",
            false,
            false,
        ),
        (
            "import * as bdd from '@cucumber/cucumber';",
            "bdd.Given(`value ${runtime}`, function*() { yield work(); });",
            false,
            false,
        ),
        (
            "import { Given as given } from '@cucumber/cucumber';",
            "function step(p, h) { given(p, h); } step(pattern, () => work());",
            false,
            false,
        ),
        (
            "const { Given: given } = require('@cucumber/cucumber');",
            "function step(p, h) { given(p, h); } step('static', () => work());",
            true,
            false,
        ),
        (
            "import { Given } from '@cucumber/cucumber'; let given = Given;",
            "function step(p, h) { given(p, h); } function local(given) { given = other; } for (let given of others) {} given.metadata = other; step('static', () => work());",
            true,
            false,
        ),
        (
            "const { Given: step } = require('@cucumber/cucumber');",
            "step(42, () => work());",
            false,
            false,
        ),
        (
            "import { Given } from '@cucumber/cucumber';",
            "Given();",
            false,
            false,
        ),
        (
            "import { Given } from '@cucumber/cucumber';",
            r"Given('\u{110000}', () => work());",
            false,
            true,
        ),
        (
            "import { Given as Step } from 'playwright-bdd/decorators';",
            "class Steps { @Step(pattern) run() {} }",
            false,
            false,
        ),
        (
            "import { Given as Step } from 'playwright-bdd/decorators';",
            "class Steps { @Step() run() {} }",
            false,
            false,
        ),
        (
            "import { Given } from '@cucumber/cucumber';",
            "Given('static', () => work());",
            true,
            false,
        ),
        (
            "import { Given } from '@cucumber/cucumber';",
            "Given(/static/, () => work());",
            true,
            false,
        ),
        (
            "import { Given } from '@cucumber/cucumber';",
            "Given(`static`, () => work());",
            true,
            false,
        ),
        (
            "",
            "function Given() {}; Given(pattern, () => work());",
            false,
            false,
        ),
        ("", "unknown.Given(pattern, () => work());", false, false),
        ("", "Given(pattern, () => work());", false, false),
        ("", "helper(pattern, () => work());", true, false),
    ].into_iter().chain(uncertain_wrappers) {
        let directory = tempfile::tempdir().unwrap();
        let source = format!("{prefix}\n{call}\nimport {{ When }} from '@cucumber/cucumber';\nWhen('known', () => open());\nWhen('known', () => close());\n");
        write(directory.path(), "steps.ts", &source);
        write(
            directory.path(),
            "suite.feature",
            "Feature: Usage\n Scenario: Known\n  When known\n",
        );
        let read_report = |filename: &str| -> Value {
            serde_json::from_reader(fs::File::open(directory.path().join("reports").join(filename)).unwrap()).unwrap()
        };
        for strict in [false, true] {
            let mut cmd = Command::cargo_bin("cuke-dedup").unwrap();
            cmd.current_dir(directory.path()).args([
                ".",
                "--definitions",
                "*.ts",
                "--features",
                "*.feature",
                "--reporters",
                "json,sarif",
                "--output",
                "reports",
                "--no-metrics",
                "--fail-on-unparseable",
            ]);
            if strict {
                cmd.arg("--fail-on-incomplete");
            }
            cmd.assert()
                .code(if severity_error || (strict && !complete) {
                    2
                } else {
                    1
                });
            let report = read_report("cuke-dedup.json");
            assert_eq!(report["corpus"]["incomplete"], !complete, "{source}");
            assert_eq!(
                report["summary"]["definitionsAnalyzed"],
                if call.contains("static") { 3 } else { 2 },
                "{source}"
            );
            assert!(
                report["findings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|f| f["rule"] == "duplicate-matcher"),
                "{source}"
            );
            let sarif = read_report("cuke-dedup.sarif");
            assert_eq!(
                sarif["runs"][0]["invocations"][0]["executionSuccessful"], complete,
                "{source}"
            );
        }
    }
}

/// Checks dynamic matchers fail completeness gates and prevent baseline creation while default analysis continues.
#[test]
fn dynamic_matcher_cannot_pass_strict_mode_or_create_a_baseline() {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "steps.ts", "import { Given } from '@cucumber/cucumber'; Given('known', () => work()); Given(buildPattern(), () => work());");
    write(
        directory.path(),
        "suite.feature",
        "Feature: Usage\n Scenario: Known\n  Given known\n",
    );
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"definitions":["steps.ts"],"features":["*.feature"],"reporters":["json"],"output":"reports","noMetrics":true}"#,
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(0);
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--fail-on-incomplete"])
        .assert()
        .code(2);
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--baseline", "baseline.json", "--update-baseline"])
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "was not updated because analysis is incomplete",
        ));
    assert!(!directory.path().join("baseline.json").exists());
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"definitions":["steps.ts"],"features":["*.feature"],"reporters":["json"],"failOnIncomplete":true}"#,
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .arg(".")
        .assert()
        .code(2);
}

/// Checks proven inert members stay complete while mutation, escape, and unknown receivers preserve discovery uncertainty.
#[test]
fn inert_local_members_do_not_make_registration_discovery_incomplete() {
    let cases = [
        ("const foreign = { Given(...args) {} }; foreign.Given('foreign', () => {});", false),
        ("const foreign = { Given: () => {} }; foreign['Given']('foreign', () => {});", false),
        ("const foreign = { ['Given']: function() {} }; foreign.Given();", false),
        ("const foreign = { *Given() {} }; foreign.Given();", false),
        ("const foreign = { Given() {} }; const alias = foreign; alias.Given();", false),
        ("const foreign = { Given() {} }; const { Given: step } = foreign; step();", false),
        ("const foreign = { Given() {} }; const step = foreign.Given; step();", false),
        ("const foreign = { Given() {} }; function unrelated(foreign) { foreign = other; } foreign.Given();", false),
        ("const foreign = { Given() {} }; type foreign = unknown; foreign.Given();", false),
        ("const foreign = { Given() {} }; export type { foreign }; foreign.Given();", false),
        ("const foreign = { Given() {} }; function inner(foreign) { foreign.Given(); }", true),
        ("unknown.Given();", true),
        ("const foreign = load(); foreign.Given();", true),
        ("import * as foreign from 'unknown-library'; foreign.Given();", true),
        ("const foreign = { Given() { register(); } }; foreign.Given();", true),
        ("const foreign = { Given: 42 }; foreign.Given();", true),
        ("const foreign = { Given() {} }; foreign.Given.Given();", true),
        ("const foreign = { Given(value = register()) {} }; foreign.Given();", true),
        ("const foreign = { Given({value}) {} }; foreign.Given();", true),
        ("const foreign = { Given() {}, [key]: other }; foreign.Given();", true),
        ("const foreign = { Given() {}, ...other }; foreign.Given();", true),
        ("const foreign = { get Given() { return other; } }; foreign.Given();", true),
        ("const foreign = { __proto__: other, Given() {} }; foreign.Given();", true),
        ("const foreign = { Given() {}, Given: other }; foreign.Given();", true),
        ("let foreign = { Given() {} }; foreign = other; foreign.Given();", true),
        ("const foreign = { Given() {} }; foreign.Given = other; foreign.Given();", true),
        ("const foreign = { Given() {} }; delete foreign.Given; foreign.Given();", true),
        ("const foreign = { Given() {} }; delete foreign[key]; foreign.Given();", true),
        ("const foreign = { Given() {} }; const alias = foreign; alias[key] = other; foreign.Given();", true),
        ("const foreign = { Given() {} }; mutate(foreign); foreign.Given();", true),
        ("const foreign = { Given() {} }; new Mutator(foreign); foreign.Given();", true),
        ("const foreign = { Given() {} }; tag`${foreign}`; foreign.Given();", true),
        ("const foreign = { Given() {}, change() { this.Given = Given; } }; foreign.change(); foreign.Given();", true),
        ("const foreign = { Given() {} }; foreign.configure(); foreign.Given();", true),
        ("const foreign = { Given() {} }; mutate({foreign}); foreign.Given();", true),
        ("const foreign = { Given() {} }; const container = [foreign]; mutate(container); foreign.Given();", true),
        ("const foreign = { Given() {} }; const container = {foreign}; mutate(container); foreign.Given();", true),
        ("const foreign = { Given() {}, valueOf() { this.Given = Given; } }; +foreign; foreign.Given();", true),
        ("const foreign = { Given() {} }; external.saved = foreign; foreign.Given();", true),
        ("const foreign = { Given() {} }; external.saved ||= foreign; foreign.Given();", true),
        ("const foreign = { Given() {} }; external.saved &&= foreign; foreign.Given();", true),
        ("const foreign = { Given() {} }; const alias = foreign; external.saved ??= alias; foreign.Given();", true),
        ("const foreign = { Given() {} }; external.saved ||= other; function unrelated(foreign) { external.saved ??= foreign; } foreign.Given();", false),
        ("const foreign = { Given: (() => {}) }; foreign.Given();", false),
        ("const foreign = { Given: (() => {}) as Function }; foreign.Given();", false),
        ("const foreign = { Given: (() => {}) satisfies Function }; foreign.Given();", false),
        ("const foreign = { Given: (function() {})! }; foreign.Given();", false),
        ("const foreign = { Given: <Function>(function*() {}) }; foreign.Given();", false),
        ("const foreign = { Given: (() => {}) as Function }; const { Given: step } = foreign; step();", false),
        ("const foreign = { Given: (() => register()) as Function }; foreign.Given();", true),
        ("const foreign = { Given: ((value = register()) => {}) as Function }; foreign.Given();", true),
        ("const foreign = { Given: (unknown as Function) }; foreign.Given();", true),
        ("const foreign = { Given: (() => {}) as Function }; foreign.Given = other; foreign.Given();", true),
        ("const foreign = { Given() {} }; function expose() { return foreign; } foreign.Given();", true),
        ("const foreign = { Given() {} }; function* expose() { yield foreign; } foreign.Given();", true),
        ("const foreign = { Given() {} }; const alias = foreign; function* expose() { yield (alias as object); } foreign.Given();", true),
        ("const foreign = { Given() {} }; async function* expose() { yield foreign; } foreign.Given();", true),
        ("const foreign = { Given() {} }; function* expose() { yield* [foreign]; } foreign.Given();", true),
        ("const foreign = { Given() {} }; function expose() { throw foreign; } foreign.Given();", true),
        ("const foreign = { Given() {} }; const alias = foreign; async function expose() { throw alias; } foreign.Given();", true),
        ("const foreign = { Given() {} }; try { throw foreign; } catch (exposed) { exposed.Given = Given; } foreign.Given();", true),
        ("const foreign = { Given() {} }; function* unrelated() { yield void foreign; throw 42; } foreign.Given();", false),
        ("const foreign = { Given() {} }; function* unrelated(foreign) { yield foreign; throw foreign; } foreign.Given();", false),
        ("const foreign = { Given() {} }; const expose = () => foreign; foreign.Given();", true),
        ("export const foreign = { Given() {} }; foreign.Given();", true),
        ("const foreign = { Given() {} }; export { foreign }; foreign.Given();", true),
        ("const foreign = { Given() {} }; function change() { foreign.Given = Given; } foreign.Given();", true),
        ("const foreign = { Given() {} }; foreign.Given(Given);", true),
        ("const foreign = { Given }; foreign.Given('foreign', () => {});", true),
        ("const foreign = require('@cucumber/cucumber'); foreign.Given(pattern, () => {});", true),
    ];
    for (source, incomplete, import) in cases.into_iter().flat_map(|(source, incomplete)| {
        [
            "import { Given } from '@cucumber/cucumber';",
            "const { Given } = require('@cucumber/cucumber');",
        ]
        .map(move |import| (source, incomplete, import))
    }) {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "steps.ts",
            &format!(
                "{import}\nGiven('known', () => open()); Given('known', () => close());\n{source}"
            ),
        );
        write(
            directory.path(),
            "suite.feature",
            "Feature: Usage\n Scenario: Known\n  Given known\n",
        );
        Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(directory.path())
            .args([
                ".",
                "--definitions",
                "*.ts",
                "--features",
                "*.feature",
                "--reporters",
                "json",
                "--output",
                "out",
                "--no-metrics",
                "--fail-on-incomplete",
            ])
            .assert()
            .code(if incomplete { 2 } else { 1 });
        let report: Value = serde_json::from_reader(
            fs::File::open(directory.path().join("out/cuke-dedup.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report["corpus"]["incomplete"], incomplete, "{source}");
        assert_eq!(report["summary"]["definitionsAnalyzed"], 2, "{source}");
        assert!(
            report["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["rule"] == "duplicate-matcher"),
            "{source}"
        );
    }
}

/// Checks a project containing only a proven inert local receiver passes strict completeness.
#[test]
fn inert_receiver_only_project_passes_strict_mode() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "const foreign = { Given(...args) {} }; foreign.Given('not a registration', () => {});",
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([
            ".",
            "--definitions",
            "*.ts",
            "--fail-on-incomplete",
            "--no-metrics",
        ])
        .assert()
        .success();
}

struct RubyReleaseRun {
    code: Option<i32>,
    definitions: u64,
    incomplete: bool,
    active: Vec<String>,
    suppressed: Vec<String>,
    stderr: String,
}

/// Runs a Ruby project with an optional definition pattern.
fn ruby_release_run(root: &Path, definitions: Option<&str>) -> RubyReleaseRun {
    let selection: Vec<&str> = definitions
        .map(|pattern| vec!["--definitions", pattern])
        .unwrap_or_default();
    ruby_run(root, &selection)
}

/// Runs the binary on a Ruby project with JSONL output and the given extra arguments.
fn ruby_run(root: &Path, extra: &[&str]) -> RubyReleaseRun {
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(root)
        .args(["--reporters", "jsonl", "--no-metrics"])
        .args(extra)
        .output()
        .unwrap();
    let rows: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let summary = rows.iter().find(|row| row["type"] == "summary").unwrap();
    let rules = |active: bool| {
        rows.iter()
            .filter(|row| row["type"] == "finding" && row["active"] == active)
            .map(|row| row["rule"].as_str().unwrap().to_owned())
            .collect()
    };
    RubyReleaseRun {
        code: output.status.code(),
        definitions: summary["summary"]["definitionsAnalyzed"].as_u64().unwrap(),
        incomplete: summary["corpus"]["incomplete"].as_bool().unwrap(),
        active: rules(true),
        suppressed: rules(false),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

const RUBY_RELEASE_FEATURE: &str =
    "Feature: release\n  Scenario: steps\n    Given same\n    Given other\n";
const RUBY_RELEASE_STEPS: &str = "Given('same') { work(1) }\nGiven('other') { work(1) }\n";

/// Runs fixed Ruby registration and feature fixtures preceded by the supplied ownership prelude.
fn ruby_release_prelude_run(prelude: &str) -> RubyReleaseRun {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.rb",
        &format!("{prelude}{RUBY_RELEASE_STEPS}"),
    );
    write(directory.path(), "a.feature", RUBY_RELEASE_FEATURE);
    ruby_release_run(directory.path(), Some("*.rb"))
}

/// Checks broad patterns preserve TypeScript selection, Ruby requires suffix opt-in, and mixed explicit selections fail.
#[test]
fn regression_ruby_release_broad_glob_keeps_typescript_selection() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    write(
        root,
        "features/support/steps.ts",
        "import { Given } from '@cucumber/cucumber';\nGiven('a user exists', () => createUser());\nGiven('a user exists', () => createUser());\n",
    );
    // A Ruby helper next to TypeScript steps was ignored before Ruby support existed.
    write(
        root,
        "features/support/env.rb",
        "Given('ruby step') { work(1) }\n",
    );
    write(
        root,
        "features/users.feature",
        "Feature: users\n  Scenario: existing\n    Given a user exists\n",
    );
    for (pattern, definitions, skipped, mixed) in [
        (Some("features/support/**"), 2, true, false),
        (Some("features/**/*"), 2, true, false),
        (None, 2, false, false),
        (Some("features/**/*.rb"), 1, false, false),
        (Some("features/**/env.rb,features/**/*.ts"), 0, false, true),
    ] {
        let run = ruby_release_run(root, pattern);
        assert_eq!(run.definitions, definitions, "{pattern:?}");
        assert_eq!(
            run.stderr.contains("source file(s) were not analyzed"),
            skipped,
            "{pattern:?}: {}",
            run.stderr
        );
        assert_eq!(
            run.stderr.contains("separate analysis runs"),
            mixed,
            "{pattern:?}"
        );
        assert_eq!(run.code == Some(2), mixed, "{pattern:?}");
        assert!(!run.stderr.contains("matched no files"), "{pattern:?}");
    }

    let ruby_only = tempfile::tempdir().unwrap();
    write(ruby_only.path(), "features/steps.rb", RUBY_RELEASE_STEPS);
    write(ruby_only.path(), "features/a.feature", RUBY_RELEASE_FEATURE);
    for (pattern, definitions, skipped) in [
        (Some("features/**/*"), 0, true),
        (Some("features/**/*.rb"), 2, false),
        (None, 0, true),
    ] {
        let run = ruby_release_run(ruby_only.path(), pattern);
        assert_eq!(run.definitions, definitions, "{pattern:?}");
        assert_eq!(
            run.stderr
                .contains("need a definition pattern that names their suffix (`.rb`)"),
            skipped,
            "{pattern:?}: {}",
            run.stderr
        );
    }
}

/// Checks supported dialect keywords retain complete Ruby handler findings and unknown keywords register nothing.
#[test]
fn regression_ruby_release_dialect_step_keywords_register_definitions() {
    for (keywords, feature) in [
        (
            ["Angenommen", "Wenn"],
            "# language: de\nFunktionalität: f\n  Szenario: s\n    Angenommen same\n    Wenn other\n",
        ),
        (
            ["Dado", "Cuando"],
            "# language: es\nCaracterística: f\n  Escenario: s\n    Dado same\n    Cuando other\n",
        ),
        (
            ["wann", "dann"],
            "# language: lu\nFunktionalitéit: f\n  Szenario: s\n    ugeholl same\n    wann other\n",
        ),
        (
            ["Gangway!", "Blimey!"],
            "# language: en-pirate\nAhoy matey!: f\n  Heave to: s\n    Gangway! same\n    Blimey! other\n",
        ),
        (
            ["假如", "当"],
            "# language: zh-CN\n功能: f\n  场景: s\n    假如same\n    当other\n",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "steps.rb",
            &format!(
                "{}('same') {{ work(1) }}\n{}('other') {{ work(1) }}\n",
                keywords[0], keywords[1]
            ),
        );
        write(directory.path(), "a.feature", feature);
        let run = ruby_release_run(directory.path(), Some("*.rb"));
        assert_eq!(run.definitions, 2, "{keywords:?}: {}", run.stderr);
        assert!(!run.incomplete, "{keywords:?}");
        assert_eq!(run.active, ["duplicate-handler"], "{keywords:?}");
    }

    // Not a step keyword in any dialect, so Cucumber-Ruby defines no such registrar.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.rb",
        "Suppose('same') { work(1) }\nSuppose('other') { work(1) }\n",
    );
    write(directory.path(), "a.feature", RUBY_RELEASE_FEATURE);
    assert_eq!(
        ruby_release_run(directory.path(), Some("*.rb")).definitions,
        0
    );
}

/// Checks registrar implementation changes revoke discovery trust while unrelated helpers preserve it.
#[test]
fn regression_ruby_release_registrar_implementation_changes_revoke_trust() {
    for (prelude, trusted) in [
        // Cucumber::Glue implements registration; any change there can redirect it.
        (
            "module Cucumber\n  module Glue\n    module Dsl\n      def register_rb_step_definition(*args); end\n    end\n  end\nend\n",
            false,
        ),
        ("class << Cucumber::Glue::Dsl\n  def rb_language; end\nend\n", false),
        ("def (Cucumber::Glue::Dsl).rb_language; end\n", false),
        (
            "Cucumber::Glue::Dsl.define_singleton_method(:rb_language) { nil }\n",
            false,
        ),
        (
            "module Cucumber\n  Glue::Dsl.instance_variable_set(:@rb_language, nil)\nend\n",
            false,
        ),
        (
            "dsl = Cucumber::Glue::Dsl\ndsl.define_method(:register_rb_step_definition) { nil }\n",
            false,
        ),
        ("class Object < BasicObject\n  def Given(*args); end\nend\n", false),
        // Subclassing runs the superclass `inherited` hook, which unresolved code may define.
        ("class Page < Base\n  def visit_home; end\nend\n", false),
        // Controls: helpers that cannot shadow the DSL extended onto `main`.
        ("def helper; end\n", true),
        ("class Page\n  def visit_home; end\nend\n", true),
        (
            "module Pages\n  class Home\n    def open; end\n  end\nend\n",
            true,
        ),
        (
            "module Cucumber\n  module Formatter\n    class Mine\n      def emit; end\n    end\n  end\nend\n",
            true,
        ),
        ("module Cucumber\n  module Glue\n  end\nend\n", true),
        ("Cucumber.logger\n", true),
        ("", true),
    ] {
        let run = ruby_release_prelude_run(prelude);
        assert_eq!(run.definitions, if trusted { 2 } else { 0 }, "{prelude}");
        assert_eq!(run.incomplete, !trusted, "{prelude}");
        assert_eq!(run.active.is_empty(), !trusted, "{prelude}");
    }
}

/// Checks Ruby ownership diagnostics identify the location of each invalidating executable effect.
#[test]
fn regression_ruby_release_ownership_diagnostic_points_at_its_cause() {
    // `include` and subclassing run `included`/`inherited` hooks, which can modify any owner.
    // An unresolved target keeps the suite untrusted; the diagnostic must name the cause.
    for (prelude, line_column) in [
        (
            "module Helpers\n  include Capybara::DSL\nend\nWorld(Helpers)\n",
            "steps.rb:2:3:",
        ),
        ("extend Navigation\n", "steps.rb:1:1:"),
        ("class Page < Base\nend\n", "steps.rb:1:14:"),
        // Each remaining invalidation path, with its cause off the first line so the old `1:1`
        // fallback cannot pass: a built-in ParameterType redefinition, a repeated wrapper
        // declaration, a wrapper mutation, dynamic method-table mutation, and a protected-name
        // definition on an owner that is referenced elsewhere.
        (
            "x = 1\nParameterType(name: 'int', regexp: /\\d+/, transformer: ->(s) { s.to_i })\n",
            "steps.rb:2:1:",
        ),
        (
            "x = 1\ndef w(t, &h)\n  Given(t, &h)\nend\ndef w(t, &h)\n  Given(t, &h)\nend\n",
            "steps.rb:5:1:",
        ),
        (
            "x = 1\ndef w(t, &h)\n  Given(t, &h)\nend\nundef w\n",
            "steps.rb:5:1:",
        ),
        (
            "x = 1\ndef w(t, &h)\n  Given(t, &h)\nend\nsend(:alias_method, name, :w)\n",
            "steps.rb:5:1:",
        ),
        (
            "x = 1\nmodule Helpers\n  def Given(*args); end\nend\nHelpers.foo\n",
            "steps.rb:3:3:",
        ),
    ] {
        let run = ruby_release_prelude_run(prelude);
        assert_eq!(run.definitions, 0, "{prelude}");
        assert!(run.incomplete);
        assert!(
            run.stderr.contains(&format!(
                "{line_column} Ruby registration ownership or executable source effects are unresolved"
            )),
            "{prelude}: {}",
            run.stderr
        );
    }
}

/// Checks Ruby and TypeScript inline suppressions agree on active findings, suppressed rules, and malformed-directive exits.
#[test]
fn regression_ruby_release_inline_suppression_matches_typescript() {
    for (file, comment, registrations) in [
        (
            "steps.ts",
            "//",
            "import { Given } from '@cucumber/cucumber';\nDIRECTIVE\nGiven('same', () => work(1));\nGiven('other', () => work(1));\n",
        ),
        (
            "steps.rb",
            "#",
            "DIRECTIVE\nGiven('same') { work(1) }\nGiven('other') { work(1) }\n",
        ),
    ] {
        for (directive, active, suppressed, code) in [
            (
                "cuke-dedup:ignore duplicate-handler -- legacy contract",
                vec![],
                vec!["duplicate-handler"],
                Some(0),
            ),
            (
                "cuke-dedup:ignore duplicate-matcher -- unrelated rule",
                vec!["duplicate-handler"],
                vec![],
                Some(1),
            ),
            (
                "cuke-dedup:ignore duplicate-handler",
                vec!["duplicate-handler"],
                vec![],
                Some(2),
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            write(
                directory.path(),
                file,
                &registrations.replace("DIRECTIVE", &format!("{comment} {directive}")),
            );
            write(directory.path(), "a.feature", RUBY_RELEASE_FEATURE);
            let pattern = format!("*{}", &file[5..]);
            let run = ruby_release_run(directory.path(), Some(&pattern));
            assert_eq!(run.active, active, "{file}: {directive}");
            assert_eq!(run.suppressed, suppressed, "{file}: {directive}");
            assert_eq!(run.code, code, "{file}: {directive}: {}", run.stderr);
        }
    }
}

const RUBY_HANDLER_VALUES_PROVIDER: &str = include_str!(
    "../fixtures/ruby-parity/equivalent-handlers/precision-inline-local/providers/assertions.rb"
);

/// Runs Ruby files beside a configured synthetic assertion provider.
fn ruby_handler_values_run(files: &[(&str, &str)]) -> RubyReleaseRun {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "provider.rb",
        RUBY_HANDLER_VALUES_PROVIDER,
    );
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"assertionModules":["provider"],"threshold":100,"nearDuplicateHandlerSimilarity":0.5,"rules":{"unused-definition":"off"}}"#,
    );
    for (name, body) in files {
        write(
            directory.path(),
            name,
            &format!("require_relative 'provider'\n{body}\n"),
        );
    }
    ruby_release_run(directory.path(), Some("*.rb"))
}

/// A block-local write beside a file-level capture must not make the sibling reader incomplete.
#[test]
fn regression_ruby_handler_values_block_local_write_keeps_capture_complete() {
    // A `|; name|` block-local shadows the file-level capture; writing it inside the handler
    // used to count as a capture write and made the sibling handler incomplete.
    let pair = "Given('the omega brake holds a local stop') { |; shared| shared = 2; SyntheticAssertions.expect(brake()).to_be(shared) }\nGiven('the omega brake holds a module stop') { SyntheticAssertions.expect(brake()).to_be(shared) }";
    let run = ruby_handler_values_run(&[("steps.rb", &format!("shared = 1\n{pair}"))]);
    assert_eq!(run.definitions, 2);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.active, Vec::<String>::new());
    // Without the block-local the write reaches the capture and both readers are uncertain.
    let plain = pair.replace("|; shared| ", "");
    let run = ruby_handler_values_run(&[("steps.rb", &format!("shared = 1\n{plain}"))]);
    assert!(run.incomplete);
    for location in ["steps.rb:3:45", "steps.rb:4:46"] {
        assert!(run.stderr.contains(location), "{location}: {}", run.stderr);
    }
}

/// Equal top-level constants collapse handlers; conflicting ones, declared after the handlers, keep them apart.
#[test]
fn regression_ruby_handler_values_top_level_constants_reach_the_fingerprint() {
    let steps = "ALPHA_LIMIT = 1\nALPHA_BOUND = 1\nGiven('the alpha meter reaches its limit') { SyntheticAssertions.expect(meter()).to_be(ALPHA_LIMIT) }\nGiven('the alpha meter reaches its bound') { SyntheticAssertions.expect(meter()).to_be(ALPHA_BOUND) }\nGiven('the beta dial holds its first stop') { SyntheticAssertions.expect(dial()).to_be(BETA_FIRST) }\nGiven('the beta dial holds its second stop') { SyntheticAssertions.expect(dial()).to_be(BETA_SECOND) }\nBETA_FIRST = 1\nBETA_SECOND = 2";
    let run = ruby_handler_values_run(&[("steps.rb", steps)]);
    assert_eq!(run.definitions, 4);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.active, vec!["duplicate-handler", "near-duplicate-step"]);
    let run = ruby_handler_values_run(&[(
        "steps.rb",
        &steps.replace("ALPHA_BOUND = 1", "ALPHA_BOUND = 2"),
    )]);
    assert_eq!(run.active, Vec::<String>::new());
    assert!(!run.incomplete);
}

/// Called identical lambdas duplicate, called conflicting ones stay apart, uncalled ones only parameterize.
#[test]
fn regression_ruby_handler_values_declared_lambda_runs_only_where_invoked() {
    let handler = |matcher: &str, subject: &str, value: u8, invocation: &str| {
        format!("Given('{matcher}') do\n  check = -> {{ SyntheticAssertions.expect({subject}()).to_be({value}) }}\n  {invocation}\nend")
    };
    let steps = [
        handler("the alpha meter settles evenly", "meter", 1, "check.call"),
        handler("the alpha meter settles smoothly", "meter", 1, "check.call"),
        handler("the beta dial resolves upward", "dial", 1, "check.call"),
        handler("the beta dial resolves downward", "dial", 2, "check.call"),
        handler("the crane idles at a first angle", "crane", 1, ""),
        handler("the crane idles at a second angle", "crane", 2, ""),
    ]
    .join("\n");
    let run = ruby_handler_values_run(&[("steps.rb", &steps)]);
    assert_eq!(run.definitions, 6);
    assert!(!run.incomplete, "{}", run.stderr);
    let mut active = run.active.clone();
    active.sort();
    assert_eq!(
        active,
        vec![
            "duplicate-handler",
            "near-duplicate-step",
            "parameterization-candidate"
        ]
    );
}

/// Conflicting trusted assertions are one differing event: near needs shared ordinary behaviour,
/// a called declaration alone shares nothing, and a stored invocation result is not an event.
#[test]
fn regression_ruby_assertion_near_conflicting_values_are_one_differing_event() {
    let handler = |matcher: &str, body: &str| format!("Given('{matcher}') do\n  {body}\nend");
    let steps = [
        handler(
            "the alpha probe reads a first value",
            "SyntheticAssertions.expect(alpha()).to_be(1); settle()",
        ),
        handler(
            "the alpha probe reads a second value",
            "SyntheticAssertions.expect(alpha()).to_be(2); settle()",
        ),
        handler(
            "the beta gauge reads a first value",
            "check = -> { SyntheticAssertions.expect(beta()).to_be(1) }; check.call",
        ),
        handler(
            "the beta gauge reads a second value",
            "check = -> { SyntheticAssertions.expect(beta()).to_be(2) }; check.call",
        ),
        handler(
            "the gamma store reads a first value",
            "act = -> { SyntheticAssertions.expect(gamma()).to_be(1) }; result = act.call",
        ),
        handler(
            "the gamma store reads a second value",
            "act = -> { SyntheticAssertions.expect(gamma()).to_be(2) }; result = act.call",
        ),
    ]
    .join("\n");
    let run = ruby_handler_values_run(&[("steps.rb", &steps)]);
    assert_eq!(run.definitions, 6);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.active, vec!["near-duplicate-step"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
}

/// Resolved provider aliases compare across files; a same-named runtime lambda stays complete and untrusted.
#[test]
fn regression_ruby_handler_values_resolved_alias_captures_compare_across_files() {
    let direct = "expect = SyntheticAssertions.method(:expect)\nThen('the parcel is ready') { |state| expect.call(state).to_be('ready') }";
    let hash = "expect = SyntheticAssertions.api.fetch(:expect)\nThen('shipment readiness has been confirmed') { |state| expect.call(state).to_be('ready') }";
    let run = ruby_handler_values_run(&[("direct.rb", direct), ("hash.rb", hash)]);
    assert_eq!(run.definitions, 2);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.active, vec!["duplicate-handler"]);
    // A same-named runtime lambda is a complete, untrusted file-local value.
    let runtime = "expect = ->(value) { value }\nThen('the parcel status is now verified') { |state| expect.call(state).to_be('ready') }";
    let run = ruby_handler_values_run(&[("direct.rb", direct), ("runtime.rb", runtime)]);
    assert_eq!(run.definitions, 2);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.active, Vec::<String>::new());
}

/// Runs Ruby files with the near threshold the parity fixtures use and no assertion trust.
fn ruby_action_run(files: &[(&str, &str)], extra: &[&str]) -> RubyReleaseRun {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        ".cuke-dedup.json",
        r#"{"threshold":100,"nearDuplicateHandlerSimilarity":0.5,"rules":{"unused-definition":"off"}}"#,
    );
    for (name, body) in files {
        write(directory.path(), name, body);
    }
    let mut args = vec!["--definitions", "*.rb"];
    args.extend_from_slice(extra);
    ruby_run(directory.path(), &args)
}

/// Direct and fluent actions share their root and method, so near wording is reported.
#[test]
fn regression_ruby_action_evidence_direct_and_fluent_actions_are_near() {
    let run = ruby_action_run(&[("steps.rb", "When('I click the save button') { page.click('#save') }\nWhen('I click save button') { page.locator('#save').click() }\n")], &[]);
    assert_eq!(run.definitions, 2);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.active, vec!["near-duplicate-step"]);
    let run = ruby_action_run(&[("steps.rb", "When('I click the save button') { page.click('#save') }\nWhen('I click save button') { page.fill('#save') }\n")], &[]);
    assert_eq!(run.active, Vec::<String>::new());
}

/// Writes an entry file that loads `helpers` dependency-only files, one indented
/// `require_relative` per line (so a diagnostic column is 3, not 1), then an unused step.
fn ruby_dependency_project(helpers: usize) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let mut entry = String::new();
    for index in 1..=helpers {
        entry.push_str(&format!("  require_relative '../../lib/h{index}'\n"));
        write(
            directory.path(),
            &format!("lib/h{index}.rb"),
            &format!("H{index} = 1\n"),
        );
    }
    entry.push_str("Given('unused step') { work(1) }\n");
    write(
        directory.path(),
        "features/step_definitions/entry.rb",
        &entry,
    );
    write(
        directory.path(),
        "features/a.feature",
        "Feature: f\n  Scenario: s\n    Given another step\n",
    );
    directory
}

/// More selected Ruby files than the graph limit, with no loads, is a complete run.
#[test]
fn regression_ruby_dependency_limits_do_not_charge_selected_entry_points() {
    // More selected Ruby files than the 1,024-module graph limit, and no requires: nothing is
    // expanded, so the run is complete. Before the fix the selection itself overflowed the limit
    // and discovery failed the run with exit 2.
    let directory = tempfile::tempdir().unwrap();
    for index in 1..=1_100 {
        write(
            directory.path(),
            &format!("features/step_definitions/s{index}.rb"),
            &format!("Given('step {index}') {{ work({index}) }}\n"),
        );
    }
    write(
        directory.path(),
        "features/a.feature",
        "Feature: f\n  Scenario: s\n    Given step 1\n",
    );
    let run = ruby_run(directory.path(), &["--definitions", "features/**/*.rb"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.definitions, 1_100);
    assert!(
        !run.stderr.contains("dependency discovery"),
        "{}",
        run.stderr
    );
    // No provider is involved, so exceeding the provider-proof budget costs nothing here.
    assert!(!run.stderr.contains("proof limit"), "{}", run.stderr);
}

/// A load past the Ruby load file limit is not followed and makes the run incomplete.
#[test]
fn regression_ruby_dependency_module_limit_marks_loads_incomplete() {
    let selection = ["--definitions", "features/step_definitions/*.rb"];
    // Control: 1,024 dependency-only helpers fit the graph, so the run is complete and the unused
    // step is reported.
    let fits = ruby_dependency_project(1_024);
    let run = ruby_run(fits.path(), &selection);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(!run.incomplete, "{}", run.stderr);
    assert!(
        run.active.contains(&"unused-definition".to_owned()),
        "{:?}",
        run.active
    );
    assert!(!run.stderr.contains("not followed"), "{}", run.stderr);

    // The 1,025th helper exceeds the limit: its load is not followed, the diagnostic names the
    // limit at that load (line 1,025, not line 1), the run is incomplete rather than failed, and
    // unused-definition findings are withheld because a step may live behind the unfollowed load.
    let over = ruby_dependency_project(1_025);
    let run = ruby_run(over.path(), &selection);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.incomplete, "{}", run.stderr);
    assert!(
        run.stderr.contains(
            "entry.rb:1025:3: Ruby source dependency was not followed: the source graph exceeds the 1,024-file limit"
        ),
        "{}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("dependency discovery"),
        "{}",
        run.stderr
    );
    assert!(
        !run.active.contains(&"unused-definition".to_owned()),
        "{:?}",
        run.active
    );
    let strict = ruby_run(
        over.path(),
        &[
            "--definitions",
            "features/step_definitions/*.rb",
            "--fail-on-incomplete",
        ],
    );
    assert_eq!(strict.code, Some(2), "{}", strict.stderr);
}

/// Cross-file provider registrations that need provider proofs: `(provider.rb, a.rb body)`.
const RUBY_PROVIDER_SHAPES: [(&str, &str); 2] = [
    // `method(:Given)` re-exported through a module constant.
    (
        "ROOT_GIVEN = method(:Given)\nmodule Provider\n  GIVEN = ::ROOT_GIVEN\nend\n",
        "register = Provider::GIVEN\nregister.call('provided step') { work(1) }\nregister.call('provided step') { work(1) }\n",
    ),
    // A top-level constant alias called directly.
    (
        "MY_GIVEN = method(:Given)\n",
        "MY_GIVEN.call('provided step') { work(1) }\nMY_GIVEN.call('provided step') { work(1) }\n",
    ),
];

/// Writes provider `shape` from [`RUBY_PROVIDER_SHAPES`] beside `fillers` plain step files, all
/// selected.
fn ruby_provider_project(fillers: usize, (provider, steps): (&str, &str)) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "features/support/provider.rb", provider);
    write(
        directory.path(),
        "features/step_definitions/a.rb",
        &format!("require_relative '../support/provider'\n{steps}"),
    );
    for index in 1..=fillers {
        write(
            directory.path(),
            &format!("features/step_definitions/f{index}.rb"),
            &format!("Given('filler {index}') {{ work({index}) }}\n"),
        );
    }
    write(
        directory.path(),
        "features/a.feature",
        "Feature: f\n  Scenario: s\n    Given provided step\n",
    );
    directory
}

/// Provider proofs over a selection larger than their budget are unavailable; the suite-wide
/// uncertainty names that budget instead of blaming metaprogramming alone.
#[test]
fn regression_ruby_provider_proof_limit_names_its_cause() {
    let selection = ["--definitions", "features/step_definitions/*.rb"];
    // Every cross-file provider registration rests on a capture such as `method(:Given)`. Without
    // proofs that capture is unresolved, so registry trust is withdrawn and the run is incomplete:
    // provider-backed registrations are never dropped silently.
    for shape in RUBY_PROVIDER_SHAPES {
        // Control: a small suite proves the provider and reports its duplicate registrations.
        let small = ruby_provider_project(1, shape);
        let run = ruby_run(small.path(), &selection);
        assert!(!run.incomplete, "{}", run.stderr);
        assert!(
            run.active.contains(&"duplicate-matcher".to_owned()),
            "{:?}",
            run.active
        );
        assert!(!run.stderr.contains("proof limit"), "{}", run.stderr);

        // 1,100 selected files exceed the 1,024-file proof budget: the provider stays unproven,
        // registry trust is withdrawn (fail closed) and the warning states why.
        let large = ruby_provider_project(1_099, shape);
        let run = ruby_run(large.path(), &selection);
        assert_eq!(run.code, Some(0), "{}", run.stderr);
        assert!(run.incomplete, "{}", run.stderr);
        assert_eq!(run.definitions, 0);
        assert!(
            run.stderr.contains(
                "metaprogramming prevents trusted registration extraction across the selected suite: provider proofs are unavailable because the selected and loaded Ruby files exceed the 1,024-file / 64 MiB proof limit"
            ),
            "{}",
            run.stderr
        );
    }
}

/// Provider proofs missing for a reason other than the budget (an unreadable selected file) do
/// not blame the proof limit.
#[cfg(unix)]
#[test]
fn regression_ruby_provider_proofs_missing_for_other_reasons_do_not_blame_the_limit() {
    use std::os::unix::fs::PermissionsExt;
    let project = ruby_provider_project(2, RUBY_PROVIDER_SHAPES[0]);
    let locked = project.path().join("features/step_definitions/f2.rb");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // Root ignores file modes; there is no unreadable file to observe.
    if fs::read(&locked).is_ok() {
        return;
    }
    let run = ruby_run(
        project.path(),
        &["--definitions", "features/step_definitions/*.rb"],
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        run.stderr.contains(
            "metaprogramming prevents trusted registration extraction across the selected suite"
        ),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("proof limit"), "{}", run.stderr);
}

/// Commits a Ruby suite with support file `base_env` at HEAD~1, then switches it to `head_env`
/// and appends a step at HEAD, and runs `--baseline-from-ref HEAD~1` with `extra` arguments;
/// returns the exit code and stderr.
fn ruby_baseline_run(
    (base_env, head_env): (Option<&str>, Option<&str>),
    steps: &str,
    extra: &[&str],
) -> (Option<i32>, String) {
    let directory = tempfile::tempdir().unwrap();
    let env = directory.path().join("features/support/env.rb");
    if let Some(contents) = base_env {
        write(directory.path(), "features/support/env.rb", contents);
    }
    write(directory.path(), "features/step_definitions/a.rb", steps);
    write(
        directory.path(),
        "features/a.feature",
        "Feature: f\n  Scenario: s\n    Given same\n",
    );
    init_repository(directory.path());
    match head_env {
        Some(contents) => write(directory.path(), "features/support/env.rb", contents),
        None => {
            let _ = fs::remove_file(&env);
        }
    }
    write(
        directory.path(),
        "features/step_definitions/a.rb",
        &format!("{steps}Given('other') {{ visit('/c') }}\n"),
    );
    for args in [vec!["add", "-A"], vec!["commit", "-qm", "head"]] {
        assert!(fixture_git()
            .args(args)
            .current_dir(directory.path())
            .status()
            .unwrap()
            .success());
    }
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(directory.path())
        .args([".", "--definitions", "features/**/*.rb", "--no-metrics"])
        .args(["--baseline-from-ref", "HEAD~1"])
        .args(extra)
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A baseline is rejected for being empty only when the current tree has definitions to match;
/// when both trees are empty (a Ruby registry withdrawn by an unresolved include in both) the run
/// follows the plain-run incompleteness policy.
#[test]
fn regression_ruby_baseline_tolerates_definitions_withdrawn_by_finalization() {
    let invalidated = Some("include Capybara::DSL\n");
    let steps = "Given('same') { visit('/a') }\nGiven('same') { visit('/b') }\n";
    let soft = "baseline revision `HEAD~1` is incomplete, so a finding it did not extract may appear as new";
    let rejected = "baseline revision `HEAD~1` is incomplete:";
    let unmatched =
        "baseline revision `HEAD~1` is incomplete: it kept no step definitions while the current tree has";
    // Both revisions invalidate the registry, so both are empty and nothing can appear new:
    // tolerated and reported by default. Before the fix the baseline was rejected with exit 2.
    let (code, stderr) = ruby_baseline_run((invalidated, invalidated), steps, &[]);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains(soft), "{stderr}");
    assert!(
        !stderr.contains(rejected) && !stderr.contains(unmatched),
        "{stderr}"
    );
    // `--fail-on-incomplete` still rejects the incomplete baseline.
    let (code, stderr) =
        ruby_baseline_run((invalidated, invalidated), steps, &["--fail-on-incomplete"]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains(rejected), "{stderr}");
    // The base is invalidated but HEAD is fixed: an empty base would report every current finding
    // as new, so it is rejected and the message says why.
    let (code, stderr) = ruby_baseline_run((invalidated, None), steps, &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains(&format!("{unmatched} 3")), "{stderr}");
    // Control: a base that kept nothing for no reported reason is rejected the same way.
    let (code, stderr) = ruby_baseline_run((None, None), "HELPER = 1\n", &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert!(stderr.contains(&format!("{unmatched} 1")), "{stderr}");
}

/// A dotted require such as `pages.v2` loads `pages.v2.rb`, as Ruby does, so the loaded file's
/// steps join the analysis; a native-suffix request stays unresolved only when a native
/// library could shadow it.
#[test]
fn regression_ruby_dotted_require_paths_load_the_ruby_file() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "features/support/pages.v2.rb",
        "Given('same step') { visit('/a') }\n",
    );
    write(
        directory.path(),
        "features/step_definitions/a.rb",
        "\nrequire_relative '../support/pages.v2'\nGiven('same step') { visit('/b') }\n",
    );
    write(
        directory.path(),
        "features/a.feature",
        "Feature: f\n  Scenario: s\n    Given same step\n",
    );
    let selection = ["--definitions", "features/step_definitions/*.rb"];
    // Before the fix the load stayed unresolved: the run was incomplete and `pages.v2.rb`, which
    // only the require reaches, contributed no definition, so the duplicate went unseen.
    let run = ruby_run(directory.path(), &selection);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.definitions, 2, "{}", run.stderr);
    assert!(
        run.active.contains(&"duplicate-matcher".to_owned()),
        "{:?}",
        run.active
    );

    // Control: written as `pages.so`, the load falls back to `pages.so.rb` like Ruby, but a native
    // candidate (`pages.bundle`) would win on some platform, so the load stays unresolved and the
    // diagnostic points at it (line 2, column 1).
    write(
        directory.path(),
        "features/support/pages.so.rb",
        "Given('same step') { visit('/a') }\n",
    );
    write(
        directory.path(),
        "features/support/pages.bundle",
        "not a library\n",
    );
    write(
        directory.path(),
        "features/step_definitions/a.rb",
        "\nrequire_relative '../support/pages.so'\nGiven('same step') { visit('/b') }\n",
    );
    let run = ruby_run(directory.path(), &selection);
    assert!(run.incomplete, "{}", run.stderr);
    assert!(
        run.stderr
            .contains("a.rb:2:1: Ruby source dependency is unresolved"),
        "{}",
        run.stderr
    );
    // Without the native candidate the same load resolves to `pages.so.rb`.
    fs::remove_file(directory.path().join("features/support/pages.bundle")).unwrap();
    let run = ruby_run(directory.path(), &selection);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.definitions, 2, "{}", run.stderr);
}

/// A Ruby file skipped by a suffix-agnostic pattern but loaded through `require_relative` is
/// analyzed, so the skipped-files warning does not count it.
#[test]
fn regression_ruby_loaded_files_are_not_reported_as_skipped() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "features/step_definitions/a.rb",
        "require_relative '../support/helpers'\nGiven('one') { help }\n",
    );
    write(
        directory.path(),
        "features/support/helpers.rb",
        "Given('two') { help }\n",
    );
    write(
        directory.path(),
        "features/a.feature",
        "Feature: f\n  Scenario: s\n    Given one\n    Given two\n",
    );
    let selection = [
        "--definitions",
        "features/step_definitions/*.rb",
        "--definitions",
        "features/support/**",
    ];
    // The only file the suffix-agnostic pattern skipped is loaded and analyzed: both steps count
    // and no warning claims it was not analyzed. Before the fix the warning reported 1 file.
    let run = ruby_run(directory.path(), &selection);
    assert_eq!(run.definitions, 2, "{}", run.stderr);
    assert!(!run.stderr.contains("not analyzed"), "{}", run.stderr);
    // Through a symlinked root, discovery records paths under the link while dependency
    // resolution records canonical targets; the comparison must still match them.
    #[cfg(unix)]
    {
        let links = tempfile::tempdir().unwrap();
        let root = links.path().join("suite");
        std::os::unix::fs::symlink(directory.path(), &root).unwrap();
        let run = ruby_run(&root, &selection);
        assert_eq!(run.definitions, 2, "{}", run.stderr);
        assert!(!run.stderr.contains("not analyzed"), "{}", run.stderr);
    }
    // Control: a skipped file nothing loads is still reported, and only that one.
    write(directory.path(), "features/support/unused.rb", "X = 1\n");
    let run = ruby_run(directory.path(), &selection);
    assert!(
        run.stderr
            .contains("warning: 1 cucumber-ruby source file(s) were not analyzed"),
        "{}",
        run.stderr
    );
}

/// The closed-load-phase suite: an entry with a duplicate pair (the positive control) that loads
/// a helper class whose instance method dispatches dynamically.
const CLOSED_ENTRY: &str =
    "require_relative 'helper'\nGiven('same step') { work(1) }\nGiven('same step') { work(2) }\n";
const CLOSED_HELPER: &str =
    "class LocalFormattingHelper\n  def log(level, message)\n    logger.send(level, message)\n  end\nend\n";

/// A matrix row: a label and the files it overrides or adds to the closed-load-phase suite.
type LoadPhaseRow = (&'static str, Vec<(&'static str, String)>);
/// A kept matrix row: a label, its files, and the exact definition count it keeps.
type KeptLoadPhaseRow = (&'static str, Vec<(&'static str, String)>, u64);

/// Runs the closed-load-phase suite with the given file overrides and extra files.
fn closed_load_phase_run(files: &[(&str, &str)]) -> RubyReleaseRun {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "steps/entry.rb", CLOSED_ENTRY);
    write(directory.path(), "steps/helper.rb", CLOSED_HELPER);
    write(
        directory.path(),
        "features/a.feature",
        "Feature: f\n  Scenario: s\n    Given same step\n",
    );
    for (path, contents) in files {
        write(directory.path(), path, contents);
    }
    ruby_run(directory.path(), &["--definitions", "steps/*.rb"])
}

/// A dynamic dispatch inside an instance method of a suite class does not invalidate
/// registrations when every load-time statement of the suite is a registration, a literal load or
/// a plain class definition, and every load resolves; any other load-time statement anywhere in
/// the suite restores the suite-wide invalidation.
#[test]
fn regression_ruby_contained_dispatch_needs_a_closed_load_phase() {
    let helper = |body: &str| format!("class LocalFormattingHelper\n{body}\nend\n");
    let dispatch = "  def log(level, message)\n    logger.send(level, message)\n  end";
    let kept: [KeptLoadPhaseRow; 13] = [
        ("closed suite", vec![], 2),
        (
            "helper used only inside a step handler",
            vec![("steps/uses.rb", "Given('uses helper') { LocalFormattingHelper.new.log(:info, 'x') }\n".to_owned())],
            3,
        ),
        (
            "scenario hook and ParameterType with lambda transformers",
            vec![("steps/hooks.rb", "# frozen_string_literal: true\nBefore('@tag') do\n  setup\nend\nParameterType(name: 'color', regexp: /red|blue/, transformer: ->(c) { c })\nParameterType(name: 'size', regexp: /big/, transformer: lambda { |s| s })\n".to_owned())],
            2,
        ),
        (
            "hook- and protocol-named instance methods",
            vec![("steps/helper.rb", helper("  def initialize\n    logger.send(level, message)\n  end\n  def method_missing(*)\n    logger.send(level, message)\n  end\n  def to_s\n    logger.send(level, message)\n  end"))],
            2,
        ),
        (
            "nested and qualified declarations",
            vec![("steps/helper.rb", format!("module Support\n  class LocalFormattingHelper\n{dispatch}\n  end\nend\nmodule Support::Formatting\n{dispatch}\nend\n"))],
            2,
        ),
        (
            "nested constant named like a core class",
            vec![("steps/helper.rb", format!("module Support\n  class String\n{dispatch}\n  end\nend\n"))],
            2,
        ),
        (
            "dispatch nested in a block inside the instance method",
            vec![("steps/helper.rb", helper("  def log(levels)\n    levels.each { |level| logger.send(level) }\n  end"))],
            2,
        ),
        (
            "transitive and repeated literal loads",
            vec![
                ("steps/entry.rb", format!("require_relative 'middle'\nrequire_relative 'middle'\n{}", CLOSED_ENTRY.lines().skip(1).collect::<Vec<_>>().join("\n"))),
                ("steps/middle.rb", "require_relative 'helper'\n".to_owned()),
            ],
            2,
        ),
        (
            "plain require through a load path",
            vec![
                (".cuke-dedup.json", r#"{"rubyLoadPaths":["lib"]}"#.to_owned()),
                ("steps/entry.rb", CLOSED_ENTRY.replacen("require_relative 'helper'", "require 'helper'", 1)),
                ("lib/helper.rb", CLOSED_HELPER.to_owned()),
                ("steps/helper.rb", "# moved to lib\n".to_owned()),
            ],
            2,
        ),
        (
            "eval and define_method inside an instance method",
            vec![("steps/helper.rb", helper("  def install(code)\n    eval(code)\n    self.class.send(:define_method, :Given) { |*| nil }\n  end"))],
            2,
        ),
        (
            "unresolved require inside a step handler",
            vec![("steps/uses.rb", "Given('loads json') { require 'json' }\n".to_owned())],
            3,
        ),
        (
            "unresolved require inside an instance method",
            vec![("steps/helper.rb", helper(&format!("{dispatch}\n  def parse\n    require 'json'\n  end")))],
            2,
        ),
        (
            "comment-only file beside the suite",
            vec![("steps/notes.rb", "# frozen_string_literal: true\n".to_owned())],
            2,
        ),
    ];
    for (label, files, definitions) in kept {
        let files: Vec<_> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
        let run = closed_load_phase_run(&files);
        assert_eq!(run.definitions, definitions, "{label}: {}", run.stderr);
        assert!(
            run.active.contains(&"duplicate-matcher".to_owned()),
            "{label}: {:?}",
            run.active
        );
    }
    let opened: [LoadPhaseRow; 14] = [
        (
            "constant assignment at top level",
            vec![("steps/other.rb", "HELPER_LEVEL = 1\n".to_owned())],
        ),
        (
            "World with a module",
            vec![("steps/other.rb", "World(Support)\n".to_owned())],
        ),
        (
            "instantiation in the class body",
            vec![(
                "steps/helper.rb",
                helper(&format!("{dispatch}\n  new.log(:info, 'x')")),
            )],
        ),
        (
            "module_function in the class body",
            vec![(
                "steps/helper.rb",
                format!("module LocalFormattingHelper\n{dispatch}\n  module_function :log\nend\n"),
            )],
        ),
        (
            "def inside class_exec",
            vec![(
                "steps/helper.rb",
                helper(&format!("  Object.class_exec do\n  {dispatch}\n  end")),
            )],
        ),
        (
            "superclass",
            vec![(
                "steps/helper.rb",
                format!("class LocalFormattingHelper < Struct\n{dispatch}\nend\n"),
            )],
        ),
        (
            "reopened core class",
            vec![(
                "steps/helper.rb",
                format!("class String\n{dispatch}\nend\n"),
            )],
        ),
        (
            "Marshal.load at top level",
            vec![(
                "steps/other.rb",
                "Marshal.load(File.binread('x'))\n".to_owned(),
            )],
        ),
        (
            "interpolated registration argument",
            vec![(
                "steps/other.rb",
                "Given(\"#{PREFIX} step\") { work }\n".to_owned(),
            )],
        ),
        (
            "constant registration argument",
            vec![("steps/other.rb", "Given(STEP_TEXT) { work }\n".to_owned())],
        ),
        (
            "unresolved require",
            vec![("steps/other.rb", "require 'capybara/cucumber'\n".to_owned())],
        ),
        (
            "singleton method",
            vec![(
                "steps/helper.rb",
                helper("  def self.log(level, message)\n    logger.send(level, message)\n  end"),
            )],
        ),
        (
            "open file sorted before the helper",
            vec![("steps/a_open.rb", "puts 'loading'\n".to_owned())],
        ),
        (
            "open file sorted after the helper",
            vec![("steps/z_open.rb", "puts 'loading'\n".to_owned())],
        ),
    ];
    for (label, files) in opened {
        let files: Vec<_> = files.iter().map(|(p, c)| (*p, c.as_str())).collect();
        let run = closed_load_phase_run(&files);
        assert_eq!(run.definitions, 0, "{label}: {}", run.stderr);
        assert!(run.incomplete, "{label}: {}", run.stderr);
    }
    // A prepared file that cannot be read is never extracted, yet the runner still loads it: the
    // suite is open even though every file the analyzer saw is closed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "steps/entry.rb", CLOSED_ENTRY);
        write(directory.path(), "steps/helper.rb", CLOSED_HELPER);
        write(directory.path(), "steps/locked.rb", "# closed\n");
        write(
            directory.path(),
            "features/a.feature",
            "Feature: f\n  Scenario: s\n    Given same step\n",
        );
        let locked = directory.path().join("steps/locked.rb");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // Root ignores file modes, so the file stays readable and the suite stays closed.
        let readable = fs::read(&locked).is_ok();
        let run = ruby_run(directory.path(), &["--definitions", "steps/*.rb"]);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            run.definitions,
            if readable { 2 } else { 0 },
            "{}",
            run.stderr
        );
    }
    // When the suite opens, the warning names the contained dispatch call (helper.rb, line 3).
    let run = closed_load_phase_run(&[("steps/other.rb", "HELPER_LEVEL = 1\n")]);
    assert!(
        run.stderr.contains("Ruby call at")
            && run.stderr.contains("helper.rb:3:5 that may redefine the DSL or dispatch dynamically can run during load"),
        "{}",
        run.stderr
    );
}

/// A dynamic call inside a scenario hook block runs only during scenarios, like one inside a step
/// handler, so it keeps registrations; hooks that can run around loading, or hook calls that are
/// not top-level and receiverless, keep the suite-wide withdrawal.
#[test]
fn regression_ruby_scenario_hook_blocks_are_deferred_like_step_handlers() {
    let run_with = |hooks: &str| {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "steps/entry.rb",
            "Given('same step') { work(1) }\nGiven('same step') { work(2) }\n",
        );
        write(directory.path(), "steps/hooks.rb", hooks);
        write(
            directory.path(),
            "features/a.feature",
            "Feature: f\n  Scenario: s\n    Given same step\n",
        );
        ruby_run(directory.path(), &["--definitions", "steps/*.rb"])
    };
    for hook in [
        "Before do\n  helper.send(kind)\nend\n",
        "Before('@tag') do |scenario|\n  helper.send(kind)\nend\n",
        "After { helper.public_send(kind) }\n",
        "Around do |scenario, block|\n  helper.send(kind)\n  block.call\nend\n",
        "AfterStep { |result| eval(code) }\n",
        "BeforeStep do\n  helper.__send__(kind)\nend\n",
    ] {
        let run = run_with(hook);
        assert_eq!(run.definitions, 2, "{hook}: {}", run.stderr);
        assert!(
            run.active.contains(&"duplicate-matcher".to_owned()),
            "{hook}: {:?}",
            run.active
        );
    }
    // Each withdrawal is located at the dynamic call itself, never at line 1, column 1.
    for (hook, location) in [
        ("BeforeAll do\n  helper.send(kind)\nend\n", "hooks.rb:2:3"),
        ("AfterAll { helper.send(kind) }\n", "hooks.rb:1:12"),
        (
            "AfterConfiguration do |config|\n  helper.send(kind)\nend\n",
            "hooks.rb:2:3",
        ),
        (
            "InstallPlugin do |config, registry|\n  helper.send(kind)\nend\n",
            "hooks.rb:2:3",
        ),
        ("self.Before do\n  helper.send(kind)\nend\n", "hooks.rb:2:3"),
        (
            "if ENV['CI']\n  Before do\n    helper.send(kind)\n  end\nend\n",
            "hooks.rb:3:5",
        ),
        // A redefined hook may run its block at load (`def self.Before` on main precedes the
        // extended DSL), so redefining one withdraws trust like redefining a step keyword.
        (
            "def self.Before(*)\n  yield\nend\nBefore do\n  helper.send(kind)\nend\n",
            "hooks.rb:1:1",
        ),
        (
            "define_singleton_method(:After) { |*a, &b| b.call }\nAfter { helper.send(kind) }\n",
            "hooks.rb:1:1",
        ),
    ] {
        let run = run_with(hook);
        assert_eq!(run.definitions, 0, "{hook}: {}", run.stderr);
        assert!(
            run.stderr.contains(&format!(
                "{location}: Ruby registration ownership or executable source effects are unresolved"
            )),
            "{hook}: {}",
            run.stderr
        );
    }
}

/// A zero-operand `load()` is an ordinary call; identical callback bodies under different
/// wrappers are near, different bodies under one wrapper are not.
#[test]
fn regression_ruby_action_evidence_callbacks_and_bare_load_calls() {
    let steps = "When('the admin applies the discount rule') { with_transaction { load_cart(); apply_discount(); save() } }\nWhen('the admin applies the shipping rule') { with_transaction { load_order(); apply_shipping(); commit() } }\nThen('the invoice grid is refreshed') { alpha { load(); render(); assert_rows() } }\nThen('the invoice list is refreshed') { beta { load(); render(); assert_rows() } }\n";
    let run = ruby_action_run(&[("steps.rb", steps)], &[]);
    assert_eq!(run.definitions, 4);
    assert!(!run.incomplete, "{}", run.stderr);
    assert_eq!(run.active, vec!["near-duplicate-step"]);
    let run = ruby_action_run(
        &[("steps.rb", &steps.replace("load();", "load('rows.rb');"))],
        &[],
    );
    assert!(run.incomplete);
    assert!(run.stderr.contains("steps.rb:3:49"), "{}", run.stderr);
}

/// Identical event streams behind differing values are never near and never take a candidate
/// slot from a pair that is.
#[test]
fn regression_ruby_action_evidence_identical_streams_are_not_near() {
    let steps = "Given('shipment is ready') { write_status(:ready) }\nThen('shipment is now ready') { write_status(:ready) }\nGiven('shipment is rejected') { write_status(:rejected) }\nThen('shipment is now rejected') { write_status(:accepted) }\nGiven('the archive badge is visible') { page.goto('/login') }\nGiven('the archive badge is now visible') { page.goto('/login'); page.wait_for_load_state() }\n";
    let run = ruby_action_run(&[("steps.rb", steps)], &[]);
    let mut active = run.active.clone();
    active.sort();
    assert_eq!(
        active,
        vec![
            "duplicate-handler",
            "near-duplicate-step",
            "near-duplicate-step"
        ]
    );
    // With one comparison, the decoy pairs (identical streams, differing symbols) must not take
    // the slot: the only near finding is the archive pair.
    let decoys = "Given('shipment is rejected') { write_status(:rejected) }\nThen('shipment is now rejected') { write_status(:accepted) }\nGiven('the archive badge is visible') { page.goto('/login') }\nGiven('the archive badge is now visible') { page.goto('/login'); page.wait_for_load_state() }\n";
    let run = ruby_action_run(
        &[("steps.rb", decoys)],
        &["--max-candidate-comparisons", "1"],
    );
    assert_eq!(run.active, vec!["near-duplicate-step"]);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
}

/// Untrusted RSpec expectations keep the exact-handler policy: duplicates stay, polarity and
/// value differences never become near or parameterization findings.
#[test]
fn regression_ruby_action_evidence_untrusted_expectations_keep_exact_policy() {
    let steps = "Given('the exact assertion') { expect(exact_parcel).to eq('ready') }\nThen('another exact assertion') { expect(exact_parcel).to eq('ready') }\nGiven('the parcel is verified') { expect(parcel).to eq('ready') }\nThen('the parcel is now verified') { expect(parcel).not_to eq('ready') }\nGiven('the parcel is ready') { expect(shipment).to eq('ready') }\nGiven('the parcel is idle') { expect(shipment).to eq('idle') }\n";
    let run = ruby_action_run(&[("steps.rb", steps)], &[]);
    assert_eq!(run.definitions, 6);
    assert!(!run.incomplete, "{}", run.stderr);
    // The identical pair's wording is far apart, so only the duplicate is reported.
    assert_eq!(run.active, vec!["duplicate-handler"]);
}

/// Ruby files reached only through `require_relative` or a `rubyLoadPaths` `require` report paths
/// relative to the analysis root, in findings and in discovery output. On Windows a raw
/// `canonicalize()` returns verbatim `\\?\C:\...` targets that do not strip against the root.
#[test]
fn regression_119_ruby_loaded_file_paths_stay_relative_to_the_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // Each file's pair shares a body no other file has, so every finding stays within one file.
    let handlers = |action: &str| {
        format!("Given('the cart is empty') do\n  {action}(:all)\nend\n\nGiven('the basket has nothing') do\n  {action}(:all)\nend\n")
    };
    write(root, ".cuke-dedup.json", r#"{"rubyLoadPaths":["lib"]}"#);
    write(
        root,
        "entry.rb",
        "require_relative 'providers/steps'\nrequire 'shared/steps'\n",
    );
    write(root, "providers/steps.rb", &handlers("clear_cart"));
    write(root, "lib/shared/steps.rb", &handlers("empty_basket"));
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(root)
        .args([
            "--reporters",
            "jsonl",
            "--no-metrics",
            "--explain-discovery",
        ])
        .args(["--definitions", "entry.rb"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut paths: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|row| row["type"] == "finding" && row["rule"] == "duplicate-handler")
        .flat_map(|row| {
            let related = row["related"].as_array().unwrap().clone();
            std::iter::once(row["primary"].clone()).chain(related)
        })
        .map(|location| {
            format!(
                "{}:{}",
                location["path"].as_str().unwrap(),
                location["line"]
            )
        })
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "lib/shared/steps.rb:1",
            "lib/shared/steps.rb:5",
            "providers/steps.rb:1",
            "providers/steps.rb:5"
        ],
        "{stderr}"
    );
    for relative in ["providers/steps.rb", "lib/shared/steps.rb"] {
        let shown = relative.replace('/', std::path::MAIN_SEPARATOR_STR);
        assert!(
            stderr.contains(&format!("cuke-dedup: definition {shown}\n")),
            "{stderr}"
        );
    }
}

/// Ruby handlers that only mark their step pending are trivial, as in the TypeScript frontend: an
/// empty body, or one `pending`/`todo`/`not_implemented` call, stub-label string, `nil`, bare
/// `next`/`return` of one, or `raise`/`fail` with a stub label. Inline blocks, do-blocks, lambdas
/// and `&method` handlers share the rule; real work, an unlabelled `raise NotImplementedError` (as
/// TypeScript's `throw new NotImplementedError()`) and two statements stay duplicates.
#[test]
fn regression_ruby_pending_stub_handlers_are_not_duplicates() {
    let inline = |body: &str| {
        format!("Given('pending one') {{ {body} }}\nGiven('pending two') {{ {body} }}\n")
    };
    let named = |declaration: &str| {
        format!("{declaration}\nGiven('pending one', &method(:stub))\nGiven('pending two', &method(:stub))\n")
    };
    let mut rows: Vec<(String, bool)> = [
        "pending",
        "pending('todo')",
        "pending 'todo'",
        "self.pending",
        "(pending)",
        "not_implemented",
        "raise 'pending'",
        "fail 'not implemented'",
        "raise NotImplementedError, 'todo'",
        "raise Cucumber::Pending",
        "fail todo",
        // TypeScript parity: `throw new Boom(audit('todo'))` and `pending(work())` are trivial.
        "raise Boom, audit('todo')",
        "pending(work())",
        "next",
        "next pending",
        // Call-free parity rows: TypeScript stubs that the existing call-free rule already covers.
        "next 'pending'",
        "nil",
        "'TODO'",
    ]
    .iter()
    .map(|body| (inline(body), false))
    .collect();
    rows.extend([
        // Both handlers carry the trailing comment: one stub alone already excludes the pair.
        ("Given('pending one') do\n  pending\n  # waiting on the API\nend\nGiven('pending two') do\n  pending\n  # waiting on the API\nend\n".to_owned(), false),
        ("Given('pending one', &-> { pending })\nGiven('pending two', &-> { pending })\n".to_owned(), false),
        ("Given('pending one', &lambda { fail 'todo' })\nGiven('pending two', &lambda { fail 'todo' })\n".to_owned(), false),
        (named("def stub = pending"), false),
        (named("def stub; end"), false),
        (named("def stub\n  return\nend"), false),
        (named("def stub\n  return pending\nend"), false),
        (inline("raise NotImplementedError"), true),
        (inline("raise 'boom'"), true),
        (inline("raise message"), true),
        (inline("raise Boom, report.todo"), true),
        (inline("work()"), true),
        (inline("pending; work()"), true),
        (inline("next work()"), true),
        (named("def stub = work"), true),
    ]);
    for (steps, duplicate) in rows {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "steps.rb", &steps);
        write(
            directory.path(),
            "pending.feature",
            "Feature: pending\n  Scenario: steps\n    Given pending one\n    Given pending two\n",
        );
        let run = ruby_run(directory.path(), &["--definitions", "steps.rb"]);
        assert_eq!(run.definitions, 2, "{steps}: {}", run.stderr);
        assert!(!run.incomplete, "{steps}: {}", run.stderr);
        let expected: &[&str] = if duplicate {
            &["duplicate-handler", "near-duplicate-step"]
        } else {
            &[]
        };
        let mut active = run.active.clone();
        active.sort();
        assert_eq!(active, expected, "{steps}");
    }
    // Named handlers with different bodies in two files never duplicate. Each registration
    // captures the definition current in its selected file, including when names coincide.
    for (left, right) in [("handler", "handler"), ("alpha", "beta")] {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "a.rb",
            &format!("def {left}; do_alpha(); end\nGiven('alpha step', &method(:{left}))\n"),
        );
        write(
            directory.path(),
            "b.rb",
            &format!("def {right}; do_beta(); end\nGiven('bravo step', &method(:{right}))\n"),
        );
        write(
            directory.path(),
            "pending.feature",
            "Feature: pending\n  Scenario: steps\n    Given alpha step\n",
        );
        let run = ruby_run(directory.path(), &["--definitions", "*.rb"]);
        assert_eq!(run.definitions, 2, "{left}/{right}: {}", run.stderr);
        assert!(!run.incomplete, "{left}/{right}");
        assert!(
            !run.active.iter().any(|rule| rule == "duplicate-handler"),
            "{left}/{right}"
        );
    }
}

/// At a global comparison limit equal to the comparisons a corpus needs, analysis is complete in
/// both languages: witnesses that propose nothing cost no comparison, so the matcher-overlap pass
/// is not truncated. One comparison short of a real overlap still truncates and fails strict mode.
#[test]
fn regression_exact_global_limit_does_not_truncate_matcher_overlap() {
    let ts = |body: &str| format!("import {{ Given }} from '@cucumber/cucumber';\n{body}");
    let rows = [
        (
            "steps.ts",
            ts("Given('same step', () => { work(1); });\nGiven('same step', () => { work(2); });\n"),
            "1",
            false,
            0,
        ),
        (
            "steps.ts",
            ts("Given('operation one', () => perform(1));\nGiven('operation two', () => perform(2));\nGiven('operation three', () => perform(3));\n"),
            "3",
            false,
            0,
        ),
        (
            "steps.ts",
            ts("Given('same step', () => { work(1); });\nGiven('same step', () => { work(2); });\nGiven(/red$/, () => { left(); });\nGiven('pick red', () => { right(); });\n"),
            "2",
            false,
            1,
        ),
        (
            "steps.ts",
            ts("Given('same step', () => { work(1); });\nGiven('same step', () => { work(2); });\nGiven(/red$/, () => { left(); });\nGiven('pick red', () => { right(); });\n"),
            "1",
            true,
            0,
        ),
        (
            "steps.rb",
            "Given('same step') { work(1) }\nGiven('same step') { work(2) }\n".to_owned(),
            "1",
            false,
            0,
        ),
        (
            "steps.rb",
            "Given('operation one') { page.perform(1) }\nGiven('operation two') { page.perform(2) }\nGiven('operation three') { page.perform(3) }\n".to_owned(),
            "3",
            false,
            0,
        ),
        (
            "steps.rb",
            "Given('same step') { work(1) }\nGiven('same step') { work(2) }\nGiven(/red$/) { left() }\nGiven('pick red') { right() }\n".to_owned(),
            "2",
            false,
            1,
        ),
        (
            "steps.rb",
            "Given('same step') { work(1) }\nGiven('same step') { work(2) }\nGiven(/red$/) { left() }\nGiven('pick red') { right() }\n".to_owned(),
            "1",
            true,
            0,
        ),
    ];
    for (file, steps, limit, truncated, overlap) in rows {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), file, &steps);
        write(
            directory.path(),
            "limits.feature",
            "Feature: limits\n  Scenario: steps\n    Given same step\n",
        );
        let output = Command::cargo_bin("cuke-dedup")
            .unwrap()
            .arg(directory.path())
            .args([
                "--definitions",
                file,
                "--reporters",
                "jsonl",
                "--no-metrics",
            ])
            .args(["--fail-on-incomplete", "--max-candidate-comparisons", limit])
            .args(["--max-structural-class-comparisons", "3"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let row = format!("{file} limit {limit}: {steps}\n{stderr}");
        let records: Vec<Value> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect();
        let summary = records
            .iter()
            .find(|record| record["type"] == "summary")
            .unwrap();
        assert_eq!(
            records
                .iter()
                .filter(|record| record["rule"] == "overlapping-matcher")
                .count(),
            overlap,
            "{row}"
        );
        let analysis = &summary["analysis"];
        assert_eq!(analysis["truncated"], truncated, "{row}");
        assert_eq!(
            analysis["skippedCandidateComparisons"],
            u64::from(truncated),
            "{row}"
        );
        assert_eq!(
            analysis["candidateSources"]["matcherOverlap"]["skipped"],
            u64::from(truncated),
            "{row}"
        );
        assert_eq!(
            analysis["candidateSources"]["matcherOverlap"]["evaluated"], overlap as u64,
            "{row}"
        );
        assert_eq!(
            stderr.contains("static matcher-overlap analysis is incomplete"),
            truncated,
            "{row}"
        );
        if truncated {
            assert_eq!(output.status.code(), Some(2), "{row}");
        } else {
            assert_ne!(output.status.code(), Some(2), "{row}");
        }
    }
}

/// A step keyword called through a receiver fails closed at the call whenever Cucumber-Ruby could
/// register it, because any receiver may evaluate to main (`def me = self`, a local rebound through
/// `binding` or `binding_of_caller`), including a local instance of a source class. The keywords
/// take `(pattern, symbol = nil, options = {}, &proc)` with a String or Regexp pattern and
/// `symbol || proc`: shapes Ruby rejects (no pattern, a non-pattern literal, a second argument that
/// is neither Symbol nor Proc, no proc) stay complete, as Watir and Builder `a(...)` calls do.
/// Known registrations keep their findings.
#[test]
fn regression_122_ruby_receiver_registrations_fail_closed() {
    let unresolved =
        "Ruby registration through a receiver is not resolved; definitions may be missing";
    let foreign = "class Foreign; def Given(*) = nil; end; foreign = Foreign.new";
    let namespace = "module Ns; def self.Given(text, &handler) = [text, handler]; end; ns = Ns";
    for (prelude, call, location) in [
        (
            "def me = self",
            "me.Given('hidden') { other() }",
            Some("4:1"),
        ),
        (
            "target = self",
            "target.When('hidden') { other() }",
            Some("4:1"),
        ),
        (
            "def wrap(target) = target.Then('hidden') { other() }",
            "wrap(self)",
            Some("3:20"),
        ),
        ("", "@browser.a('hidden') { other() }", Some("4:1")),
        ("", "Steps.But('hidden') { other() }", Some("4:1")),
        (
            "handler = -> { other() }",
            "router.Given('hidden', &handler)",
            Some("4:1"),
        ),
        (
            "handler = -> { other() }",
            "router.Given('hidden', nil, &handler)",
            Some("4:1"),
        ),
        ("", "router.Given('hidden', false) { other() }", Some("4:1")),
        ("", "router.Given('hidden', :step_method)", Some("4:1")),
        ("", "router.Given('hidden', :\"step_method\")", Some("4:1")),
        ("", "router.Given((1)) { other() }", None),
        (
            "name = :step_method",
            "router.Given('hidden', name)",
            Some("4:1"),
        ),
        ("", "router.Given('hidden', -> { other() })", Some("4:1")),
        (
            "args = ['hidden', :step_method]",
            "router.Given(*args)",
            Some("4:1"),
        ),
        (
            "def wrap(...) = target.Given(...)",
            "wrap('hidden') { other() }",
            Some("3:17"),
        ),
        (foreign, "foreign.Given('hidden') { other() }", Some("4:1")),
        (namespace, "ns.Given('hidden') { other() }", Some("4:1")),
        (
            foreign,
            "foreign.Given(self).When('hidden') { other() }",
            Some("4:1"),
        ),
        // Every String or Regexp pattern form registers with a block.
        ("", "router.Given(/x/) { other() }", Some("4:1")),
        ("", "router.Given(%r{x}) { other() }", Some("4:1")),
        ("", "router.Given(%q(x)) { other() }", Some("4:1")),
        ("", "router.Given(?x) { other() }", Some("4:1")),
        ("", "router.Given('a' 'b') { other() }", Some("4:1")),
        (
            "",
            "router.Given(<<~TEXT) { other() }\ntext\nTEXT",
            Some("4:1"),
        ),
        (
            "def wrap(...) = target.Given('hidden', nil, ...)",
            "wrap { other() }",
            Some("3:17"),
        ),
        (
            "def wrap(...) = target.Given('hidden', nil, {}, ...)",
            "wrap { other() }",
            Some("3:17"),
        ),
        (
            "def wrap(...) = target.Given('hidden', false, ...)",
            "wrap { other() }",
            Some("3:17"),
        ),
        (
            "def wrap(&) = target.Given('hidden', &)",
            "wrap { other() }",
            Some("3:15"),
        ),
        (
            "opts = {}",
            "router.Given('hidden', **opts) { other() }",
            Some("4:1"),
        ),
        ("args = []", "router.Given(42, *args) { other() }", None),
        // A sign written directly before a number is part of the literal; a spaced one is a call.
        ("", "router.Given(-1) { other() }", None),
        ("", "router.Given('hidden', -1) { other() }", None),
        ("", "router.Given('hidden', - 1) { other() }", Some("4:1")),
        // Arguments after a splat are fixed too.
        (
            "rest = []",
            "router.Given('hidden', *rest, 1, 2, 3) { other() }",
            None,
        ),
        (
            "rest = []",
            "router.Given(*rest, 'hidden', 'b') { other() }",
            None,
        ),
        (
            "rest = []",
            "router.Given(*rest, 'hidden', :step_method)",
            Some("4:1"),
        ),
        // `__LINE__` and `__ENCODING__` are never a pattern; `__FILE__` is a String.
        ("", "router.Given(__LINE__) { other() }", None),
        ("", "router.Given(__ENCODING__) { other() }", None),
        ("", "router.Given('hidden', __FILE__)", None),
        ("", "router.Given('hidden', __LINE__)", None),
        // Hundreds of splats stay bounded: four fixed arguments always exceed the cap.
        (
            "rest = []",
            &format!(
                "router.Given({}1, 1, 1, 1) {{ other() }}",
                "*rest, ".repeat(300)
            ),
            None,
        ),
        // Every splat or `...` supplies its own number of arguments.
        (
            "rest = []",
            "router.Given('hidden', *rest, :step_method, *rest, {})",
            Some("4:1"),
        ),
        (
            "rest = []",
            "router.Given(*rest, *rest, 'hidden', :step_method)",
            Some("4:1"),
        ),
        (
            "def wrap(...) = target.Given(*pre, 'hidden', :step_method, {}, ...)",
            "wrap",
            Some("3:17"),
        ),
        // `false.to_proc` and the backtick method can be redefined to return a Proc or Symbol.
        ("", "router.Given('hidden', &false)", Some("4:1")),
        ("", "router.Given('hidden', &(nil))", None),
        ("", "router.Given('hidden', `true`)", Some("4:1")),
        ("", "router.Given('hidden', <<~`CMD`)\nls\nCMD", Some("4:1")),
        (
            "",
            "router.Given('hidden', nil, {}, key: 1) { other() }",
            None,
        ),
        (
            "rest = []",
            "router.Given('hidden', 'b', *rest) { other() }",
            None,
        ),
        (
            "rest = []",
            "router.Given('hidden', nil, {}, 4, *rest) { other() }",
            None,
        ),
        ("", "router.Given('hidden', ('second')) { other() }", None),
        ("", "router.Given(-> { other() }) { other() }", None),
        ("", "router.Given('hidden', nil, {}, 4) { other() }", None),
        // Each literal that is neither a String nor a Regexp is rejected as a pattern.
        ("", "router.Given(:\"x\") { other() }", None),
        ("", "router.Given(1) { other() }", None),
        ("", "router.Given(1.5) { other() }", None),
        ("", "router.Given(1r) { other() }", None),
        ("", "router.Given(1i) { other() }", None),
        ("", "router.Given(nil) { other() }", None),
        ("", "router.Given(true) { other() }", None),
        ("", "router.Given(false) { other() }", None),
        ("", "router.Given([]) { other() }", None),
        ("", "router.Given(%w[a]) { other() }", None),
        ("", "router.Given(%i[a]) { other() }", None),
        ("", "router.Given({}) { other() }", None),
        ("", "router.Given(1..2) { other() }", None),
        // Shapes Ruby rejects: no proc (MissingProc), a pattern that is not a String or Regexp,
        // no pattern at all, or a second argument that is neither Symbol nor Proc.
        ("", "@browser.a(text: 'Go').click", None),
        ("", "browser.a('Go')", None),
        ("link = 'Go'", "browser.a(:text, link)", None),
        ("", "browser.a(:text) { other() }", None),
        (
            "def page(xml) = xml.a(href: '/x') { xml.text('Go') }",
            "page(nil)",
            None,
        ),
        ("", "router.Given { other() }", None),
        ("", "router.Given('hidden')", None),
        ("", "router.Given('hidden', &nil)", None),
        ("", "router.Given('hidden', 'second') { other() }", None),
        ("", "router.Given('hidden', true) { other() }", None),
        ("", "router.Given('hidden', key: 1) { other() }", None),
        ("", "router.Given('hidden', <<~TEXT)\ntext\nTEXT", None),
        ("opts = {}", "router.Given('hidden', **opts)", None),
        // A literal non-empty `**hash` is always a Hash in the second slot, so no block is used.
        ("", "router.Given('hidden', **{key: 1}) { other() }", None),
        ("", "self.Given('hidden')", None),
        // A second argument that is a literal neither Symbol nor Proc raises ArgumentError.
        ("", "router.Given('hidden', 'a' 'b')", None),
        ("", "router.Given('hidden', ?c)", None),
        ("", "router.Given('hidden', 1)", None),
        ("", "router.Given('hidden', 1.5)", None),
        ("", "router.Given('hidden', 1r)", None),
        ("", "router.Given('hidden', 1i)", None),
        ("", "router.Given('hidden', /x/)", None),
        ("", "router.Given('hidden', [])", None),
        ("", "router.Given('hidden', %w[a])", None),
        ("", "router.Given('hidden', %i[a])", None),
        ("", "router.Given('hidden', {})", None),
        ("", "router.Given('hidden', 1..2)", None),
        ("", "router.Given('hidden', nil)", None),
        ("", "router.Given('hidden', false)", None),
        ("", "record.Given", None),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "steps.rb",
            &format!("{RUBY_RELEASE_STEPS}{prelude}\n{call}\n"),
        );
        write(directory.path(), "release.feature", RUBY_RELEASE_FEATURE);
        let run = ruby_run(
            directory.path(),
            &["--definitions", "steps.rb", "--fail-on-incomplete"],
        );
        let row = format!("{prelude} / {call}: {}", run.stderr);
        assert_eq!(run.definitions, 2, "{row}");
        assert_eq!(run.active, ["duplicate-handler"], "{row}");
        assert_eq!(run.incomplete, location.is_some(), "{row}");
        assert_eq!(
            run.code,
            Some(if location.is_some() { 2 } else { 1 }),
            "{row}"
        );
        assert_eq!(run.stderr.contains(unresolved), location.is_some(), "{row}");
        if let Some(location) = location {
            assert!(
                run.stderr
                    .contains(&format!("steps.rb:{location}: {unresolved}")),
                "{row}"
            );
        }
    }
}

/// Every `%r` delimiter registers like the slash literal with the same source: a second
/// registration with a different handler is reported as a duplicate matcher, which needs both
/// matchers recognized. Controls: a different pattern under the same delimiter is distinct, and a
/// literal Ruby rejects stays incomplete instead of registering.
#[test]
fn regression_191_ruby_percent_r_delimiters_are_recognized() {
    for (slash, percent) in [
        (r"/\Aitem 1\z/", r"%r!\Aitem 1\z!"),
        (r"/\Aitem (1)\z/", r"%r(\Aitem (1)\z)"),
        (r"/\Aitem [1]\z/", r"%r[\Aitem [1]\z]"),
        (r"/\Aitem <1>\z/", r"%r<\Aitem <1>\z>"),
        (r"/\Aitem1{1}\z/", r"%r{\Aitem1{1}\z}"),
        (r"/\Aitem 1\|2\z/", r"%r|\Aitem 1\|2\z|"),
        (r"/\Aitem!1\z/", r"%r!\Aitem\!1\z!"),
        (r"/\Aitem\/1\z/", r"%r{\Aitem/1\z}"),
        (r"/\Aitem [a-c]\z/", r"%r-\Aitem [a\-c]\z-"),
        (r"/\Aitem 1\z/m", r"%r(\Aitem 1\z)m"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(
            directory.path(),
            "steps.rb",
            &format!("Given({slash}) {{ first() }}\nGiven({percent}) {{ second() }}\n"),
        );
        let run = ruby_run(directory.path(), &["--definitions", "steps.rb"]);
        assert_eq!(run.definitions, 2, "{percent}: {}", run.stderr);
        assert!(!run.incomplete, "{percent}: {}", run.stderr);
        assert!(
            run.active.iter().any(|rule| rule == "duplicate-matcher"),
            "{percent}: {:?}",
            run.active
        );
        // Control: the same delimiter with another pattern is not a duplicate.
        let distinct = percent.replace("item", "other");
        write(
            directory.path(),
            "steps.rb",
            &format!("Given({slash}) {{ first() }}\nGiven({distinct}) {{ second() }}\n"),
        );
        let run = ruby_run(directory.path(), &["--definitions", "steps.rb"]);
        assert_eq!(run.definitions, 2, "{distinct}: {}", run.stderr);
        assert!(!run.incomplete, "{distinct}: {}", run.stderr);
        assert!(
            !run.active.iter().any(|rule| rule == "duplicate-matcher"),
            "{distinct}: {:?}",
            run.active
        );
    }
    // Unmodeled control: `|` closes the literal, so `b|` is not a flag and Ruby rejects the file.
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.rb",
        "Given(/\\Aitem 1\\z/) { first() }\nGiven(%r|\\Aitem 1\\z|b|) { second() }\n",
    );
    let run = ruby_run(directory.path(), &["--definitions", "steps.rb"]);
    assert!(run.incomplete, "{}", run.stderr);
}

/// A stray `<` before a placeholder does not hide it: `the <a<b> value` expands to `the <a7 value`
/// and uses the definition for that text, leaving the one for the unexpanded text unused. Controls:
/// a plain placeholder and an unknown name expand or stay as the table says, so the unused line
/// moves with the expansion instead of being fixed by a blanket "never substitute".
#[test]
fn regression_186_outline_placeholder_after_a_stray_angle_bracket_is_substituted() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@cucumber/cucumber';\nGiven('the <a7 value', () => first());\nGiven('the <a<b> value', () => second());\nGiven('plain 7 and <z>', () => third());\n",
    );
    write(
        directory.path(),
        "a.feature",
        "Feature: f\n  Scenario Outline: s\n    Given the <a<b> value\n    And plain <b> and <z>\n    Examples:\n      | b |\n      | 7 |\n",
    );
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(directory.path())
        .args(["--reporters", "jsonl", "--no-metrics"])
        .output()
        .unwrap();
    let unused: Vec<u64> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|row| row["type"] == "finding" && row["rule"] == "unused-definition")
        .map(|row| row["primary"]["line"].as_u64().unwrap())
        .collect();
    assert_eq!(unused, [3], "{}", String::from_utf8_lossy(&output.stderr));
}

/// An ambiguous feature step reports an end column one past its last character whatever whitespace
/// follows the keyword, identically in JSON and SARIF. Controls: the one-space step keeps the old
/// column, and the Markdown and outline steps report the converted and source lines.
#[test]
fn regression_187_step_end_column_follows_the_source_line() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from '@cucumber/cucumber';\nGiven(/^the /, () => first());\nGiven(/panel/, () => second());\n",
    );
    write(
        directory.path(),
        "a.feature",
        "Feature: f\n  Scenario Outline: s\n    Given the panel opens\n    When   the panel opens\n    Then \t the panel <n>\n    And    the panel 日本\n    Examples:\n      | n |\n      | 1 |\n",
    );
    write(
        directory.path(),
        "b.feature.md",
        "# Feature: f\n\n## Scenario: s\n\n* Given   the panel opens\n",
    );
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(directory.path())
        .args(["--reporters", "json,sarif", "--no-metrics"])
        .output()
        .unwrap();
    let reports = directory.path().join("reports/cuke-dedup");
    let json: Value =
        serde_json::from_str(&fs::read_to_string(reports.join("cuke-dedup.json")).unwrap())
            .unwrap();
    let sarif: Value =
        serde_json::from_str(&fs::read_to_string(reports.join("cuke-dedup.sarif")).unwrap())
            .unwrap();
    let mut json_spans: Vec<_> = json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["rule"] == "ambiguous-step")
        .map(|finding| {
            let primary = &finding["primary"];
            (
                primary["path"].as_str().unwrap().to_owned(),
                primary["line"].as_u64().unwrap(),
                primary["column"].as_u64().unwrap(),
                primary["endColumn"].as_u64().unwrap(),
            )
        })
        .collect();
    json_spans.sort();
    let expected = [
        ("a.feature", 3, 5, 26),
        ("a.feature", 4, 5, 27),
        ("a.feature", 5, 5, 25),
        ("a.feature", 6, 5, 24),
        ("b.feature.md", 5, 3, 26),
    ];
    assert_eq!(
        json_spans,
        expected.map(|(path, line, column, end)| (path.to_owned(), line, column, end)),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut sarif_spans: Vec<_> = sarif["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|result| result["ruleId"] == "ambiguous-step")
        .map(|result| {
            let region = &result["locations"][0]["physicalLocation"]["region"];
            (
                result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                region["startLine"].as_u64().unwrap(),
                region["startColumn"].as_u64().unwrap(),
                region["endColumn"].as_u64().unwrap(),
            )
        })
        .collect();
    sarif_spans.sort();
    assert_eq!(sarif_spans, json_spans);
}

/// An import of a non-ECMAScript file is not parsed with another grammar. A barrel that re-exports
/// a Ruby, JSON, CSS or feature file keeps the registration it also re-exports, and direct imports
/// from such a file are reported unresolved without a syntax-error cause. Control: a malformed
/// TypeScript module still fails closed with its syntax-error cause.
#[test]
fn regression_190_non_ecmascript_import_targets_are_not_parsed() {
    let esm = "export { When } from '@cucumber/cucumber';\n";
    for (target, body) in [
        ("support.rb", esm),
        ("support.rb", "# ruby\nGiven(/a/) { }\n"),
        ("support.json", "{}\n"),
        ("support.css", "a {}\n"),
        ("support.feature", "Feature: f\n"),
        ("support.d.ts", esm),
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "package.json", "{}");
        write(directory.path(), target, body);
        write(
            directory.path(),
            "barrel.ts",
            &format!(
                "export * from './{target}';\nexport {{ Given }} from '@cucumber/cucumber';\n"
            ),
        );
        write(
            directory.path(),
            "steps.ts",
            "import { Given } from './barrel';\nGiven('s', () => work());\n",
        );
        write(
            directory.path(),
            "a.feature",
            "Feature: f\n  Scenario: s\n    Given s\n",
        );
        let run = ruby_run(directory.path(), &[]);
        assert_eq!(run.definitions, 1, "{target}: {}", run.stderr);
        assert!(!run.incomplete, "{target}: {}", run.stderr);
        assert!(
            !run.stderr.contains("syntax errors"),
            "{target}: {}",
            run.stderr
        );
    }
    for import in [
        "import { When } from './support.rb';\nWhen('s', () => work());\n",
        "import * as bdd from './support.rb';\nbdd.When('s', () => work());\n",
        "import bdd from './support.rb';\nbdd.When('s', () => work());\n",
        "import bdd = require('./support.rb');\nbdd.When('s', () => work());\n",
        "const { When } = require('./support.rb');\nWhen('s', () => work());\n",
        "const bdd = require('./support.rb');\nbdd.When('s', () => work());\n",
    ] {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "package.json", "{}");
        write(
            directory.path(),
            "support.rb",
            "export { When } from '@cucumber/cucumber';\n",
        );
        write(directory.path(), "steps.ts", import);
        write(
            directory.path(),
            "a.feature",
            "Feature: f\n  Scenario: s\n    Given s\n",
        );
        let run = ruby_run(directory.path(), &[]);
        assert_eq!(run.definitions, 0, "{import}: {}", run.stderr);
        assert!(run.incomplete, "{import}: {}", run.stderr);
        assert!(
            !run.stderr.contains("syntax errors"),
            "{import}: {}",
            run.stderr
        );
    }
    // Control: the same barrel over a malformed TypeScript module is refused with its cause.
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "package.json", "{}");
    write(directory.path(), "support.ts", "const broken = ;\n");
    write(
        directory.path(),
        "barrel.ts",
        "export * from './support.ts';\nexport { Given } from '@cucumber/cucumber';\n",
    );
    write(
        directory.path(),
        "steps.ts",
        "import { Given } from './barrel';\nGiven('s', () => work());\n",
    );
    write(
        directory.path(),
        "a.feature",
        "Feature: f\n  Scenario: s\n    Given s\n",
    );
    let run = ruby_run(directory.path(), &[]);
    assert!(run.incomplete, "{}", run.stderr);
    assert!(run.stderr.contains("syntax errors"), "{}", run.stderr);
}

/// Checks a Gherkin parse error lists its expected tokens in one order across process runs.
#[test]
fn regression_gherkin_parse_error_token_order_is_deterministic() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/ruby-parity/gherkin/malformed-table-neighbor");
    // A CRLF checkout (Windows CI) parses to a different expected-token set; an LF copy pins one
    // input on every platform.
    let directory = tempfile::tempdir().unwrap();
    for name in [
        "steps.rb",
        "features/missing-final-pipe.feature",
        "features/ragged-extra-cell.feature",
        "features/valid-neighbor.feature",
    ] {
        let contents = fs::read_to_string(fixture.join(name)).unwrap();
        write(directory.path(), name, &contents.replace("\r\n", "\n"));
    }
    let root = directory.path();
    // Each process seeds the parser's `HashSet` differently; twelve unsorted runs of a two-token
    // set all print the sorted order by chance with probability 2^-12.
    let runs = (0..12)
        .map(|_| {
            let output = analyze_root_with(root, &["--definitions", "**/*.rb"])
                .code(2)
                .get_output()
                .clone();
            String::from_utf8(output.stderr).unwrap()
        })
        .collect::<Vec<_>>();
    assert!(runs.iter().all(|stderr| stderr == &runs[0]), "{runs:#?}");
    let line = runs[0]
        .lines()
        .find(|line| line.contains("missing-final-pipe.feature"))
        .unwrap();
    assert!(
        line.starts_with("cuke-dedup: failed to parse Gherkin feature "),
        "{line}"
    );
    assert!(
        line.ends_with(r#"missing-final-pipe.feature: unterminated Gherkin table row at line 7: Error at 8:1: {"_", "unknown keyword"}"#),
        "{line}"
    );
}

/// Runs `steps.rb` and `steps.feature` through the built binary with the Ruby definition pattern.
fn ruby_escape_run(steps: &str, feature: &str) -> RubyReleaseRun {
    let directory = tempfile::tempdir().unwrap();
    write(directory.path(), "steps.rb", steps);
    write(directory.path(), "steps.feature", feature);
    ruby_run(directory.path(), &["--definitions", "steps.rb"])
}

/// Every escape Ruby decodes in a double-quoted matcher gives the value Ruby builds: the escaped
/// spelling duplicates its plain twin and matches the feature step, while a matcher that differs
/// by one character does neither. Controls: unmodeled escapes (control and meta forms, bytes that
/// are not UTF-8, `\x` without digits) keep the run incomplete, and a single-quoted literal keeps
/// its backslash.
#[test]
fn regression_192_ruby_double_quoted_escapes_decode_to_ruby_values() {
    for (escaped, twin, step) in [
        (
            r#""tab\sseparated""#,
            "'tab separated'",
            Some("tab separated"),
        ),
        (
            r#""the \e[1mbold\e[0m step""#,
            "'the \u{1b}[1mbold\u{1b}[0m step'",
            None,
        ),
        (r#""bell\a\b\f\v""#, "'bell\u{7}\u{8}\u{c}\u{b}'", None),
        (r#""item \x41""#, "'item A'", Some("item A")),
        (r#""item \101""#, "'item A'", Some("item A")),
        (r#""caf\xC3\xA9""#, "'café'", Some("café")),
        (r#""caf\303\251""#, "'café'", Some("café")),
        (r#""keep\d+""#, "'keepd+'", Some("keepd+")),
        (r#""nul\0end""#, "'nul\u{0}end'", None),
    ] {
        let steps = format!("Given({escaped}) {{ first() }}\nGiven({twin}) {{ second() }}\n");
        let feature = format!(
            "Feature: escapes\n  Scenario: one\n    Given {}\n",
            step.unwrap_or("unrelated")
        );
        let run = ruby_escape_run(&steps, &feature);
        assert_eq!(run.definitions, 2, "{escaped}: {}", run.stderr);
        assert!(!run.incomplete, "{escaped}: {}", run.stderr);
        assert!(
            run.active.iter().any(|rule| rule == "duplicate-matcher"),
            "{escaped}: {:?}",
            run.active
        );
        if step.is_some() {
            // The decoded matchers match the feature step, so neither is unused.
            assert!(
                !run.active.iter().any(|rule| rule == "unused-definition"),
                "{escaped}: {:?}",
                run.active
            );
        }
        // Control: against an unrelated matcher the escaped one is no duplicate.
        let steps =
            format!("Given({escaped}) {{ first() }}\nGiven('unrelated text') {{ second() }}\n");
        let run = ruby_escape_run(&steps, &feature);
        assert!(!run.incomplete, "{escaped}: {}", run.stderr);
        assert!(
            !run.active.iter().any(|rule| rule == "duplicate-matcher"),
            "{escaped}: {:?}",
            run.active
        );
    }
    // A single-quoted literal keeps `\s`, so it is not the decoded double-quoted matcher.
    let run = ruby_escape_run(
        "Given(\"tab\\sseparated\") { first() }\nGiven('tab\\\\sseparated') { second() }\n",
        "Feature: escapes\n  Scenario: one\n    Given tab separated\n",
    );
    assert!(!run.incomplete, "{}", run.stderr);
    assert!(
        !run.active.iter().any(|rule| rule == "duplicate-matcher"),
        "{:?}",
        run.active
    );
    // Unmodeled controls: the matcher is indeterminate and the run incomplete at its own line.
    for unmodeled in [
        r#""a\cAb""#,
        r#""a\C-ab""#,
        r#""a\M-ab""#,
        r#""bad \xFF byte""#,
        r#""bad \351 byte""#,
    ] {
        let run = ruby_escape_run(
            &format!("Given('first') {{ first() }}\n\nGiven({unmodeled}) {{ second() }}\n"),
            "Feature: escapes\n  Scenario: one\n    Given first\n",
        );
        assert_eq!(run.definitions, 1, "{unmodeled}: {}", run.stderr);
        assert!(run.incomplete, "{unmodeled}: {}", run.stderr);
        assert!(
            run.stderr.contains("steps.rb:3:1"),
            "{unmodeled}: {}",
            run.stderr
        );
    }
}

/// A `ParameterType` name and regexp written with escapes resolve like their plain spelling, so
/// the `{channel}` registrations compare; an unmodeled escape in the regexp keeps the parameter
/// type unresolved and the run incomplete.
#[test]
fn regression_192_ruby_parameter_type_escapes_decode_like_plain_text() {
    let run = |parameter: &str| {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), "a.rb", parameter);
        write(
            directory.path(),
            "steps.rb",
            "Given('channel {channel}') { first() }\nThen('channel {channel}') { second() }\n",
        );
        write(
            directory.path(),
            "steps.feature",
            "Feature: channels\n Scenario: output\n  Given channel stdout\n",
        );
        ruby_run(directory.path(), &["--definitions", "*.rb"])
    };
    let plain = run("ParameterType(name: 'channel', regexp: 'stdout|stderr', transformer: ->(value) { value })\n");
    let escaped = run(
        r#"ParameterType(name: "chan\x6eel", regexp: "std\x6fut|stderr", transformer: ->(value) { value })"#,
    );
    assert!(!plain.incomplete, "{}", plain.stderr);
    assert!(
        plain.active.iter().any(|rule| rule == "duplicate-matcher"),
        "{:?}",
        plain.active
    );
    assert_eq!(escaped.incomplete, plain.incomplete, "{}", escaped.stderr);
    assert_eq!(escaped.active, plain.active, "{}", escaped.stderr);
    let unmodeled = run(
        r#"ParameterType(name: "channel", regexp: "std\cAut|stderr", transformer: ->(value) { value })"#,
    );
    assert!(unmodeled.incomplete, "{}", unmodeled.stderr);
}
