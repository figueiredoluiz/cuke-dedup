use super::*;

/// Pins first-seen numbering across repeated spellings, Unicode, parameters and nested locals.
#[test]
fn identifier_numbering_preserves_first_seen_bindings() {
    for (source, expected) in [
        (
            "function z(a, z) { var a; let 界; let b; }",
            vec!["z", "a", "界", "b"],
        ),
        (
            "function z({key: a = outside()}, [b, ...c]: Kind) { const d = other(); }",
            vec!["z", "a", "b", "c", "d"],
        ),
        (
            "function z(a) { let b; { let a; let c; } }",
            vec!["z", "a", "b", "c"],
        ),
        ("function* z(a) { let b; yield a; }", vec!["z", "a", "b"]),
    ] {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{source}");
        let handler = tree.root_node().named_child(0).unwrap();
        let expected: BTreeMap<_, _> = expected
            .into_iter()
            .enumerate()
            .map(|(i, name)| (name.to_owned(), format!("v{i}")))
            .collect();
        assert_eq!(
            declared_identifiers(handler, source.as_bytes()),
            expected,
            "{source}"
        );
    }
}

/// Measures only identifier membership, with setup outside timing and exact ordering controls.
#[test]
#[ignore = "run explicitly without competing test workloads"]
fn identifier_membership_growth_stays_below_quadratic() {
    let mut timings = Vec::new();
    for size in [4_000, 16_000] {
        let names: Vec<_> = (0..size).map(|i| format!("name_{i:05}")).collect();
        let mut best = std::time::Duration::MAX;
        for _ in 0..5 {
            let mut output = Default::default();
            let start = std::time::Instant::now();
            for name in &names {
                push_identifier(&mut output, name);
            }
            best = best.min(start.elapsed());
            assert_eq!(output.into_iter().collect::<Vec<_>>(), names);
        }
        eprintln!("{size} identifiers: {best:?}");
        timings.push(best.as_secs_f64());
    }
    let growth = timings[1] / timings[0];
    assert!(growth < 9.0, "fourfold identifier input grew {growth:.2}x");
}
