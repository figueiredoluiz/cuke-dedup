use super::{BehaviorEvent, BehaviorEventRef, ControlFlowOperation};
use crate::model::HandlerFingerprint;

/// Parsing then re-encoding returns each signature unchanged, including unknown and empty ones.
#[test]
fn legacy_signatures_round_trip_known_and_unknown_events() {
    let signatures = [
        "method:instance sync",
        "call:v0#click",
        "call:(subscript_expression:dynamic-key)",
        "assert:expect.not#toBe:subject:expected",
        "assert:unresolved",
        "deferred-assert:expect#toEqual:subject:expected",
        "deferred-assert:unresolved",
        "if_statement",
        "throw_statement",
        "deferred-call:opaque",
        "custom:event:with delimiters # and \0",
        "",
    ];

    for signature in signatures {
        assert_eq!(
            BehaviorEventRef::from_legacy(signature).into_legacy(),
            signature,
            "{signature:?}"
        );
    }
}

/// Prefixed signatures classify as method, call, assertion or control flow, with deferral
/// read from the prefix; `method` without a colon is not a method, and an unknown statement
/// kind stays `Legacy`.
#[test]
fn typed_classification_keeps_method_execution_and_assertion_anchors_distinct() {
    assert_eq!(
        BehaviorEventRef::from_legacy("method:async").method_semantics(),
        Some("async")
    );
    assert!(BehaviorEventRef::from_legacy("method:").is_method());
    assert!(!BehaviorEventRef::from_legacy("method").is_method());
    assert!(BehaviorEventRef::from_legacy("deferred-assert:unresolved").is_deferred_assertion());
    assert!(!BehaviorEventRef::from_legacy("assert:unresolved").is_deferred_assertion());
    assert_eq!(
        BehaviorEventRef::from_legacy("call:page#click"),
        BehaviorEventRef::Call("page#click")
    );
    assert_eq!(
        BehaviorEventRef::from_legacy("assert:expect#toBe:subject:ready"),
        BehaviorEventRef::Assertion {
            deferred: false,
            payload: "expect#toBe:subject:ready"
        }
    );
    assert_eq!(
        BehaviorEventRef::from_legacy("if_statement"),
        BehaviorEventRef::ControlFlow(ControlFlowOperation::If)
    );
    assert_eq!(
        BehaviorEventRef::from_legacy("if-statements"),
        BehaviorEventRef::Legacy("if-statements")
    );
}

/// Deferring an assertion changes only its prefix, and `unresolved_assertion` is detected as
/// unresolved whether deferred or not.
#[test]
fn deferring_and_unresolved_markers_preserve_assertion_payloads() {
    for signature in [
        "assert:expect#toBe:subject:expected",
        "assert:unresolved",
        "assert:",
    ] {
        let BehaviorEventRef::Assertion { payload, .. } = BehaviorEventRef::from_legacy(signature)
        else {
            unreachable!("test signatures are assertions");
        };
        let event = BehaviorEvent::Assertion {
            deferred: false,
            payload: payload.to_owned(),
        }
        .deferred();
        assert_eq!(
            event.into_legacy(),
            signature.replacen("assert:", "deferred-assert:", 1)
        );
    }
    assert!(BehaviorEvent::unresolved_assertion(false).is_unresolved_assertion());
    assert!(BehaviorEvent::unresolved_assertion(true).is_unresolved_assertion());
}

/// `HandlerFingerprint` serializes `behavior_signature` as a JSON string array with fixed
/// field names and order, and deserializes back to an equal value.
#[test]
fn handler_fingerprint_keeps_the_public_string_array_shape() {
    let fingerprint = HandlerFingerprint {
        exact: String::new(),
        normalized: String::new(),
        alpha_normalized: String::new(),
        structural: String::new(),
        behavior_signature: vec![
            "method:instance sync".to_owned(),
            "assert:expect.not#toBe:subject:ready".to_owned(),
        ],
        source_snippet: String::new(),
        comparable: true,
        trivial: false,
        effect_stream: Default::default(),
        capture_scope: None,
    };
    let serialized = serde_json::to_string(&fingerprint).unwrap();
    assert_eq!(
        serialized,
        r#"{"exact":"","normalized":"","alpha_normalized":"","structural":"","behavior_signature":["method:instance sync","assert:expect.not#toBe:subject:ready"],"source_snippet":"","comparable":true,"trivial":false}"#
    );
    assert_eq!(
        serde_json::from_str::<HandlerFingerprint>(&serialized).unwrap(),
        fingerprint
    );
}
