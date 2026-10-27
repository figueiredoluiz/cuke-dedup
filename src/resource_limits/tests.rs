use super::*;
use std::fs;

/// Reads a file of exactly `limit` bytes and rejects one byte more as an input-limit error.
#[test]
fn bounded_reader_accepts_exact_limit_and_rejects_next_byte() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("input.ts");
    fs::write(&path, "1234").unwrap();
    assert_eq!(read_utf8(&path, "source", 4).unwrap(), "1234");

    fs::write(&path, "12345").unwrap();
    let error = read_utf8(&path, "source", 4).unwrap_err();
    assert!(is_input_limit_error(&error));
    assert!(error.to_string().contains("4-byte input limit"));
}

/// Invalid UTF-8 under the limit fails with a UTF-8 error not classified as an input limit.
#[test]
fn bounded_reader_rejects_invalid_utf8_without_misclassifying_it_as_a_limit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("input.ts");
    fs::write(&path, [0xff]).unwrap();

    let error = read_utf8(&path, "source", 4).unwrap_err();
    assert!(!is_input_limit_error(&error));
    assert!(error.to_string().contains("not valid UTF-8"));
}

/// Accepts metadata at the exact boundary and reuses the standard diagnostic above it.
#[test]
fn known_input_limit_only_rejects_metadata_above_the_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("dependency.rb");
    fs::write(&path, "1234").unwrap();
    assert_eq!(known_input_limit_exceeded(&path, "source", 4), None);

    fs::write(&path, "12345").unwrap();
    let diagnostic = known_input_limit_exceeded(&path, "source", 4).unwrap();
    assert!(diagnostic.contains("dependency.rb"));
    assert!(diagnostic.contains("4-byte input limit"));
}

/// `/dev/zero`, which metadata does not size, is cut off at the limit by the bounded read.
#[cfg(unix)]
#[test]
fn bounded_reader_limits_special_files_without_reliable_metadata_lengths() {
    let error = read_utf8(Path::new("/dev/zero"), "special input", 4).unwrap_err();
    assert!(is_input_limit_error(&error));
    assert!(error.to_string().contains("4-byte input limit"));
}

/// A 1,000-way alternation exceeds a 128-byte program limit with `CompiledTooBig(128)`.
#[test]
fn regex_builder_enforces_the_configured_program_limit() {
    let pattern = (0..1_000)
        .map(|index| format!("literal-{index}"))
        .collect::<Vec<_>>()
        .join("|");
    let error = compile_regex_with_limits(&pattern, 128, 128).unwrap_err();
    assert!(matches!(error, regex::Error::CompiledTooBig(128)));
}

/// `charge` takes every amount, saturating, and reports exceeded only once work passes the
/// limit; reaching the limit exactly is not exhaustion, and exhaustion persists.
#[test]
fn work_budget_charge_exceeds_only_past_the_limit_and_saturates() {
    for (limit, charges, expected) in [
        (0, &[0][..], &[false][..]),
        (0, &[1, 0], &[true, true]),
        (2, &[1, 1], &[false, false]),
        (2, &[2, 1, 0], &[false, true, true]),
        (usize::MAX, &[usize::MAX, usize::MAX], &[false, false]),
        (usize::MAX - 1, &[usize::MAX, 1], &[true, true]),
    ] {
        let mut budget = WorkBudget::new(limit);
        let answers: Vec<_> = charges
            .iter()
            .map(|amount| budget.charge(*amount))
            .collect();
        assert_eq!(answers, expected, "{limit} {charges:?}");
        let total = charges
            .iter()
            .fold(0usize, |sum, amount| sum.saturating_add(*amount));
        assert_eq!(
            budget.spent(),
            total,
            "a charge past the limit is still taken"
        );
        assert_eq!(budget.limit(), limit);
    }
}

/// `admit` charges only amounts that keep the total within the limit, so one-unit admissions
/// stop at exactly `limit` units, like a `count >= limit` check before charging; `fits`
/// answers the same question without charging.
#[test]
fn work_budget_admit_charges_only_what_fits() {
    for limit in 0..4 {
        let mut budget = WorkBudget::new(limit);
        let admitted = (0..8).take_while(|_| budget.admit(1)).count();
        assert_eq!((admitted, budget.spent()), (limit, limit));
        assert!(!budget.admit(1), "a refused admission charges nothing");
        assert_eq!(budget.spent(), limit);
    }
    let mut bytes = WorkBudget::new(4);
    assert!(bytes.fits(4) && !bytes.fits(5) && !bytes.fits(usize::MAX));
    assert_eq!(bytes.spent(), 0, "`fits` charges nothing");
    assert!(bytes.admit(3));
    assert!(!bytes.admit(2));
    assert!(
        bytes.admit(1),
        "a smaller amount still fits after a refusal"
    );
    assert!(bytes.admit(0));
    assert!(!bytes.admit(usize::MAX));
    assert_eq!(bytes.spent(), 4);
}

