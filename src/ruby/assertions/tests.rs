use super::*;
use crate::model::Rule;
use crate::source_adapter::SourceExtractionSession;

/// Checks that optional proof failures retain ordinary findings and correct completeness.
#[test]
fn proof_exhaustion_and_cycles_preserve_untrusted_baseline_findings() {
    let root = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: root.path().join("steps.rb"),
        language: crate::ruby::RUBY,
    };
    let source = "require_relative './steps'\nGiven('same') { work() }; Then('same') { work() }; Then('other') { work() }";
    std::fs::write(&file.path, source).unwrap();
    let mut session = SourceExtractionSession::new(root.path());
    let definitions = crate::source_adapter::adapter_for_language(file.language)
        .unwrap()
        .extract_with_session(source, &file, &mut session)
        .unwrap()
        .definitions;
    let config = crate::config::Config::load(root.path(), Default::default()).unwrap();
    let loaded = super::super::providers::load_units(std::slice::from_ref(&file), (1, 4096))
        .unwrap()
        .unwrap();
    let loop_edge = SourceDependency::new(
        definitions[0].location.clone(),
        crate::config::canonical_platform_path(&file.path).unwrap(),
    );
    let mut cases: Vec<_> = [vec![], vec!["./steps".to_owned()]]
        .into_iter()
        .map(|configured| {
            (
                AssertionProviders::from_units(&loaded, &[], &configured, 0),
                !configured.is_empty(),
            )
        })
        .collect();
    for (budget, edges) in [
        ((0, 1024, 1024), vec![]),
        ((1, 0, 1024), vec![]),
        ((1, 1024, 0), vec![]),
        ((1, 1024, 1024), vec![loop_edge]),
    ] {
        cases.push((
            AssertionProviders::collect_with_budget(
                std::slice::from_ref(&file),
                &edges,
                &["./steps".into()],
                budget,
            )
            .unwrap(),
            true,
        ));
    }
    for (providers, incomplete) in cases {
        assert_eq!(providers.incomplete(), incomplete);
        assert!(providers.get(&file.path, source).is_none());
        session
            .state::<crate::ruby::RubySession>()
            .unwrap()
            .assertions = providers;
        let mut actual = definitions.clone();
        let finalized = session
            .finalize(std::slice::from_ref(&file), &mut actual)
            .unwrap();
        assert_eq!(finalized.uncertainties.len(), usize::from(incomplete));
        assert_eq!(actual.len(), 3);
        let result = crate::analysis::analyze_with_diagnostics(actual, vec![], &config)
            .unwrap()
            .result;
        for rule in [Rule::DuplicateMatcher, Rule::DuplicateHandler] {
            assert!(result.findings.iter().any(|finding| finding.rule == rule));
        }
    }
}

/// Checks source, work, and dependency boundaries before publishing assertion trust.
#[test]
fn proof_budget_checkpoints_and_invalid_edges_never_grant_trust() {
    let root = tempfile::tempdir().unwrap();
    let files = provider_files(root.path());
    let source = "require_relative 'assertions'\ndef helper; require_relative 'assertions'; end\nself.require_relative 'assertions'\nrequire_relative 'other'\nGiven('one') { Assertions.expect(page).to_be(UNKNOWN) }";
    std::fs::write(&files[0].path, source).unwrap();
    let provider = "module Assertions; def self.expect(actual); actual; end; end";
    std::fs::write(&files[1].path, provider).unwrap();
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    let mut loads: Vec<_> = descendants(tree.root_node())
        .into_iter()
        .filter(|n| {
            n.child_by_field_name("method")
                .is_some_and(|m| text(m, source) == "require_relative")
        })
        .collect();
    loads.sort_by_key(|node| node.start_byte());
    let edge = |node| {
        SourceDependency::new(
            location(&files[0], node, source),
            crate::config::canonical_platform_path(&files[1].path).unwrap(),
        )
    };
    let configured = ["./assertions".into()];
    let collect = |edges: &[SourceDependency], work| {
        AssertionProviders::collect_with_budget(&files, edges, &configured, (2, 4096, work))
            .unwrap()
    };
    let valid = edge(loads[0]);
    for work in 0..=2000 {
        let proofs = collect(std::slice::from_ref(&valid), work);
        if proofs.incomplete() {
            assert!(proofs.get(&files[0].path, source).is_none());
        } else {
            assert_eq!(
                proofs.get(&files[0].path, source).unwrap().factories.len(),
                1
            );
        }
    }
    assert!(!collect(std::slice::from_ref(&valid), 2000).incomplete());
    let mut missing_location = valid.clone();
    missing_location.location.line = 999;
    let mut missing_target = valid.clone();
    missing_target.target = root.path().join("unselected.rb");
    for invalid in [
        missing_location,
        missing_target,
        edge(loads[1]),
        edge(loads[2]),
        edge(loads[3]),
    ] {
        let proofs = collect(&[invalid], 2000);
        assert!(!proofs.incomplete());
        assert!(proofs
            .get(&files[0].path, source)
            .is_none_or(|bindings| bindings.factories.is_empty()));
    }
    for provider in ["module Assertions; end", "module Outer::Assertions; end"] {
        std::fs::write(&files[1].path, provider).unwrap();
        assert!(collect(std::slice::from_ref(&valid), 2000)
            .get(&files[0].path, source)
            .is_none_or(|bindings| bindings.factories.is_empty()));
    }
}

