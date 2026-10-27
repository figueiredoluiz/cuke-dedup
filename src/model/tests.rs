use super::*;

/// Builds a synthetic non-comparable step definition at the requested source line.
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
            effect_stream: Default::default(),
            capture_scope: None,
        },
        framework: Framework::UNKNOWN,
        analysis_policy: Default::default(),
        registration: "Given".to_owned(),
        location: SourceLocation::new("steps.ts", line, 1, line, 10),
        inline_suppressions: Vec::new(),
    }
}

/// Builds a duplicate-matcher finding between two synthetic definitions with the requested severity.
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

/// Checks each public rule name round-trips and identifies its matching documentation anchor.
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

/// Checks multipart fingerprints distinguish embedded separators, part boundaries, and the single-string domain.
#[test]
fn multipart_fingerprint_has_unambiguous_framing_and_a_separate_domain() {
    assert_ne!(
        stable_fingerprint_parts(["a\0b"]),
        stable_fingerprint_parts(["a", "b"])
    );
    assert_ne!(stable_fingerprint("a"), stable_fingerprint_parts(["a"]));
    assert_ne!(stable_fingerprint_parts([]), stable_fingerprint_parts([""]));
}

/// Checks severity display, accepted aliases, and the diagnostic for an unknown severity.
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

/// Checks duplication thresholds include duplication rules but exclude ambiguity and unused-definition findings.
#[test]
fn threshold_rules_exclude_correctness_and_usage_findings() {
    assert!(Rule::DuplicateMatcher.contributes_to_duplication_threshold());
    assert!(Rule::ParameterizationCandidate.contributes_to_duplication_threshold());
    assert!(!Rule::AmbiguousStep.contributes_to_duplication_threshold());
    assert!(!Rule::UnusedDefinition.contributes_to_duplication_threshold());
}

/// Checks thresholds count distinct definitions from active errors while excluding warnings and suppressions.
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

/// Checks an empty result passes a zero threshold with zero duplication.
#[test]
fn empty_analysis_has_zero_percent_duplication() {
    let result = AnalysisResult::default();
    let threshold = DuplicationThreshold::from_result(&result, 0.0);
    assert_eq!(threshold.percentage, 0.0);
    assert!(threshold.passed);
}

/// Checks separate definitions sharing a location are both counted toward duplication.
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

