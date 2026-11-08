use assert_cmd::Command;
use serde_json::Value;
use std::{fs, time::Duration};

/// Preserves a complete positive, then reports bounded ambiguity explicitly without replacing a baseline.
#[test]
fn ambiguity_limits_preserve_cli_findings_and_baseline_on_refusal() {
    for (file, definitions) in [
        ("steps.ts", "import { Given } from '@cucumber/cucumber';\nGiven('same',()=>first());\nGiven('same',()=>second());\n"),
        ("steps.rb", "Given('same') { first }; Given('same') { second }"),
    ] {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join(file), definitions).unwrap();
    fs::write(root.join(".cuke-dedup.json"), r#"{"noMetrics":true,"maxCandidateComparisons":1,"rules":{"duplicate-matcher":"off","normalized-matcher":"off","duplicate-handler":"off","near-duplicate-step":"off","parameterization-candidate":"off","unused-definition":"off","overlapping-matcher":"off","ambiguous-step":"warning"}}"#).unwrap();
    let baseline = b"{\"schemaVersion\":3,\"fingerprints\":{}}";
    fs::write(root.join("baseline.json"), baseline).unwrap();
    let mut first_fingerprint = None;
    for (count, expected_exit, expected_findings) in [(1, 0, 1), (25_001, 2, 25_000)] {
        fs::write(
            root.join("input.feature"),
            format!(
                "Feature: F\n Scenario: S\n{}",
                "  Given same\n".repeat(count)
            ),
        )
        .unwrap();
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command
            .current_dir(root)
            .timeout(Duration::from_secs(60))
            .args([
                ".",
                "--definitions",
                file,
                "--features",
                "input.feature",
                "--reporters",
                "jsonl,json,sarif",
                "--require-definitions",
                "--require-features",
                "--fail-on-incomplete",
                "--baseline",
                "baseline.json",
            ]);
        if count > 1 {
            command.arg("--update-baseline");
        }
        let output = command.output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(expected_exit), "{stderr}");
        let records = jsonl_records(&output.stdout);
        let summary = records.last().unwrap();
        assert_eq!(summary["type"], "summary");
        assert_eq!(summary["analysis"]["truncated"], count > 1);
        assert_eq!(
            summary["summary"]["byRule"]["ambiguous-step"],
            expected_findings
        );
        assert_eq!(records[0]["related"].as_array().unwrap().len(), 2);
        if count == 1 {
            first_fingerprint = Some(records[0]["fingerprint"].clone());
        } else {
            assert_eq!(Some(records[0]["fingerprint"].clone()), first_fingerprint);
            assert!(stderr.contains("ambiguity"), "{stderr}");
            assert!(stderr.contains("not updated"), "{stderr}");
        }
        assert_eq!(fs::read(root.join("baseline.json")).unwrap(), baseline);
        let json: Value = serde_json::from_slice(&fs::read(root.join("reports/cuke-dedup/cuke-dedup.json")).unwrap()).unwrap();
        assert_eq!(json["analysis"]["truncated"], count > 1);
        let sarif: Value = serde_json::from_slice(&fs::read(root.join("reports/cuke-dedup/cuke-dedup.sarif")).unwrap()).unwrap();
        assert_eq!(sarif["runs"][0]["invocations"][0]["executionSuccessful"], count == 1);
    }
    }
}

/// Decodes the complete JSON value stream without copying stdout into a second UTF-8 buffer.
fn jsonl_records(bytes: &[u8]) -> Vec<Value> {
    serde_json::Deserializer::from_slice(bytes)
        .into_iter::<Value>()
        .collect::<Result<Vec<_>, _>>()
        .expect("valid JSONL report")
}
