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
