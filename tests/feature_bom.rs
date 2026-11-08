use assert_cmd::Command;
use serde_json::{json, Value};
use std::{fs, time::Duration};

/// Preserves JS/Ruby usage and ambiguity findings, neighboring positives and strict malformed outcomes.
#[test]
fn feature_bom_cli_matrix_preserves_findings_and_incomplete_controls() {
    for (file, definitions) in [
        ("steps.ts", "import { Given } from '@cucumber/cucumber';\nGiven('done', () => first());\nGiven('done', () => second());\nGiven('unused', () => third());\n"),
        ("steps.rb", "Given('done') { first }\nGiven('done') { second }\nGiven('unused') { third }\n"),
    ] {
        for (extension, source, malformed) in [
            ("feature", "Feature: input\n Scenario: open\n  Given done\n", false),
            ("feature.md", "`@fast`\n# Feature: input\n## Scenario: open\n* Given done\n", false),
            ("feature", "Feature: bad\n Scenario: open\n  Given done\n    | broken\n", true),
            ("feature.md", "# Feature: bad\n## Scenario: open\n* Given done\n  | broken\n", true),
        ] {
            let root = tempfile::tempdir().unwrap();
            fs::write(root.path().join(file), definitions).unwrap();
            fs::write(root.path().join("zz-valid.feature"), "Feature: neighbor\n Scenario: open\n  Given done\n").unwrap();
            let mut plain: Option<Value> = None;
            for prefix in ["", "\u{feff}"] {
                fs::write(root.path().join(format!("a.{extension}")), format!("{prefix}{source}")).unwrap();
                let output = Command::cargo_bin("cuke-dedup")
                    .unwrap()
                    .current_dir(root.path())
                    .timeout(Duration::from_secs(20))
                    .args([".", "--definitions", file, "--features", "**/*.feature,**/*.feature.md", "--reporters", "json", "--no-metrics", "--require-features", "--require-definitions", "--fail-on-incomplete"])
                    .output().unwrap();
                assert_eq!(output.status.code(), Some(if malformed { 2 } else { 1 }), "{file}/{extension}/{prefix:?}: {}", String::from_utf8_lossy(&output.stderr));
                let report: Value = serde_json::from_slice(&fs::read(root.path().join("reports/cuke-dedup/cuke-dedup.json")).unwrap()).unwrap();
                assert_eq!(report["corpus"]["incomplete"], malformed);
                assert_eq!(report["summary"]["definitionsAnalyzed"], 3);
                assert_eq!(report["summary"]["featureStepsAnalyzed"], if malformed { 1 } else { 2 });
                let expected = if malformed { json!({"ambiguous-step":1,"duplicate-matcher":1}) } else { json!({"ambiguous-step":2,"duplicate-matcher":1,"unused-definition":1}) };
                assert_eq!(report["summary"]["byRule"], expected);
                assert_eq!(report["summary"]["findings"], if malformed { 2 } else { 4 });
                if let Some(baseline) = &plain {
                    for field in ["summary", "corpus", "analysis", "findings"] {
                        assert_eq!(report[field], baseline[field], "{file}/{extension}/{field}");
                    }
                } else {
                    plain = Some(report);
                }
            }
        }
    }
}