/// Checks the stable fingerprint value, hexadecimal shape, and case sensitivity.
#[test]
fn fingerprint_is_stable_and_content_sensitive() {
    assert_eq!(
        stable_fingerprint("hello"),
        "d45a69d7e7ff4c802cfc49b4f1fa7d11"
    );
    assert_eq!(stable_fingerprint("hello").len(), 32);
    assert!(stable_fingerprint("hello")
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
    assert_ne!(stable_fingerprint("hello"), stable_fingerprint("Hello"));
}

/// Checks displayed source locations normalize Windows separators to forward slashes.
#[test]
fn displayed_locations_use_portable_path_separators() {
    let location = SourceLocation::new("steps\\checkout.ts", 3, 4, 3, 10);
    assert_eq!(location.display(Path::new(".")), "steps/checkout.ts:3:4");
}

/// Builds semantics with the given encoding and events; the exact text is non-ASCII.
fn semantics(encoding: RepresentationEncoding, events: Vec<BehaviorEvent>) -> HandlerSemantics {
    HandlerSemantics {
        exact: "exact \u{e9}".to_owned(),
        normalized: "normalized".to_owned(),
        alpha: "alpha".to_owned(),
        structural: Some("structural".to_owned()),
        events,
        source_snippet: "snippet".to_owned(),
        comparable: true,
        trivial: false,
        encoding,
        stream: EffectStream::default(),
        capture_scope: None,
    }
}

/// `Digest` replaces every representation with its stable digest; equal text maps to equal
/// digests and different text to different ones.
#[test]
fn digest_encoding_hashes_every_representation() {
    let finished = semantics(RepresentationEncoding::Digest, Vec::new()).finish();
    assert_eq!(finished.exact, stable_fingerprint("exact \u{e9}"));
    assert_eq!(finished.normalized, stable_fingerprint("normalized"));
    assert_eq!(finished.alpha_normalized, stable_fingerprint("alpha"));
    assert_eq!(finished.structural, stable_fingerprint("structural"));
    assert_eq!(finished.source_snippet, "snippet");
    let mut other = semantics(RepresentationEncoding::Digest, Vec::new());
    other.alpha = "alpha2".to_owned();
    let other = other.finish();
    assert_eq!(finished.exact, other.exact);
    assert_ne!(finished.alpha_normalized, other.alpha_normalized);
}

/// `Verbatim` keeps text for normalized, alpha and structural and pads the exact digest to the
/// requested length: zero, below, at and above one digest.
#[test]
fn verbatim_encoding_keeps_text_and_pads_the_exact_digest() {
    let digest = stable_fingerprint("exact \u{e9}");
    for (exact_len, expected) in [
        (0, String::new()),
        (5, digest[..5].to_owned()),
        (32, digest.clone()),
        (40, format!("{digest}{}", &digest[..8])),
    ] {
        let finished =
            semantics(RepresentationEncoding::Verbatim { exact_len }, Vec::new()).finish();
        assert_eq!(finished.exact, expected);
        assert_eq!(finished.normalized, "normalized");
        assert_eq!(finished.alpha_normalized, "alpha");
        assert_eq!(finished.structural, "structural");
    }
}

/// An absent structural representation reuses the encoded exact identity under both encodings;
/// a present one is encoded like any other representation, even when it equals the exact text.
#[test]
fn missing_structural_reuses_the_encoded_exact_identity() {
    for encoding in [
        RepresentationEncoding::Digest,
        RepresentationEncoding::Verbatim { exact_len: 12 },
    ] {
        let mut missing = semantics(encoding, Vec::new());
        missing.structural = None;
        let missing = missing.finish();
        assert_eq!(missing.structural, missing.exact);
    }
    let mut same_text = semantics(
        RepresentationEncoding::Verbatim { exact_len: 12 },
        Vec::new(),
    );
    same_text.structural = Some("exact \u{e9}".to_owned());
    assert_eq!(same_text.finish().structural, "exact \u{e9}");
}

/// Events encode as legacy signatures in order; an unresolved assertion withdraws comparability
/// under both encodings, a resolved one does not, and an incomparable input stays incomparable.
#[test]
fn finish_encodes_events_and_withdraws_comparability_on_unresolved_assertions() {
    let resolved = vec![
        BehaviorEvent::Method("async".to_owned()),
        BehaviorEvent::Call("work".to_owned()),
        BehaviorEvent::assertion("q", "m", "s", "e").deferred(),
        BehaviorEvent::ControlFlow(ControlFlowOperation::If),
    ];
    for encoding in [
        RepresentationEncoding::Digest,
        RepresentationEncoding::Verbatim { exact_len: 3 },
    ] {
        let finished = semantics(encoding, resolved.clone()).finish();
        assert!(finished.comparable);
        assert_eq!(finished.behavior_signature.len(), 4);
        assert_eq!(finished.behavior_signature[0], "method:async");
        assert_eq!(finished.behavior_signature[1], "call:work");
        assert!(finished.behavior_signature[2].starts_with("deferred-assert:q#m:"));
        assert_eq!(finished.behavior_signature[3], "if_statement");
        let mut unresolved = resolved.clone();
        unresolved.push(BehaviorEvent::unresolved_assertion(true));
        assert!(!semantics(encoding, unresolved).finish().comparable);
        let mut input = semantics(encoding, Vec::new());
        input.comparable = false;
        assert!(!input.finish().comparable);
    }
}

/// `finish` carries the effect stream and capture scope through unchanged: the default is partial
/// and unrestricted, and every other value survives under both encodings.
#[test]
fn finish_carries_effect_stream_and_capture_scope() {
    let default = semantics(RepresentationEncoding::Digest, Vec::new()).finish();
    assert_eq!(default.effect_stream, EffectStream::Partial);
    assert_eq!(default.capture_scope, None);
    for encoding in [
        RepresentationEncoding::Digest,
        RepresentationEncoding::Verbatim { exact_len: 3 },
    ] {
        for (stream, scope) in [
            (EffectStream::Complete, Some("")),
            (EffectStream::ActionOnly, Some("4:9")),
        ] {
            let mut input = semantics(encoding, Vec::new());
            input.stream = stream;
            input.capture_scope = scope.map(str::to_owned);
            let finished = input.finish();
            assert_eq!(finished.effect_stream, stream);
            assert_eq!(finished.capture_scope.as_deref(), scope);
        }
    }
}

/// Default-valued typed fields are omitted from serialized handlers and absent fields
/// deserialize to the defaults; other values round-trip.
#[test]
fn typed_handler_fields_serialize_only_when_set() {
    let mut handler = semantics(RepresentationEncoding::Digest, Vec::new()).finish();
    let plain = serde_json::to_value(&handler).unwrap();
    assert!(plain.get("effect_stream").is_none() && plain.get("capture_scope").is_none());
    assert_eq!(
        serde_json::from_value::<HandlerFingerprint>(plain).unwrap(),
        handler
    );
    handler.effect_stream = EffectStream::ActionOnly;
    handler.capture_scope = Some("4:9".to_owned());
    let tagged = serde_json::to_value(&handler).unwrap();
    assert_eq!(tagged["effect_stream"], "action-only");
    assert_eq!(
        serde_json::from_value::<HandlerFingerprint>(tagged).unwrap(),
        handler
    );
}

/// Handlers have one behaviour identity only when events, effect stream and capture scope all
/// agree; events alone do not separate handlers that differ in the other two.
#[test]
fn behavior_identity_separates_what_events_cannot() {
    let handler = |events: &[&str], stream, scope: Option<&str>| {
        let mut handler = definition(1).handler;
        handler.behavior_signature = events.iter().map(|event| (*event).to_owned()).collect();
        handler.effect_stream = stream;
        handler.capture_scope = scope.map(str::to_owned);
        handler
    };
    let base = handler(&["call:work"], EffectStream::ActionOnly, Some("4:9"));
    for (name, other, same) in [
        (
            "identical",
            handler(&["call:work"], EffectStream::ActionOnly, Some("4:9")),
            true,
        ),
        (
            "other events",
            handler(&["call:rest"], EffectStream::ActionOnly, Some("4:9")),
            false,
        ),
        (
            "other stream",
            handler(&["call:work"], EffectStream::Complete, Some("4:9")),
            false,
        ),
        (
            "other scope key",
            handler(&["call:work"], EffectStream::ActionOnly, Some("7:12")),
            false,
        ),
        (
            "bare scope",
            handler(&["call:work"], EffectStream::ActionOnly, Some("")),
            false,
        ),
        (
            "no scope",
            handler(&["call:work"], EffectStream::ActionOnly, None),
            false,
        ),
    ] {
        assert_eq!(
            base.behavior_identity() == other.behavior_identity(),
            same,
            "{name}"
        );
        assert_eq!(
            base.behavior_digest() == other.behavior_digest(),
            same,
            "{name}: digest"
        );
    }
    // The digest spells each regime as the serialized name.
    for stream in [
        EffectStream::Partial,
        EffectStream::Complete,
        EffectStream::ActionOnly,
    ] {
        assert_eq!(serde_json::to_value(stream).unwrap(), stream.name());
    }
    // The bare scope (empty key) and no scope are different identities.
    let unscoped = handler(&["call:work"], EffectStream::ActionOnly, None);
    let bare = handler(&["call:work"], EffectStream::ActionOnly, Some(""));
    assert_ne!(unscoped.behavior_identity(), bare.behavior_identity());
    assert_ne!(unscoped.behavior_digest(), bare.behavior_digest());
    // A handler with neither fact digests as its events alone.
    let plain = handler(&["call:work"], EffectStream::Partial, None);
    assert_eq!(
        plain.behavior_digest(),
        stable_fingerprint_parts(["call:work"])
    );
}

/// Every framework a registered adapter declares round-trips through its id, is declared by one
/// adapter type only and never reuses the reserved `unknown`; the former closed enum's ids stay
/// declared; `unknown` maps to [`Framework::UNKNOWN`] and any undeclared id is rejected with the
/// declared ids listed.
#[test]
fn framework_ids_keep_their_serialized_form_and_reject_undeclared_ids() {
    use crate::source_adapter::SOURCE_ADAPTER_REGISTRY;
    // An adapter type's registrations share one declaration list (the JavaScript, TypeScript and
    // TSX adapters return the same named static); each distinct list, keyed by address, counts
    // once, so an id repeated within a list or across two lists survives as a duplicate.
    let mut lists: Vec<&'static [crate::source_adapter::FrameworkContract]> =
        SOURCE_ADAPTER_REGISTRY
            .iter()
            .map(|registration| registration.adapter.framework_contracts())
            .collect();
    lists.sort_unstable_by_key(|list| list.as_ptr() as usize);
    lists.dedup_by_key(|list| list.as_ptr() as usize);
    let mut declared: Vec<&'static str> = lists
        .iter()
        .flat_map(|list| list.iter().map(|contract| contract.framework.id()))
        .collect();
    declared.sort_unstable();
    let ids = declared;
    assert!(
        ids.windows(2).all(|pair| pair[0] != pair[1]),
        "each id is declared once, by one adapter type: {ids:?}"
    );
    // A declared `unknown` would deserialize as `Framework::UNKNOWN` and lose its adapter.
    assert!(!ids.contains(&Framework::UNKNOWN.id()), "{ids:?}");
    // The kebab-case strings of the former enum variants; reports and fingerprints keep them.
    for legacy in [
        "cucumber-js",
        "cucumber-ruby",
        "cypress-cucumber",
        "jest-cucumber",
        "playwright-bdd",
        "vitest-cucumber",
    ] {
        assert!(
            ids.contains(&legacy),
            "{legacy} is no longer declared: {ids:?}"
        );
    }
    for id in ids.iter().copied().chain(["unknown"]) {
        let framework: Framework = serde_json::from_value(serde_json::json!(id)).unwrap();
        assert_eq!(framework, Framework::new(id));
        assert_eq!(
            serde_json::to_value(framework).unwrap(),
            serde_json::json!(id)
        );
    }
    assert_eq!(
        serde_json::from_value::<Framework>(serde_json::json!("unknown")).unwrap(),
        Framework::UNKNOWN
    );
    for undeclared in ["cucumber", "Cucumber-JS", "", "jest"] {
        let error = serde_json::from_value::<Framework>(serde_json::json!(undeclared))
            .unwrap_err()
            .to_string();
        assert!(
            error.starts_with(&format!(
                "unknown framework `{undeclared}`, expected one of "
            )),
            "{error}"
        );
        assert!(
            error.contains("cucumber-js") && error.contains("unknown"),
            "{error}"
        );
    }
}
