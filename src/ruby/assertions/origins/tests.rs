use super::*;
use crate::source_adapter::SourceFile;

/// Checks work exhaustion, dependency context, and configured-origin controls.
#[test]
fn origin_work_limits_never_publish_partial_authority() {
    let root = tempfile::tempdir().unwrap();
    let sources = [
        ("provider.rb", "module Assertions; def self.expect(actual); actual; end; end"),
        ("fixture.rb", "require_relative 'provider'; module Fixture; EXPECT = Assertions.method(:expect); end"),
        ("steps.rb", "require_relative 'fixture'; check = Fixture::EXPECT; Given('same') { check.call(value).to_be(1) }"),
    ];
    let files: Vec<_> = sources
        .iter()
        .map(|(name, source)| {
            let path = root.path().join(name);
            std::fs::write(&path, source).unwrap();
            SourceFile {
                path,
                language: crate::ruby::RUBY,
            }
        })
        .collect();
    let units = super::super::super::providers::load_units(&files, (3, 4096))
        .unwrap()
        .unwrap();
    let edges: Vec<_> = [1, 2]
        .into_iter()
        .map(|index| {
            let unit = &units[index];
            SourceDependency::new(
                location(
                    &unit.file,
                    unit.tree.root_node().named_child(0).unwrap(),
                    &unit.source,
                ),
                units[index - 1].canonical_path.clone(),
            )
        })
        .collect();
    let mut exhausted = false;
    let mut complete_from = None;
    for budget in 0..4000 {
        match Origins::collect(&units, &edges, &["provider".into()], None, budget) {
            None => exhausted = true,
            Some(origins) => {
                complete_from.get_or_insert(budget);
                assert_eq!(origins[&files[2].path].factories.len(), 1);
            }
        }
    }
    assert!(exhausted);
    // The first complete budget is the total charge on this graph, so a dropped or moved
    // charge shifts it.
    assert_eq!(complete_from, Some(3148));
    let absolute = units[0].canonical_path.to_string_lossy().into_owned();
    let origins = Origins::collect(&units, &edges, &[absolute], None, usize::MAX).unwrap();
    assert!(origins[&files[2].path]
        .factories
        .values()
        .all(|factory| factory.trusted));
    let mut missing_call = edges[1].clone();
    missing_call.location.line = 999;
    missing_call.target = units[0].canonical_path.clone();
    let contexts = preceding_context(
        1,
        &units,
        &[edges[1].clone(), missing_call],
        &BTreeMap::new(),
        0,
        &mut WorkBudget::new(usize::MAX),
    )
    .unwrap();
    assert!(contexts.0.is_empty() && contexts.1.is_empty());
    std::fs::write(&files[1].path, "later { require_relative 'provider' }").unwrap();
    let nested = super::super::super::providers::load_units(&files, (3, 4096))
        .unwrap()
        .unwrap();
    let call = descendants(nested[1].tree.root_node())
        .into_iter()
        .find(|node| {
            node.child_by_field_name("method")
                .is_some_and(|method| text(method, &nested[1].source) == "require_relative")
        })
        .unwrap();
    let nested_edge = SourceDependency::new(
        location(&nested[1].file, call, &nested[1].source),
        nested[0].canonical_path.clone(),
    );
    assert!(preceding_context(
        0,
        &nested,
        &[nested_edge],
        &BTreeMap::new(),
        0,
        &mut WorkBudget::new(usize::MAX)
    )
    .is_none());
    for (depth, budget) in [(32, usize::MAX), (0, 0), (0, 8)] {
        let mut work = WorkBudget::new(budget);
        assert!(preceding_context(1, &units, &edges, &BTreeMap::new(), depth, &mut work).is_none());
        if depth == 32 {
            // The depth cut still charges its visit, so a deep chain cannot hide work.
            assert_eq!(work.spent(), units.len() + edges.len());
        }
    }
    let node = units[2].tree.root_node().named_child(1).unwrap();
    assert!(resolve(
        node,
        &units[2].source,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &units,
        32
    )
    .is_none());
}
