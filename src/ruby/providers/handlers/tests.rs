use super::*;

/// Verify bound method proofs require unique preceding bindings, closed class lookup, and available work budget.
#[test]
fn bound_methods_require_unique_bindings_classes_and_unmodified_lookup() {
    for (classes, binding, expected) in [
        (
            "class Bound; def handle; work(); end; end",
            "instance.method(:handle); instance = Bound.new",
            false,
        ),
        (
            "class Bound; def handle; work(); end; end",
            "instance = Cucumber.new",
            false,
        ),
        (
            "class Bound; def handle; work(); end; end",
            "instance = Bound.new",
            true,
        ),
        (
            "class Bound; def handle; work(); end; end",
            "instance = Bound.new; instance = Bound.new",
            false,
        ),
        (
            "class Bound; def handle; work(); end; end; class Bound; end",
            "instance = Bound.new",
            false,
        ),
        (
            "class Bound; def handle; work(); end; alias other handle; end",
            "instance = Bound.new",
            false,
        ),
        (
            "class Bound; def handle; work(); end; def method; end; end",
            "instance = Bound.new",
            false,
        ),
    ] {
        let source = format!("{classes}; {binding}; instance.method(:handle)");
        let units = [test_unit(&source)];
        let root = units[0].tree.root_node();
        assert!(!root.has_error());
        let nodes = descendants(root);
        let references = identifier_references(&nodes, &source);
        let mut assignments = Assignments::new();
        for (assignment, left, value) in root_assignments(&nodes, root) {
            assignments
                .entry(text(left, &source))
                .or_default()
                .push((assignment, left, value));
        }
        let mut graph = Graph::new(&units, &[]);
        graph.classes.insert(
            "Bound".into(),
            nodes
                .iter()
                .filter(|n| n.kind() == "class")
                .map(|n| (0, *n))
                .collect(),
        );
        let receiver = nodes
            .iter()
            .filter(|n| {
                n.kind() == "call"
                    && n.parent() == Some(root)
                    && n.child_by_field_name("method")
                        .is_some_and(|m| text(m, &source) == "method")
            })
            .min_by_key(|n| n.start_byte())
            .unwrap()
            .child_by_field_name("receiver")
            .unwrap();
        for limited in [false, true] {
            if limited {
                graph.set_proof_limit(0);
            }
            let proof =
                graph.bound_instance_method(0, receiver, "handle", (&assignments, &references));
            assert_eq!(
                proof.is_some(),
                expected && !limited,
                "{source}, limited {limited}"
            );
        }
    }
}