/// Many closed providers make this collector's own counter, not the origins pass that runs first
/// with a separate counter under the same limit, decide where proofs become available.
#[test]
fn assertion_work_boundary_is_decided_by_its_own_counter() {
    let root = tempfile::tempdir().unwrap();
    let files = provider_files(root.path());
    let source =
        "require_relative 'assertions'\nGiven('one') { Assertions0.expect(page).to_be(1) }";
    std::fs::write(&files[0].path, source).unwrap();
    // Discriminating shape: each closed `expect` module charges the whole graph twice here,
    // while origins charge it about once.
    let provider: String = (0..20)
        .map(|index| {
            format!("module Assertions{index}; def self.expect(actual); actual; end; end\n")
        })
        .collect();
    std::fs::write(&files[1].path, provider).unwrap();
    let units = super::super::providers::load_units(&files, (2, 1 << 20))
        .unwrap()
        .unwrap();
    let load = units[0].tree.root_node().named_child(0).unwrap();
    let edges = [SourceDependency::new(
        location(&files[0], load, source),
        units[1].canonical_path.clone(),
    )];
    let configured = ["./assertions".into()];
    let first_complete = |complete: &dyn Fn(usize) -> bool| {
        let (mut low, mut high) = (0, 100_000);
        while low < high {
            let middle = (low + high) / 2;
            if complete(middle) {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        low
    };
    let origins = first_complete(&|work| {
        origins::Origins::collect(&units, &edges, &configured, None, work).is_some()
    });
    let assertions = first_complete(&|work| {
        !AssertionProviders::from_units(&units, &edges, &configured, work).incomplete()
    });
    assert!(origins < assertions, "{origins} {assertions}");
    assert_eq!(assertions, 10453);
}

/// Session proofs survive source deletion without reloading and are keyed by current source bytes.
#[test]
fn shared_snapshot_retains_proofs_without_reloading_and_checks_current_source_bytes() {
    let root = tempfile::tempdir().unwrap();
    let files = provider_files(root.path());
    let source = "require_relative 'assertions'\nGiven('one') { Assertions.expect(page).to_be(UNKNOWN) }; Then('two') { Assertions.expect(page).to_be(UNKNOWN) }; Given('ordinary one') { work() }; Then('ordinary two') { work() }";
    std::fs::write(&files[0].path, source).unwrap();
    std::fs::write(
        &files[1].path,
        "module Assertions; def self.expect(actual); actual; end; end",
    )
    .unwrap();
    let units = super::super::providers::load_units(&files, (2, 4096))
        .unwrap()
        .unwrap();
    let root_node = units[0].tree.root_node();
    let load = root_node.named_child(0).unwrap();
    let edges = [SourceDependency::new(
        location(&files[0], load, source),
        crate::config::canonical_platform_path(&files[1].path).unwrap(),
    )];
    for file in &files {
        std::fs::remove_file(&file.path).unwrap();
    }
    let mut session =
        SourceExtractionSession::with_options(root.path(), &[], &["./assertions".into()])
            .with_dependencies(&edges);
    let state = session.state::<crate::ruby::RubySession>().unwrap();
    state.providers = super::super::providers::Providers::from_units(&units, &edges).unwrap();
    state.assertions =
        AssertionProviders::from_units(&units, &edges, &["./assertions".into()], 2000);
    assert!(state.assertions.get(&files[0].path, source).is_some());
    assert!(state
        .assertions
        .get(&files[0].path, &format!("{source}\nchanged()"))
        .is_none());
    let (definitions, finalized) = finish(source, &files, &mut session);
    assert!(finalized.uncertainties.is_empty());
    assert!(finalized.advisories.is_empty());
    assert_eq!(definitions.len(), 4);
    let config = crate::config::Config::load(root.path(), Default::default()).unwrap();
    assert_eq!(handler_findings(definitions, &config), 1);
}

/// Checks rejected-provider advisories alongside preserved ordinary analysis outcomes.
#[test]
fn rejected_configured_trust_is_advisory_without_changing_final_outcomes() {
    let root = tempfile::tempdir().unwrap();
    let files = provider_files(root.path());
    let config = crate::config::Config::load(root.path(), Default::default()).unwrap();
    for (provider, trusted) in [
        (
            "module Assertions; def self.expect(actual); actual; end; end",
            true,
        ),
        ("module Assertions; end", false),
        (
            "module Assertions; def self.expect(actual); actual; end; end\nAlias = Assertions",
            true,
        ),
    ] {
        let source = "require_relative 'assertions'\nGiven('one') { Assertions.expect(page).to_be(UNKNOWN) }; Then('two') { Assertions.expect(page).to_be(UNKNOWN) }; Given('ordinary one') { work() }; Then('ordinary two') { work() }; Given('block one') { Assertions.expect(page) { first() }.to_be(true) }; Then('block two') { Assertions.expect(page) { second() }.to_be(true) }; Given('dispatch one') { Assertions.expect(page).to_be(true) }; Then('dispatch two') { Assertions&.expect(page).to_be(true) }";
        std::fs::write(&files[1].path, provider).unwrap();
        let (definitions, finalized) = extract_configured(root.path(), &files, source);
        assert!(finalized.uncertainties.is_empty());
        assert_eq!(finalized.advisories.len(), usize::from(!trusted));
        if trusted {
            assert_ne!(
                definitions[4].handler.behavior_signature,
                definitions[5].handler.behavior_signature
            );
            assert_ne!(
                definitions[6].handler.behavior_signature,
                definitions[7].handler.behavior_signature
            );
        }
        if !trusted {
            assert_eq!(finalized.advisories[0].location.path, files[0].path);
        }
        assert_eq!(
            handler_findings(definitions, &config),
            if trusted { 1 } else { 2 }
        );
    }
}
/// Every producer path writes the effect-stream and capture-scope typed fields and no marker
/// event: ordinary and unmodeled handlers, `__FILE__` captures, configured assertion streams
/// and bound-instance handlers (a parameterized scope that replaces a bare one).
#[test]
fn typed_handler_facts_are_the_only_carriers_on_every_producer_path() {
    use crate::model::EffectStream::{ActionOnly, Complete, Partial};
    /// One extracted source and what each of its definitions must carry.
    struct Row {
        context: &'static str,
        source: String,
        stream: crate::model::EffectStream,
        scope: Option<String>,
        definitions: usize,
    }
    let bound = |body: &str| {
        format!("class Bound; def handle; work({body}); end; end\ninstance = Bound.new; Given('same', &instance.method(:handle)); Then('same', &instance.method(:handle))")
    };
    // The scope key of a bound handler is the receiver allocation's byte span.
    let allocation = |source: &str| {
        let start = source.find("Bound.new").unwrap();
        Some(format!("{start}:{}", start + "Bound.new".len()))
    };
    let row = |context, source: &str, stream, scope: Option<&str>| Row {
        context,
        source: source.to_owned(),
        stream,
        scope: scope.map(str::to_owned),
        definitions: 1,
    };
    let (bound_plain, bound_bare) = (bound(""), bound("__FILE__"));
    let rows = [
        row("ordinary calls", "Given('a') { work() }", ActionOnly, None),
        row(
            "unmodeled construct",
            "Given('a') { foo { |x| x } }",
            Partial,
            None,
        ),
        row(
            "bare scope",
            "Given('a') { puts __FILE__ }",
            ActionOnly,
            Some(""),
        ),
        row(
            "scope without a stream",
            "Given('a') { foo { |x| __FILE__ } }",
            Partial,
            Some(""),
        ),
        row(
            "configured assertions",
            "require_relative 'assertions'\nGiven('a') { Assertions.expect(page).to_be(true) }",
            Complete,
            None,
        ),
        row(
            "assertions with scope",
            "require_relative 'assertions'\nGiven('a') { Assertions.expect(page).to_be(__FILE__) }",
            Complete,
            Some(""),
        ),
        Row {
            definitions: 2,
            scope: allocation(&bound_plain),
            ..row("bound instance", &bound_plain, ActionOnly, None)
        },
        Row {
            definitions: 2,
            scope: allocation(&bound_bare),
            ..row(
                "bound instance with bare scope",
                &bound_bare,
                ActionOnly,
                None,
            )
        },
    ];
    let root = tempfile::tempdir().unwrap();
    let files = provider_files(root.path());
    std::fs::write(
        &files[1].path,
        "module Assertions; def self.expect(actual); actual; end; end",
    )
    .unwrap();
    for row in rows {
        let (definitions, _) = extract_configured(root.path(), &files, &row.source);
        assert_eq!(definitions.len(), row.definitions, "{}", row.context);
        for definition in &definitions {
            let handler = &definition.handler;
            assert_eq!(handler.effect_stream, row.stream, "{}", row.context);
            assert_eq!(handler.capture_scope, row.scope, "{}", row.context);
            assert!(
                handler
                    .behavior_signature
                    .iter()
                    .all(|event| !event.starts_with("method:")),
                "{}: {:?}",
                row.context,
                handler.behavior_signature
            );
        }
    }
}

/// Writes `source` as the entry file, declares `./assertions` as an assertion module reached
/// from it, and extracts and finalizes the entry file.
fn extract_configured(
    root: &Path,
    files: &[SourceFile],
    source: &str,
) -> (
    Vec<crate::model::StepDefinition>,
    crate::source_adapter::SourceFinalization,
) {
    std::fs::write(&files[0].path, source).unwrap();
    let edges = [SourceDependency::new(
        crate::model::SourceLocation::new(files[0].path.clone(), 1, 1, 1, 30),
        crate::config::canonical_platform_path(&files[1].path).unwrap(),
    )];
    let mut session = SourceExtractionSession::with_options(root, &[], &["./assertions".into()])
        .with_dependencies(&edges);
    let adapter = crate::source_adapter::adapter_for_language(crate::ruby::RUBY).unwrap();
    adapter.prepare_session(files, &mut session).unwrap();
    finish(source, files, &mut session)
}

/// Extract the entry file, finalize its session, and return definitions with the finalization result.
fn finish(
    source: &str,
    files: &[SourceFile],
    session: &mut SourceExtractionSession,
) -> (
    Vec<crate::model::StepDefinition>,
    crate::source_adapter::SourceFinalization,
) {
    let mut definitions = crate::source_adapter::adapter_for_language(crate::ruby::RUBY)
        .unwrap()
        .extract_with_session(source, &files[0], session)
        .unwrap()
        .definitions;
    let finalized = session.finalize(files, &mut definitions).unwrap();
    (definitions, finalized)
}

/// Build Ruby entry and assertion-provider file descriptors under the test root.
fn provider_files(root: &Path) -> Vec<SourceFile> {
    ["steps.rb", "assertions.rb"]
        .map(|name| SourceFile {
            path: root.join(name),
            language: crate::ruby::RUBY,
        })
        .into()
}

/// Count duplicate-handler findings produced from the supplied definitions and configuration.
fn handler_findings(
    definitions: Vec<crate::model::StepDefinition>,
    config: &crate::config::Config,
) -> usize {
    crate::analysis::analyze_with_diagnostics(definitions, vec![], config)
        .unwrap()
        .result
        .findings
        .into_iter()
        .filter(|finding| finding.rule == Rule::DuplicateHandler)
        .count()
}

/// Verify unavailable provider snapshots preserve ordinary findings and report uncertainty only for configured providers.
#[test]
fn unavailable_snapshot_preserves_ordinary_findings_and_configured_uncertainty() {
    let root = tempfile::tempdir().unwrap();
    let files = provider_files(root.path());
    let config = crate::config::Config::load(root.path(), Default::default()).unwrap();
    let source = "Given('one') { work() }; Then('two') { work() }";
    let adapter = crate::source_adapter::adapter_for_language(crate::ruby::RUBY).unwrap();
    for configured in [vec![], vec!["./assertions".into()]] {
        let mut session = SourceExtractionSession::with_options(root.path(), &[], &configured);
        adapter.prepare_session(&files, &mut session).unwrap();
        let (definitions, finalized) = finish(source, &files, &mut session);
        assert_eq!(
            finalized.uncertainties.len(),
            usize::from(!configured.is_empty())
        );
        assert!(finalized.advisories.is_empty());
        assert_eq!(handler_findings(definitions, &config), 1);
    }
}
