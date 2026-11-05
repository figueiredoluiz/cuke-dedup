use super::*;

/// Nested pretty JSON accounting matches actual indentation and rejects an over-limit write.
#[test]
fn pretty_finding_cost_counts_nested_output_and_enforces_the_limit() {
    let value = serde_json::json!({"text": "界\n\"", "items": [1, {"empty": []}]});
    for depth in [0, 2, 4] {
        let mut nested = value.clone();
        for _ in 0..depth {
            nested = serde_json::json!([nested]);
        }
        let output = serde_json::to_string_pretty(&nested).unwrap();
        let lines = output.lines().collect::<Vec<_>>();
        let finding = lines[depth..lines.len() - depth].join("\n");
        assert_eq!(pretty_json_cost(&value, depth), Some(finding.len() + 2));
    }
    let mut counter = JsonCostCounter {
        bytes: MAX_BUFFERED_REPORT_BYTES - 1,
        extra_indent: 4,
    };
    assert!(counter.write_all(b"\n").is_err());
    assert_eq!(counter.bytes, MAX_BUFFERED_REPORT_BYTES - 1);
    counter.write_all(b"x").unwrap();
    counter.flush().unwrap();
    assert_eq!(counter.bytes, MAX_BUFFERED_REPORT_BYTES);
    assert!(counter.write_all(b"x").is_err());
}