/// Exhaustion from either total latches across passes, a new pass restarts only pass work,
/// and work within the limit never latches.
#[test]
fn pass_budget_latches_exhaustion_across_passes() {
    let mut budget = PassBudget::new(2);
    assert!(!budget.charge_pass(2));
    assert!(!budget.begin_pass());
    assert!(!budget.charge_pass(2), "pass work restarts with each pass");
    assert!(!budget.charge_run(2));
    assert_eq!(budget.spent(), (2, 2));
    assert!(budget.charge_pass(1));
    assert!(budget.begin_pass(), "pass exhaustion survives a new pass");
    assert_eq!(budget.spent(), (2, 0));
    assert!(budget.charge_pass(0) && budget.exhausted());

    let mut run = PassBudget::new(2);
    assert!(run.charge_run(3));
    assert!(run.begin_pass() && run.charge_pass(0));
    run.set_limit(usize::MAX);
    assert!(run.exhausted(), "a raised limit does not clear the latch");

    let mut lowered = PassBudget::new(4);
    assert!(!lowered.charge_run(2) && !lowered.charge_pass(2));
    lowered.set_limit(1);
    assert!(
        lowered.exhausted(),
        "a lowered limit latches work already over it"
    );

    let mut raised = PassBudget::new(0);
    raised.set_limit(3);
    assert!(!raised.charge_run(3) && !raised.charge_pass(3));
    assert_eq!(raised.spent(), (3, 3));

    // Totals saturate instead of wrapping, so work past `usize::MAX` stays exhausted.
    let mut saturated = PassBudget::new(usize::MAX - 1);
    assert!(saturated.charge_run(usize::MAX) && saturated.charge_run(1));
    assert!(saturated.charge_pass(usize::MAX) && saturated.charge_pass(usize::MAX));
    assert_eq!(saturated.spent(), (usize::MAX, usize::MAX));
    let mut at_limit = PassBudget::new(usize::MAX);
    assert!(!at_limit.charge_run(usize::MAX) && !at_limit.charge_pass(usize::MAX));
}

/// Pins each release limit constant except `MAX_ASSERTION_RESOLUTION_WORK` to its value.
#[test]
fn release_resource_limits_match_the_documented_policy() {
    assert_eq!(MAX_PROJECT_INPUT_BYTES, 8 * 1024 * 1024);
    assert_eq!(MAX_CONFIG_INPUT_BYTES, 1024 * 1024);
    assert_eq!(MAX_SUPPRESSION_REASON_CHARS, 512);
    assert_eq!(MAX_BASELINE_INPUT_BYTES, 8 * 1024 * 1024);
    assert_eq!(MAX_REGEX_PATTERN_BYTES, 1024 * 1024);
    assert_eq!(REGEX_SIZE_LIMIT_BYTES, 1024 * 1024);
    assert_eq!(REGEX_DFA_SIZE_LIMIT_BYTES, 2 * 1024 * 1024);
    assert_eq!(MAX_STATIC_OVERLAP_FINDINGS, 10_000);
    assert_eq!(MAX_REGISTRATION_MODULES, 1_024);
    assert_eq!(MAX_REGISTRATION_MODULE_BYTES, 64 * 1024 * 1024);
    assert_eq!(MAX_REGISTRATION_RESOLUTION_STATES, 16_384);
    assert_eq!(MAX_PROJECT_CONFIG_EXTENDS_DEPTH, 16);
    assert_eq!(MAX_PROJECT_METADATA_FILES, 1_024);
    assert_eq!(MAX_PROJECT_METADATA_BYTES, 16 * 1024 * 1024);
    assert_eq!(MAX_PROJECT_MODULE_MAPPINGS, 16_384);
    assert_eq!(MAX_WORKSPACE_SCAN_ENTRIES, 100_000);
    assert_eq!(MAX_GHERKIN_MARKDOWN_CANDIDATES, 10_000);
    assert_eq!(MAX_GHERKIN_MARKDOWN_PROBE_BYTES, 64 * 1024 * 1024);
    assert_eq!(MAX_GHERKIN_MARKDOWN_PROBES, 128);
    assert_eq!(GHERKIN_MARKDOWN_RESTORE_BATCH_SIZE, 32);
    assert_eq!(MAX_CANDIDATE_COMPARISONS, 2_000_000);
    assert_eq!(MAX_STRUCTURAL_CLASS_COMPARISONS, 250_000);
    assert_eq!(SOURCE_CLASSIFICATION_PREFIX_BYTES, 64 * 1024);
    assert_eq!(MINIFIED_MEAN_LINE_BYTES, 200);
}
