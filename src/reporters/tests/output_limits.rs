use super::*;

/// Keeps fixture records small enough to reach the finding ceiling before the byte ceiling.
fn small_result(root: &Path) -> AnalysisResult {
    let mut analysis = result(root);
    analysis.findings[0].evidence.comparison = None;
    analysis.findings[0].related.clear();
    analysis
}

/// Checks real count boundaries, complete totals, and identical string/stream records.
#[test]
fn jsonl_count_limits_preserve_complete_summaries() {
    let root = Path::new("/repo");
    for count in [10_001, 0, 1, 9_999, 10_000] {
        let mut analysis = small_result(root);
        analysis.findings = vec![analysis.findings[0].clone(); count];
        let context = ReportContext::new(&analysis, root, 0.0);
        let output = render_jsonl(&context).unwrap();
        let mut streamed = Vec::new();
        jsonl::write_jsonl_context(&context, None, &mut streamed).unwrap();
        assert_eq!(output.as_bytes(), streamed);
        assert!(output.ends_with('\n'));
        assert!(output.len() <= 16 * 1024 * 1024);
        let summary: serde_json::Value =
            serde_json::from_str(output.lines().last().unwrap()).unwrap();
        let emitted = count.min(10_000);
        assert_eq!(output.lines().count(), emitted + 1, "count={count}");
        assert_eq!(summary["recordCount"], emitted + 1);
        assert_eq!(summary["findingsTruncated"], count - emitted);
        assert_eq!(summary["truncated"], count > emitted);
        assert_eq!(
            summary["summary"],
            serde_json::to_value(&context.summary).unwrap()
        );
    }
}

/// Checks serialized Unicode bytes trigger omissions before the finding ceiling.
#[test]
fn jsonl_byte_limits_preserve_valid_records_and_totals() {
    let root = Path::new("/repo");
    let mut analysis = small_result(root);
    analysis.findings[0].message = "界".repeat(2_000);
    analysis.findings = vec![analysis.findings[0].clone(); 3_000];
    let context = ReportContext::new(&analysis, root, 0.0);
    let output = render_jsonl(&context).unwrap();
    assert!(output.len() <= 16 * 1024 * 1024, "{} bytes", output.len());
    assert!(output.ends_with('\n'));
    let last: serde_json::Value = serde_json::from_str(output.lines().last().unwrap()).unwrap();
    assert_eq!(last["type"], "summary");
    let mut emitted = 0;
    for line in output.lines() {
        let record: serde_json::Value = serde_json::from_str(line).unwrap();
        if record["type"] == "finding" {
            emitted += 1;
            assert_eq!(record["message"], analysis.findings[0].message);
        } else {
            assert_eq!(record["recordCount"], emitted + 1);
            assert_eq!(record["findingsTruncated"], 3_000 - emitted);
            assert_eq!(record["truncated"], true);
            assert_eq!(
                record["summary"],
                serde_json::to_value(&context.summary).unwrap()
            );
        }
    }
    assert!(emitted > 0 && emitted < 3_000);
}

/// Direct Config mutation cannot repeat stdout reports or file-write destinations.
#[test]
fn public_report_dispatch_deduplicates_each_format() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let analysis = small_result(&config.root);
    let context = ReportContext::new(&analysis, &config.root, 100.0);
    for stdout in [ReporterKind::Jsonl, ReporterKind::Terminal] {
        config.reporters = vec![
            ReporterKind::Sarif,
            stdout,
            ReporterKind::Json,
            ReporterKind::Html,
        ];
        let mut expected = Vec::new();
        let expected_paths = write_reports(&context, &config, &mut expected).unwrap();
        assert_eq!(expected_paths.len(), 3);
        config.reporters.extend(config.reporters.clone());
        let mut actual = Vec::new();
        let actual_paths = write_reports(&context, &config, &mut actual).unwrap();
        assert_eq!(actual_paths, expected_paths);
        assert_eq!(actual, expected);
    }
}

