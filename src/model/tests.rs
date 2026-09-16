use super::*;

fn definition(line: usize) -> StepDefinition {
    StepDefinition {
        matcher: format!("step {line}"),
        normalized_matcher: format!("step {line}"),
        matcher_kind: MatcherKind::CucumberExpression,
        matcher_flags: String::new(),
        handler: HandlerFingerprint {
            exact: String::new(),
            normalized: String::new(),
            alpha_normalized: String::new(),
            structural: String::new(),
            behavior_signature: Vec::new(),
            source_snippet: String::new(),
            comparable: false,
            trivial: true,
        },
        framework: Framework::Unknown,
        registration: "Given".to_owned(),
        location: SourceLocation::new("steps.ts", line, 1, line, 10),
        inline_suppressions: Vec::new(),
    }
}

fn finding(
    definitions: &[StepDefinition],
    left: usize,
    right: usize,
    severity: Severity,
) -> Finding {
    Finding {
        rule: Rule::DuplicateMatcher,
        severity,
        message: "duplicate".to_owned(),
        primary: definitions[left].location.clone(),
        related: vec![definitions[right].location.clone()],
        evidence: FindingEvidence {
            matcher_similarity: Some(1.0),
            handler_similarity: None,
            matcher_difference: String::new(),
            handler_evidence: String::new(),
            comparison: None,
            cluster: None,
        },
        suggested_action: "consolidate".to_owned(),
        suppression: None,
    }
}

#[test]
fn rule_names_are_stable_kebab_case() {
    for &rule in Rule::ALL {
        assert_eq!(rule.to_string().parse::<Rule>(), Ok(rule));
        assert_eq!(
            rule.documentation_url(),
            format!(
                "https://github.com/figueiredoluiz/cuke-dedup/blob/main/docs/rules.md#{}",
                rule
            )
        );
    }
}

#[test]
fn multipart_fingerprint_matches_nul_joined_input_without_join_allocation() {
    assert_eq!(
        stable_fingerprint_parts(["duplicate-matcher", "alpha", "bravo"]),
        stable_fingerprint("duplicate-matcher\0alpha\0bravo")
    );
}

#[test]
fn severity_text_forms_are_complete_and_reject_unknown_values() {
    assert_eq!(Severity::Off.to_string(), "off");
    assert_eq!(Severity::Warning.to_string(), "warning");
    assert_eq!(Severity::Error.to_string(), "error");
    assert_eq!("off".parse(), Ok(Severity::Off));
    assert_eq!("warn".parse(), Ok(Severity::Warning));
    assert_eq!("warning".parse(), Ok(Severity::Warning));
    assert_eq!("error".parse(), Ok(Severity::Error));
    assert_eq!(
        "fatal".parse::<Severity>(),
        Err("invalid severity `fatal` (expected off, warning, or error)".to_owned())
    );
}

#[test]
fn threshold_rules_exclude_correctness_and_usage_findings() {
    assert!(Rule::DuplicateMatcher.contributes_to_duplication_threshold());
    assert!(Rule::ParameterizationCandidate.contributes_to_duplication_threshold());
    assert!(!Rule::AmbiguousStep.contributes_to_duplication_threshold());
    assert!(!Rule::UnusedDefinition.contributes_to_duplication_threshold());
}

#[test]
fn duplication_threshold_counts_unique_active_error_definitions() {
    let definitions: Vec<_> = (1..=4).map(definition).collect();
    let first = finding(&definitions, 0, 1, Severity::Error);
    let second = finding(&definitions, 0, 2, Severity::Error);
    let warning = finding(&definitions, 2, 3, Severity::Warning);
    let mut suppressed = finding(&definitions, 1, 3, Severity::Error);
    suppressed.suppression = Some(Suppression {
        reason: "accepted".to_owned(),
    });
    let result = AnalysisResult {
        definitions,
        feature_steps: Vec::new(),
        findings: vec![first, second, warning, suppressed],
    };

    let at_limit = DuplicationThreshold::from_result(&result, 75.0);
    assert_eq!(at_limit.duplicated_definitions, 3);
    assert_eq!(at_limit.total_definitions, 4);
    assert_eq!(at_limit.percentage, 75.0);
    assert_eq!(at_limit.rules, [Rule::DuplicateMatcher]);
    assert!(at_limit.passed);
    assert!(!DuplicationThreshold::from_result(&result, 74.99).passed);
}

#[test]
fn empty_analysis_has_zero_percent_duplication() {
    let result = AnalysisResult::default();
    let threshold = DuplicationThreshold::from_result(&result, 0.0);
    assert_eq!(threshold.percentage, 0.0);
    assert!(threshold.passed);
}

#[test]
fn duplication_threshold_counts_multiple_definitions_at_one_location() {
    let mut definitions = vec![definition(1), definition(2)];
    definitions[1].location = definitions[0].location.clone();
    let duplicate = finding(&definitions, 0, 1, Severity::Error);
    let result = AnalysisResult {
        definitions,
        feature_steps: Vec::new(),
        findings: vec![duplicate],
    };
    let threshold = DuplicationThreshold::from_result(&result, 100.0);
    assert_eq!(threshold.duplicated_definitions, 2);
    assert_eq!(threshold.percentage, 100.0);
}

#[test]
fn fingerprint_is_stable_and_content_sensitive() {
    assert_eq!(stable_fingerprint("hello"), "a430d84680aabd0b");
    assert_ne!(stable_fingerprint("hello"), stable_fingerprint("Hello"));
}

#[test]
fn displayed_locations_use_portable_path_separators() {
    let location = SourceLocation::new("steps\\checkout.ts", 3, 4, 3, 10);
    assert_eq!(location.display(Path::new(".")), "steps/checkout.ts:3:4");
}
