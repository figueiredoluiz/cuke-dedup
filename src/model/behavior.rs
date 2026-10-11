//! Typed behavior events with a lossless legacy-signature boundary.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BehaviorEvent {
    Method(String),
    Call(String),
    Assertion { deferred: bool, payload: String },
    ControlFlow(ControlFlowOperation),
    Legacy(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ControlFlowOperation {
    If,
    Switch,
    For,
    ForIn,
    While,
    Do,
    Return,
    Throw,
}

impl ControlFlowOperation {
    fn from_legacy(value: &str) -> Option<Self> {
        Some(match value {
            "if_statement" => Self::If,
            "switch_statement" => Self::Switch,
            "for_statement" => Self::For,
            "for_in_statement" => Self::ForIn,
            "while_statement" => Self::While,
            "do_statement" => Self::Do,
            "return_statement" => Self::Return,
            "throw_statement" => Self::Throw,
            _ => return None,
        })
    }

    fn as_legacy(self) -> &'static str {
        match self {
            Self::If => "if_statement",
            Self::Switch => "switch_statement",
            Self::For => "for_statement",
            Self::ForIn => "for_in_statement",
            Self::While => "while_statement",
            Self::Do => "do_statement",
            Self::Return => "return_statement",
            Self::Throw => "throw_statement",
        }
    }
}

impl BehaviorEvent {
    pub(crate) fn assertion(qualifier: &str, matcher: &str, subject: &str, expected: &str) -> Self {
        Self::Assertion {
            deferred: false,
            payload: format!(
                "{qualifier}#{matcher}:{}:{}",
                super::stable_fingerprint(subject),
                super::stable_fingerprint(expected)
            ),
        }
    }

    pub(crate) fn unresolved_assertion(deferred: bool) -> Self {
        Self::Assertion {
            deferred,
            payload: "unresolved".to_owned(),
        }
    }

    pub(crate) fn deferred(self) -> Self {
        match self {
            Self::Assertion { payload, .. } => Self::Assertion {
                deferred: true,
                payload,
            },
            other => other,
        }
    }

    pub(crate) fn is_unresolved_assertion(&self) -> bool {
        matches!(self, Self::Assertion { payload, .. } if payload == "unresolved")
    }

    pub(crate) fn into_legacy(self) -> String {
        match self {
            Self::Method(payload) => format!("method:{payload}"),
            Self::Call(payload) => format!("call:{payload}"),
            Self::Assertion { deferred, payload } => {
                format!(
                    "{}{payload}",
                    if deferred {
                        "deferred-assert:"
                    } else {
                        "assert:"
                    }
                )
            }
            Self::ControlFlow(operation) => operation.as_legacy().to_owned(),
            Self::Legacy(value) => value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BehaviorEventRef<'a> {
    Method(&'a str),
    Call(&'a str),
    Assertion { deferred: bool, payload: &'a str },
    ControlFlow(ControlFlowOperation),
    Legacy(&'a str),
}

impl<'a> BehaviorEventRef<'a> {
    pub(crate) fn from_legacy(value: &'a str) -> Self {
        if let Some(payload) = value.strip_prefix("method:") {
            return Self::Method(payload);
        }
        if let Some(payload) = value.strip_prefix("call:") {
            return Self::Call(payload);
        }
        if let Some(payload) = value.strip_prefix("deferred-assert:") {
            return Self::Assertion {
                deferred: true,
                payload,
            };
        }
        if let Some(payload) = value.strip_prefix("assert:") {
            return Self::Assertion {
                deferred: false,
                payload,
            };
        }
        ControlFlowOperation::from_legacy(value)
            .map(Self::ControlFlow)
            .unwrap_or(Self::Legacy(value))
    }

    pub(crate) fn method_semantics(self) -> Option<&'a str> {
        match self {
            Self::Method(payload) => Some(payload),
            _ => None,
        }
    }

    pub(crate) fn is_method(self) -> bool {
        matches!(self, Self::Method(_))
    }

    pub(crate) fn is_deferred_assertion(self) -> bool {
        matches!(self, Self::Assertion { deferred: true, .. })
    }

    #[cfg(test)]
    fn into_legacy(self) -> String {
        match self {
            Self::Method(payload) => format!("method:{payload}"),
            Self::Call(payload) => format!("call:{payload}"),
            Self::Assertion { deferred, payload } => format!(
                "{}{payload}",
                if deferred {
                    "deferred-assert:"
                } else {
                    "assert:"
                }
            ),
            Self::ControlFlow(operation) => operation.as_legacy().to_owned(),
            Self::Legacy(value) => value.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BehaviorEvent, BehaviorEventRef, ControlFlowOperation};
    use crate::model::HandlerFingerprint;

    #[test]
    fn legacy_signatures_round_trip_known_and_unknown_events() {
        let signatures = [
            "method:instance sync",
            "method:ruby:lexical-file",
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

    #[test]
    fn deferring_and_unresolved_markers_preserve_assertion_payloads() {
        for signature in [
            "assert:expect#toBe:subject:expected",
            "assert:unresolved",
            "assert:",
        ] {
            let BehaviorEventRef::Assertion { payload, .. } =
                BehaviorEventRef::from_legacy(signature)
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
}
