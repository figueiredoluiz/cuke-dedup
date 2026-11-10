use assert_cmd::Command;
use cuke_dedup::gherkin;
use serde_json::Value;
use std::{fs, path::Path, time::Duration};

#[path = "common/feature_expansion.rs"]
mod feature_fixtures;

#[path = "common/git_fixture.rs"]
mod git_fixture;

/// Reads the complete real CLI report without normalizing or discarding fields.
fn json_report(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("reports/cuke-dedup/cuke-dedup.json")).unwrap())
        .unwrap()
}

/// Independent public calls reset successful count, retained-payload and template-scan consumption.
#[test]
fn public_feature_calls_reset_each_successfully_consumed_allowance() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("fresh.feature");
    for (source, count) in [
        (outline(500, 251, 1), 125_500),
        (outline(200, 42, 4096), 8_400),
        (feature_fixtures::shrinking_outline(513), 513),
    ] {
        fs::write(&path, &source).unwrap();
        for _ in 0..2 {
            assert_eq!(gherkin::extract(&source, &path).unwrap().len(), count);
            assert_eq!(gherkin::extract_file(&path).unwrap().len(), count);
        }
    }
}

/// Real invocation/history extraction shares its allowance, while another invocation starts fresh.
#[test]
fn reference_baseline_shares_feature_allowance_and_new_invocations_reset_it() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("steps.ts"), "import { Given } from '@cucumber/cucumber'; Given('noise x',()=>first()); Given('noise x',()=>second());").unwrap();
    let feature = root.path().join("input.feature");
    fs::write(&feature, outline(500, 251, 1)).unwrap();
    let revision = git_fixture::snapshot(root.path());
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root.path())
        .timeout(Duration::from_secs(20))
        .args([
            ".",
            "--baseline-from-ref",
            &revision,
            "--reporters",
            "json",
            "--no-metrics",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        message.contains("baseline revision") && message.contains("concrete steps"),
        "{message}"
    );
    for (small, expected) in [(false, 125_500), (true, 1)] {
        if small {
            fs::write(&feature, outline(1, 1, 1)).unwrap();
        }
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command
            .current_dir(root.path())
            .timeout(Duration::from_secs(20))
            .args([".", "--reporters", "json", "--no-metrics"]);
        if small {
            command.args(["--baseline-from-ref", &revision]);
        }
        let output = command.output().unwrap();
        assert!(
            matches!(output.status.code(), Some(0 | 1)),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = json_report(root.path());
        assert_eq!(report["summary"]["featureStepsAnalyzed"], expected);
        assert_eq!(report["corpus"]["incomplete"], false);
        assert_eq!(report["analysis"]["truncated"], false);
        if !small {
            assert_eq!(report["summary"]["byRule"]["ambiguous-step"], 500);
        }
    }
}

/// Creates a compact outline whose concrete step count and text bytes multiply independently.
fn outline(steps: usize, rows: usize, width: usize) -> String {
    format!(
        "Feature: expansion\n Scenario Outline: rows\n{} Examples:\n | value |\n{}",
        "  Given noise <value>\n".repeat(steps),
        format!(" | {} |\n", "x".repeat(width)).repeat(rows)
    )
}

/// Public extraction must reject count, payload and scan amplification before returning a partial feature.
#[test]
fn public_feature_extraction_refuses_count_and_payload_amplification() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("expanded.feature");
    for (name, source) in [
        ("count", outline(1_000, 251, 1)),
        ("payload", outline(200, 128, 4_096)),
        ("scan", feature_fixtures::shrinking_outline(1_025)),
    ] {
        let expected = if name == "scan" {
            "scanned template bytes"
        } else {
            "feature expansion"
        };
        let error = gherkin::extract(&source, Path::new("expanded.feature"))
            .err()
            .map(|error| format!("{error:#}"));
        assert!(
            error.is_some_and(|error| error.contains(expected)),
            "{name}"
        );
        fs::write(&path, source).unwrap();
        let error = gherkin::extract_file(&path).unwrap_err();
        assert!(format!("{error:#}").contains(expected), "{name}");
    }
    fs::write(
        &path,
        "Feature: fitting\n Scenario: retained\n  Given done\n",
    )
    .unwrap();
    let steps = gherkin::extract_file(&path).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].text, "done");
}

/// Overflow preserves neighboring positive findings, withdraws unused claims and leaves baselines intact.
#[test]
fn feature_expansion_limits_preserve_findings_and_mark_reports_incomplete() {
    for case in [
        "small",
        "count",
        "payload",
        "aggregate",
        "aggregate-payload",
        "scan",
        "aggregate-scan",
    ] {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("steps.ts"), "import { Given } from '@cucumber/cucumber';\nGiven('done', () => first());\nGiven('done', () => second());\nGiven('unused', () => third());\n").unwrap();
        fs::write(
            root.path().join("zz-valid.feature"),
            "Feature: control\n Scenario: retained\n  Given done\n",
        )
        .unwrap();
        let source = match case {
            "small" => outline(1, 1, 1),
            "count" => outline(1_000, 251, 1),
            "payload" => outline(200, 128, 4_096),
            "aggregate-payload" => outline(200, 42, 4_096),
            "scan" => feature_fixtures::shrinking_outline(1_025),
            "aggregate-scan" => feature_fixtures::shrinking_outline(513),
            _ => outline(500, 251, 1),
        };
        fs::write(root.path().join("a.feature"), &source).unwrap();
        if case.starts_with("aggregate") {
            fs::write(root.path().join("b.feature"), &source).unwrap();
        }
        let output = Command::cargo_bin("cuke-dedup")
            .unwrap()
            .current_dir(root.path())
            .timeout(Duration::from_secs(20))
            .args([".", "--reporters", "json,sarif", "--no-metrics"])
            .output()
            .unwrap();
        let incomplete = case != "small";
        assert_eq!(
            output.status.code(),
            Some(if incomplete { 2 } else { 1 }),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = json_report(root.path());
        assert_eq!(report["corpus"]["incomplete"], incomplete, "{case}");
        assert_eq!(
            report["summary"]["featureStepsAnalyzed"],
            match case {
                "small" => 2,
                "aggregate" => 125_501,
                "aggregate-payload" => 8_401,
                "aggregate-scan" => 514,
                _ => 1,
            },
            "{case}"
        );
        assert_eq!(report["summary"]["byRule"]["ambiguous-step"], 1, "{case}");
        assert_eq!(
            report["summary"]["byRule"]["duplicate-matcher"], 1,
            "{case}"
        );
        assert_eq!(
            report["summary"]["byRule"]["unused-definition"],
            if incomplete {
                Value::Null
            } else {
                Value::from(1)
            },
            "{case}"
        );
        let sarif: Value = serde_json::from_slice(
            &fs::read(root.path().join("reports/cuke-dedup/cuke-dedup.sarif")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            sarif["runs"][0]["invocations"][0]["executionSuccessful"], !incomplete,
            "{case}"
        );
        if incomplete {
            let baseline = root.path().join("baseline.json");
            let before = b"{\"schemaVersion\":3,\"fingerprints\":{}}";
            fs::write(&baseline, before).unwrap();
            Command::cargo_bin("cuke-dedup")
                .unwrap()
                .current_dir(root.path())
                .timeout(Duration::from_secs(20))
                .args([
                    ".",
                    "--baseline",
                    "baseline.json",
                    "--update-baseline",
                    "--reporters",
                    "json",
                ])
                .assert()
                .code(2);
            assert_eq!(fs::read(&baseline).unwrap(), before, "{case}");
        }
    }
}