/// Pins exact serialized-byte boundaries, metadata reservation, and retention of later small records.
#[test]
fn jsonl_budget_boundaries_keep_records_atomic() {
    let root = Path::new("/repo");
    let mut analysis = small_result(root);
    analysis.findings[0].message = "界\n\"".to_owned();
    let corpus = CorpusCensus {
        definition_files: 1,
        definition_files_with_definitions: 1,
        definitions_extracted: 2,
        feature_files: 0,
        feature_files_parsed: 0,
        feature_files_without_steps: 0,
        incomplete: false,
    };
    let census = crate::analysis::AnalysisCensus::default();
    let metadata = CliReportMetadata {
        corpus: &corpus,
        analysis: &census,
        execution_successful: true,
    };
    let metrics = ExecutionMetrics::new(1, 0, 1.0, 2.0, 3.0);
    let context = ReportContext::with_metrics(&analysis, root, 100.0, &metrics);
    for metadata in [None, Some(&metadata)] {
        let mut complete = Vec::new();
        jsonl::write_jsonl_context(&context, metadata, &mut complete).unwrap();
        for (budget, expected) in [
            (complete.len() - 1, 0),
            (complete.len(), 1),
            (complete.len() + 1, 1),
        ] {
            let mut output = Vec::new();
            jsonl::write_jsonl_with_limits(&context, metadata, &mut output, 10, budget).unwrap();
            assert!(output.len() <= budget);
            let records: Vec<serde_json::Value> = output
                .split(|b| *b == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_slice(line).unwrap())
                .collect();
            assert_eq!(records.len(), expected + 1);
            let summary = records.last().unwrap();
            assert_eq!(summary["findingsTruncated"], 1 - expected);
            assert_eq!(summary["truncated"], expected == 0);
            assert_eq!(
                summary["summary"],
                serde_json::to_value(&context.summary).unwrap()
            );
            assert!(summary.get("metrics").is_some());
            assert_eq!(summary.get("corpus").is_some(), metadata.is_some());
            assert_eq!(summary.get("analysis").is_some(), metadata.is_some());
            if let Some(metadata) = metadata {
                assert_eq!(
                    summary["corpus"],
                    serde_json::to_value(metadata.corpus).unwrap()
                );
                assert_eq!(
                    summary["analysis"],
                    serde_json::to_value(metadata.analysis).unwrap()
                );
            }
        }
        let mut output = Vec::new();
        assert!(jsonl::write_jsonl_with_limits(&context, metadata, &mut output, 10, 1).is_err());
        assert!(output.is_empty());
    }
    let small_size = render_jsonl(&context).unwrap().len();
    let mut large = analysis.findings[0].clone();
    large.message = "large".repeat(400);
    analysis.findings.insert(0, large);
    let context = ReportContext::new(&analysis, root, 100.0);
    let mut output = Vec::new();
    jsonl::write_jsonl_with_limits(&context, None, &mut output, 10, small_size).unwrap();
    let records: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["message"], "界\n\"");
    assert_eq!(records[1]["findingsTruncated"], 1);
}

/// Preserves inactive records and error propagation through both write and flush failures.
#[test]
fn jsonl_keeps_inactive_records_and_propagates_io_failures() {
    let root = Path::new("/repo");
    let mut analysis = small_result(root);
    let mut suppressed = analysis.findings[0].clone();
    suppressed.suppression = Some(Suppression {
        reason: "intentional".to_owned(),
    });
    let mut off = analysis.findings[0].clone();
    off.severity = Severity::Off;
    analysis.findings.extend([suppressed, off]);
    let context = ReportContext::new(&analysis, root, 0.0);
    let records: Vec<serde_json::Value> = render_jsonl(&context)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.len(), 4);
    assert_eq!(
        records
            .iter()
            .take(3)
            .map(|r| r["active"].as_bool().unwrap())
            .collect::<Vec<_>>(),
        [true, false, false]
    );
    assert_eq!(records[3]["findingsTruncated"], 0);
    for flush_failure in [false, true] {
        let mut writer = FailingOutput {
            flush_failure,
            ..Default::default()
        };
        let error = jsonl::write_jsonl_context(&context, None, &mut writer).unwrap_err();
        assert_broken_pipe(error);
    }
    analysis.findings = vec![analysis.findings[0].clone(); 20];
    let context = ReportContext::new(&analysis, root, 0.0);
    assert!(render_jsonl(&context).unwrap().len() > 8 * 1024);
    let mut writer = FailingOutput::default();
    assert!(jsonl::write_jsonl_context(&context, None, &mut writer).is_err());
    assert!(
        !writer.flushed,
        "a failed buffer write must return before flushing"
    );
}

