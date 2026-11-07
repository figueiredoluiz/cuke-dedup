/// Builds empty-value rows whose output is tiny but each template traversal scans 64 KiB.
pub fn shrinking_outline(rows: usize) -> String {
    let header = "v".repeat(62);
    let template = format!("<{header}>").repeat(1_024);
    format!(
        "Feature: shrinking\n Scenario Outline: rows\n  Given {template}\n Examples:\n | {header} |\n{}",
        " | |\n".repeat(rows)
    )
}
