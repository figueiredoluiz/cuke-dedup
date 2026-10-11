use assert_cmd::Command;
use serde_json::Value;
use std::fs;

#[test]
fn escaped_tables_preserve_findings_and_reject_malformed_neighbors_in_both_languages() {
    for (extension, definitions) in [
        ("rb", "Given('path {word}') { first() }; Then('path {word}') { second() }"),
        ("ts", "import { Given, Then } from '@cucumber/cucumber'; Given('path {word}', () => first()); Then('path {word}', () => second());"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(format!("steps.{extension}")), definitions).unwrap();
        fs::write(dir.path().join("valid.feature"), "Feature: paths\n Scenario Outline: escaped\n  Given path <value>\n Examples:\n  | value |\n  | C:\\tmp\\q |\n").unwrap();
        for bad_row in [None, Some(r"| a\q | b |"), Some(r"| bad\q")] {
            if let Some(row) = bad_row {
                fs::write(dir.path().join("broken.feature"), format!("Feature: invalid\n Scenario Outline: rows\n  Given path <value>\n Examples:\n  | value |\n  {row}\n")).unwrap();
            }
            let output = Command::cargo_bin("cuke-dedup").unwrap()
                .arg(dir.path()).args(["--definitions", &format!("*.{extension}"),
                    "--reporters", "jsonl", "--no-metrics", "--fail-on-incomplete"])
                .output().unwrap();
            assert_eq!(output.status.code(), Some(if bad_row.is_some() { 2 } else { 1 }),
                "{extension}: {}", String::from_utf8_lossy(&output.stderr));
            let rows: Vec<Value> = String::from_utf8(output.stdout).unwrap().lines()
                .map(|line| serde_json::from_str(line).unwrap()).collect();
            let summary = rows.last().unwrap();
            assert_eq!(summary["summary"]["definitionsAnalyzed"], 2);
            assert_eq!(summary["summary"]["featureStepsAnalyzed"], 1);
            assert_eq!(summary["corpus"]["incomplete"], bad_row.is_some());
            assert_eq!(summary["corpus"]["featureFilesParsed"], 1);
            assert_eq!(rows.iter().filter(|r| r["rule"] == "duplicate-matcher").count(), 1);
            let ambiguity = rows.iter().find(|r| r["rule"] == "ambiguous-step").unwrap();
            assert_eq!(ambiguity["primary"]["path"], "valid.feature");
            assert_eq!(ambiguity["primary"]["line"], 3);
        }
    }
}