/// Rejects either data writes or the final flush to exercise buffered error paths.
#[derive(Default)]
struct FailingOutput {
    flush_failure: bool,
    flushed: bool,
    writes: usize,
}

impl std::io::Write for FailingOutput {
    /// Fails the first write only, so later success cannot mask a swallowed write error.
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        if self.flush_failure || self.writes > 1 {
            Ok(bytes.len())
        } else {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }

    /// Records flush attempts and rejects them only when the test targets that failure.
    fn flush(&mut self) -> std::io::Result<()> {
        self.flushed = true;
        if self.flush_failure {
            Err(std::io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }
}

/// Distinguishes bounded-buffer omissions from genuine serialization failures.
#[test]
fn jsonl_record_capacity_does_not_hide_serialization_errors() {
    assert_eq!(
        jsonl::fitting_jsonl_record(&true, 5).unwrap().unwrap(),
        b"true\n"
    );
    assert!(jsonl::fitting_jsonl_record(&true, 4).unwrap().is_none());
    let unsupported = std::collections::BTreeMap::from([(vec![1, 2], true)]);
    let error = jsonl::fitting_jsonl_record(&unsupported, 1_024).unwrap_err();
    assert!(!error.is_io());
    assert!(error.to_string().contains("key must be a string"));
}

/// Propagates a write error triggered specifically when the summary crosses the buffer boundary.
#[test]
fn jsonl_summary_write_failure_is_not_swallowed() {
    let root = Path::new("/repo");
    let mut analysis = small_result(root);
    analysis.findings[0].message.clear();
    let initial = render_jsonl(&ReportContext::new(&analysis, root, 0.0)).unwrap();
    let finding_bytes = initial.lines().next().unwrap().len() + 1;
    let summary_bytes = initial.lines().last().unwrap().len() + 1;
    analysis.findings =
        vec![analysis.findings[0].clone(); (8 * 1024 - summary_bytes) / finding_bytes];
    let initial = render_jsonl(&ReportContext::new(&analysis, root, 0.0)).unwrap();
    let padding = (8 * 1024 + 1) - initial.len();
    assert!(padding <= JSONL_TEXT_LIMIT_CHARS);
    analysis.findings[0].message = "x".repeat(padding);
    let context = ReportContext::new(&analysis, root, 0.0);
    let complete = render_jsonl(&context).unwrap();
    assert_eq!(complete.len(), 8 * 1024 + 1);
    let finding_bytes = complete.len() - complete.lines().last().unwrap().len() - 1;
    assert!(finding_bytes < 8 * 1024);
    let mut writer = FailingOutput::default();
    let error = jsonl::write_jsonl_context(&context, None, &mut writer).unwrap_err();
    assert_broken_pipe(error);
    assert!(!writer.flushed);
}

/// Checks the preserved I/O cause through the reporter's contextual error chain.
fn assert_broken_pipe(error: anyhow::Error) {
    assert!(error.chain().any(|cause| cause
        .downcast_ref::<std::io::Error>()
        .is_some_and(|cause| cause.kind() == std::io::ErrorKind::BrokenPipe)));
}
