use super::{ColumnIndex, BLOCK_BYTES};

/// The previous per-node computation: characters after the last newline before `offset`.
fn prefix_scan_column(source: &str, offset: usize) -> usize {
    source[..offset]
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .count()
        + 1
}

/// Bytes since the last newline before `offset`, as tree-sitter reports a point's column.
fn byte_column(source: &str, offset: usize) -> usize {
    offset
        - source[..offset]
            .rfind('\n')
            .map_or(0, |newline| newline + 1)
}

/// Every character boundary of each source maps to the column the per-node prefix scan gave.
#[test]
fn indexed_columns_match_the_prefix_scan_at_every_boundary() {
    let straddle = format!("{}é tail", "a".repeat(BLOCK_BYTES - 1));
    let long_line = "x€".repeat(3 * BLOCK_BYTES);
    let cases = [
        ("empty", String::new()),
        ("ascii", "Given('a', () => {});\nWhen('b');".to_owned()),
        ("multibyte", "é漢😀 x\n  é😀\n漢".to_owned()),
        (
            "bom and crlf",
            "\u{feff}Given('a');\r\nWhen('é');\r\n".to_owned(),
        ),
        ("char straddles a block boundary", straddle),
        (
            "line spans many blocks",
            format!("first\n{long_line}\nlast"),
        ),
        ("length is a block multiple", "é".repeat(BLOCK_BYTES)),
        ("trailing newline", "a\n".to_owned()),
    ];
    for (label, source) in cases {
        let index = ColumnIndex::new(&source);
        for offset in (0..=source.len()).filter(|offset| source.is_char_boundary(*offset)) {
            assert_eq!(
                index.column(source.as_bytes(), offset, byte_column(&source, offset)),
                prefix_scan_column(&source, offset),
                "{label}: offset {offset}"
            );
        }
    }
}

/// A node location takes rows from tree-sitter and converts both byte columns to characters.
#[test]
fn node_location_converts_start_and_end_columns_to_characters() {
    let source = "const é = 1;\né; Given('漢😀', () => {\n  é;\n});\n";
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .expect("TypeScript grammar loads");
    let tree = parser.parse(source, None).expect("source parses");
    let call = tree
        .root_node()
        .named_child(2)
        .and_then(|statement| statement.named_child(0))
        .expect("third statement is a call expression");
    let matcher = call
        .child_by_field_name("arguments")
        .and_then(|arguments| arguments.named_child(0))
        .expect("call has a matcher argument");
    let file = crate::source_adapter::SourceFile {
        path: "steps.ts".into(),
        language: crate::typescript::TYPESCRIPT,
    };
    let index = ColumnIndex::new(source);
    let span = |node| {
        let location = index.location(&file, node, source);
        (
            location.line,
            location.column,
            location.end_line,
            location.end_column,
        )
    };
    // Byte columns would read (2, 5, 4, 3) and (2, 11, 2, 20).
    assert_eq!(span(call), (2, 4, 4, 3));
    assert_eq!(span(matcher), (2, 10, 2, 14));
}
