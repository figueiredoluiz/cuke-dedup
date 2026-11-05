use super::*;

/// The result builder maps every severity; final reports exclude inactive notes.
#[test]
fn sarif_result_levels_preserve_active_filtering() {
    let root = Path::new("/repo");
    for (severity, level, retained) in [
        (Severity::Error, "error", 1),
        (Severity::Warning, "warning", 1),
        (Severity::Off, "note", 0),
    ] {
        let mut analysis = super::super::tests::result(root);
        analysis.findings[0].severity = severity;
        assert_eq!(sarif_finding(&analysis.findings[0], root)["level"], level);
        let context = ReportContext::new(&analysis, root, 100.0);
        let report: serde_json::Value =
            serde_json::from_str(&render_sarif(&context).unwrap()).unwrap();
        assert_eq!(
            report["runs"][0]["results"].as_array().unwrap().len(),
            retained
        );
    }
}
