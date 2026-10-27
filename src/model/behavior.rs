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
    /// Maps a tree-sitter statement kind such as `if_statement` to its operation; `None` for any
    /// other string.
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

    /// Returns the tree-sitter statement kind used in legacy signatures; inverse of `from_legacy`.
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
    /// Builds a non-deferred assertion whose payload is `qualifier#matcher:` followed by the
    /// fingerprints of `subject` and `expected`, colon-separated.
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

    /// Builds an assertion with the `unresolved` payload, which makes the handler not comparable
    /// in `HandlerSemantics::finish`.
    pub(crate) fn unresolved_assertion(deferred: bool) -> Self {
        Self::Assertion {
            deferred,
            payload: "unresolved".to_owned(),
        }
    }

    /// Marks an assertion as deferred, keeping its payload; other events are returned unchanged.
    pub(crate) fn deferred(self) -> Self {
        match self {
            Self::Assertion { payload, .. } => Self::Assertion {
                deferred: true,
                payload,
            },
            other => other,
        }
    }

    /// Returns whether this is an assertion with the `unresolved` payload, deferred or not.
    pub(crate) fn is_unresolved_assertion(&self) -> bool {
        matches!(self, Self::Assertion { payload, .. } if payload == "unresolved")
    }

    /// Encodes the event as its legacy signature string: `method:`, `call:`, `assert:` or
    /// `deferred-assert:` prefixes, the bare statement kind, or the `Legacy` text verbatim.
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
    /// Classifies a legacy signature by prefix, then by statement kind; anything else, including
    /// the empty string, becomes `Legacy` with the input unchanged.
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

    /// Returns the payload of a `Method` event; `None` for other events.
    pub(crate) fn method_semantics(self) -> Option<&'a str> {
        match self {
            Self::Method(payload) => Some(payload),
            _ => None,
        }
    }

    /// Returns whether the event is a `Method` event, including one with an empty payload.
    pub(crate) fn is_method(self) -> bool {
        matches!(self, Self::Method(_))
    }

    /// Returns whether the event is an assertion marked deferred.
    pub(crate) fn is_deferred_assertion(self) -> bool {
        matches!(self, Self::Assertion { deferred: true, .. })
    }

    /// Encodes the borrowed event like `BehaviorEvent::into_legacy`; test-only.
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
mod tests;
