//! Helpers shared by the Ruby integration suites.
use serde_json::Value;

/// Sorted, deduplicated handler-comparison rules among the active findings.
pub fn active_handler_rules(rows: &[Value]) -> Vec<String> {
    let mut rules: Vec<String> = rows
        .iter()
        .filter(|row| row["type"] == "finding" && row["active"] == true)
        .filter_map(|row| row["rule"].as_str())
        .filter(|rule| {
            matches!(
                *rule,
                "duplicate-handler" | "near-duplicate-step" | "parameterization-candidate"
            )
        })
        .map(str::to_owned)
        .collect();
    rules.sort();
    rules.dedup();
    rules
}
