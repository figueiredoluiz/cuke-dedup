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

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

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

#[test]
fn grammar_limitations_keep_supported_findings_and_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    write(
        root.path(),
        ".cuke-dedup.json",
        r#"{"rules":{"duplicate-matcher":"warning"},"threshold":100}"#,
    );
    write(
        root.path(),
        "clean.ts",
        "Given('clean',()=>first()); Given('clean',()=>second());",
    );
    write(
        root.path(),
        "swallowed.tsx",
        "const icon=<span>&#128465;</span>; Given('swallowed',()=>icon);",
    );
    let output = tempfile::tempdir().unwrap();
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root.path())
        .args([".", "--reporters", "json", "--output"])
        .arg(output.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("syntax errors"));
    let report: Value =
        serde_json::from_str(&fs::read_to_string(output.path().join("cuke-dedup.json")).unwrap())
            .unwrap();
    assert_eq!(report["corpus"]["incomplete"], true);
    let paths: Vec<_> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["rule"] == "duplicate-matcher")
        .map(|finding| finding["primary"]["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["clean.ts"]);
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root.path())
        .args([".", "--fail-on-unparseable"])
        .assert()
        .code(2);
}

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

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(&root)
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

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(&root)
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

#[test]
fn oversized_unchanged_sources_and_features_fail_changed_analysis_closed() {
    let directory = tempfile::tempdir().unwrap();
    write_sized(directory.path(), "steps.ts", 8 * 1024 * 1024 + 1);
    write_sized(
        directory.path(),
        "features/example.feature",
        8 * 1024 * 1024 + 1,
    );
    init_repository(directory.path());
    write(directory.path(), "changed.txt", "changed\n");

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("definition source"))
        .stderr(predicate::str::contains("feature file"));
}

#[test]
fn an_oversized_unchanged_feature_alone_fails_changed_analysis_closed() {
    let directory = tempfile::tempdir().unwrap();
    write_sized(
        directory.path(),
        "features/example.feature",
        8 * 1024 * 1024 + 1,
    );
    init_repository(directory.path());
    write(directory.path(), "changed.txt", "changed\n");

    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(directory.path())
        .args([".", "--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("feature file"))
        .stderr(predicate::str::contains("input limit"));
}

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

#[test]
fn an_unparseable_file_is_tolerated_and_reported_by_default_and_gated_by_flags() {
    let directory = tempfile::tempdir().unwrap();
    write(
        directory.path(),
        "valid.ts",
        "Given('an analyzed step', () => work());\n",
    );
    write(directory.path(), "broken.ts", "Given('broken', () => {\n");

    // Default: the unparseable file does not abort the scan. The valid file's definition is still
    // analyzed and the parse failure is reported, so the run is visibly incomplete, not silently
    // clean.
    analyze_root(directory.path())
        .success()
        .stderr(predicate::str::contains(
            "source contains JavaScript/TypeScript syntax errors",
        ));

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
