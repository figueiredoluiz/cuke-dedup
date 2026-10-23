//! The words that mark a step handler as a pending stub, shared by every source language.
//!
//! An adapter recognizes the syntax that carries a marker (a call, an identifier, a string, a
//! raised error) and hands the text to [`is_stub_label`]. The core decides which text counts, so
//! two languages cannot disagree on the vocabulary.

/// Pending markers after [`stub_label_key`] folding: `pending`, `not_implemented`, `TODO`,
/// `"Not implemented"` and every other spelling of them reduce to one of these.
const STUB_LABELS: [&str; 3] = ["pending", "notimplemented", "todo"];

/// Drops every non-alphanumeric character, including quotes, and lowercases the rest.
fn stub_label_key(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether `text` names a pending marker once punctuation, quoting and case are dropped.
pub(crate) fn is_stub_label(text: &str) -> bool {
    STUB_LABELS.contains(&stub_label_key(text).as_str())
}

#[cfg(test)]
mod tests {
    use super::is_stub_label;

    /// Every spelling that folds to a marker counts; near misses and other words do not, so
    /// neither widening the folding nor narrowing the vocabulary passes unnoticed.
    #[test]
    fn labels_fold_punctuation_quotes_and_case() {
        for label in [
            "pending",
            "Pending",
            "PENDING",
            "not_implemented",
            "NotImplemented",
            "not implemented",
            "'Not Implemented'",
            "\"todo\"",
            "`TODO`",
            "  to-do  ",
            "to.do!",
        ] {
            assert!(is_stub_label(label), "{label:?}");
        }
        for other in [
            "",
            "'",
            "done",
            "pend",
            "pending_review",
            "implemented",
            "notimplementedyet",
            "todos",
            "p3nding",
        ] {
            assert!(!is_stub_label(other), "{other:?}");
        }
    }
}
