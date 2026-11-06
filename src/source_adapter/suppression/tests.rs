use super::InlineSuppressions;
use crate::model::Rule;
use crate::source_adapter::SourceFile;

/// Every registration on a row gets that row's suppressions and its own copy of the row's directive
/// diagnostics, as when each one parsed the lines above; a later row reads only its own lines.
#[test]
fn cached_rows_replay_suppressions_and_diagnostics_per_registration() {
    let lines = [
        "const m = 1;",
        "// cuke-dedup:ignore not-a-rule -- reason",
        "// cuke-dedup:ignore duplicate-handler -- kept on purpose",
        "Given('a', () => {}); Given('b', () => {});",
        "",
        "Given('c', () => {});",
    ];
    let file = SourceFile {
        path: "steps.ts".into(),
        language: crate::typescript::TYPESCRIPT,
    };
    let suppressions = InlineSuppressions::new("//", &lines, &file);
    let mut diagnostics = Vec::new();
    let first = suppressions.at(3, &mut diagnostics);
    let second = suppressions.at(3, &mut diagnostics);
    let other_row = suppressions.at(5, &mut diagnostics);

    assert_eq!(first, second);
    assert_eq!(
        first.iter().map(|item| item.rule).collect::<Vec<_>>(),
        [Rule::DuplicateHandler]
    );
    assert!(other_row.is_empty());
    let located: Vec<_> = diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.location.line,
                diagnostic.location.column,
                diagnostic.message.as_str(),
            )
        })
        .collect();
    assert_eq!(
        located,
        [
            (2, 1, "unknown rule `not-a-rule`"),
            (2, 1, "unknown rule `not-a-rule`"),
        ]
    );
}
