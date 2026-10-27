use super::*;

impl Graph<'_> {
    /// Replaces the shared work limit the release build fixes at the assertion-resolution limit.
    pub(super) fn set_proof_limit(&self, limit: usize) {
        self.with_budget(|budget| budget.set_limit(limit));
    }
}

/// Verify isolated constructor proofs require a closed class owner and non-escaping ordinary method use.
#[test]
fn isolated_owner_shapes_retain_only_closed_constructor_proofs() {
    for (declaration, use_site, expected) in [
        ("class Local; def run; end; end", "instance.run", true),
        (
            "class self::Local; def run; end; end",
            "instance.run",
            false,
        ),
        (
            "class Local < Parent; def run; end; end",
            "instance.run",
            false,
        ),
        (
            "module Outer; class Local; def run; end; end; end",
            "instance.run",
            false,
        ),
        ("class Local; def run; end; end", "instance.()", false),
        ("class Local; def run; end; end", "escape(instance)", false),
    ] {
        let units = [test_unit(&format!(
            "{declaration}; instance = Local.new; {use_site}"
        ))];
        assert!(!units[0].tree.root_node().has_error());
        let mut result = Providers::default();
        Graph::new(&units, &[]).isolated_instances(&mut result);
        assert_eq!(
            result
                .0
                .values()
                .any(|proof| !proof.isolated_constants.is_empty()),
            expected,
            "{declaration}; {use_site}"
        );
    }
}

/// Verify ambiguous or protected namespaces cannot supply forwarding proofs.
#[test]
fn forwarders_reject_ambiguous_and_protected_namespace_inventories() {
    let units = [test_unit("module Forward; def self.register(pattern, &handler); REGISTER.call(pattern, &handler); end; end")];
    let owner = descendants(units[0].tree.root_node())
        .into_iter()
        .find(|n| n.kind() == "module")
        .unwrap();
    for (count, protected) in [(2, false), (1, true)] {
        let mut graph = Graph::new(&units, &[]);
        graph.namespaces.insert("Forward".into(), count);
        if protected {
            graph.protected_modules.insert(owner.id());
        }
        graph.collect_forwarders();
        assert!(graph.forwarders.is_empty());
    }
}

/// Verify dynamically named namespaces cannot supply forwarding proofs.
#[test]
fn forwarders_reject_dynamic_namespace_names() {
    let units = [test_unit("module self::Forward; def self.register(pattern, &handler); REGISTER.call(pattern, &handler); end; end")];
    assert!(!units[0].tree.root_node().has_error());
    let mut graph = Graph::new(&units, &[]);
    graph.collect_forwarders();
    assert!(graph.forwarders.is_empty());
}

/// Verify shorthand registration-alias dispatch does not gain exported provider trust.
#[test]
fn registration_alias_shorthand_dispatch_does_not_gain_export_trust() {
    let source = "register = method(:Given); register.('pattern') { work() }";
    let units = [test_unit(source)];
    assert!(!units[0].tree.root_node().has_error());
    let nodes = descendants(units[0].tree.root_node());
    let references = identifier_references(&nodes, source);
    let mut result = Providers::default();
    Graph::new(&units, &[]).local_export_aliases(
        0,
        &nodes,
        &references,
        &mut BTreeSet::new(),
        &mut result,
    );
    assert!(result.0.is_empty());
}

/// Verify exhausted optional proof budgets withdraw indirect definitions while preserving direct findings and reporting uncertainty.
#[test]
fn exhausted_optional_budget_withdraws_proofs_and_retains_direct_findings() {
    use crate::model::Rule;
    use crate::source_adapter::{SourceAdapter, SourceExtractionSession};

    let config_root = tempfile::tempdir().unwrap();
    let config = crate::config::Config::load(
        config_root.path(),
        crate::config::ConfigOverrides::default(),
    )
    .unwrap();
    for (source, optional) in [
        ("def handler; perform(); end; Given('optional one', &method(:handler)); Then('optional two', &method(:handler))", true),
        ("Given('direct') { work() }; Then('direct') { work() }", false),
    ] {
        let units = [test_unit(source)];
        let size = descendants(units[0].tree.root_node()).len();
        let mut exhausted = false;
        let mut complete = false;
        for limit in (0..=size * 20).step_by((size / 4).max(1)) {
            let graph = Graph::new(&units, &[]);
            graph.set_proof_limit(limit);
            let (providers, assertions) = Providers::collect_proofs(&units, &[], graph);
            let incomplete = providers.work_exhausted();
            exhausted |= incomplete;
            complete |= !incomplete;
            if incomplete { assert!(providers.0.is_empty()); }
            let mut session = SourceExtractionSession::default();
            let state = session.state::<super::super::RubySession>().unwrap();
            state.providers = providers;
            state.assertions = assertions;
            let extraction = super::super::RubyAdapter.extract_with_session(source, &units[0].file, &mut session).unwrap();
            assert_eq!(extraction.definitions.len(), if optional && incomplete { 0 } else { 2 }, "limit {limit}: {source}");
            let result = crate::analysis::analyze_with_step_usage(
                extraction.definitions, vec![], &config,
                &extraction.indirect_usage.unwrap_or_default(),
            ).unwrap().result;
            assert_eq!(result.findings.iter().any(|f| f.rule == Rule::DuplicateMatcher), !optional);
            assert_eq!(result.findings.iter().any(|f| f.rule == Rule::DuplicateHandler), optional && !incomplete, "limit {limit}: {source}");
            let finalized = super::super::RubyAdapter.finalize_session(&mut session).unwrap();
            assert_eq!(finalized.uncertainties.iter().any(|u| u.message.contains("work limit")), incomplete);
        }
        assert!(exhausted && complete);
    }
}

/// Verify closed execution stops at prior or partial exhaustion and completes both callbacks when budget permits.
#[test]
fn closed_execution_stops_after_prior_or_partial_budget_exhaustion() {
    let source = "first = -> { Given('first') { work() } }; first.call; second = -> { Then('second') { work() } }; second.call";
    let units = [test_unit(source)];
    let nodes = descendants(units[0].tree.root_node());
    let first_body = nodes
        .iter()
        .find(|n| n.kind() == "lambda")
        .unwrap()
        .child_by_field_name("body")
        .unwrap();
    for prior in [false, true] {
        let graph = Graph::new(&units, &[]);
        graph.set_proof_limit(
            nodes.len() * 2
                + 2
                + descendants(first_body).len()
                + descendants(first_body)
                    .iter()
                    .find(|n| matches!(n.kind(), "block" | "do_block"))
                    .map(|n| descendants(*n).len())
                    .unwrap(),
        );
        // A pass charged past any limit latches exhaustion that the next pass inherits.
        assert_eq!(
            graph.charge_proof_work(if prior { usize::MAX } else { 0 }),
            prior
        );
        let mut result = Providers::default();
        graph.closed_execution(&mut result);
        assert!(graph.budget_exhausted());
        assert_eq!(
            result.0.values().map(|p| p.executed.len()).sum::<usize>(),
            usize::from(!prior)
        );
    }
    let graph = Graph::new(&units, &[]);
    let mut result = Providers::default();
    graph.closed_execution(&mut result);
    assert!(!graph.budget_exhausted());
    assert_eq!(
        result.0.values().map(|p| p.executed.len()).sum::<usize>(),
        2
    );
}

/// Verify proof passes retain exhaustion and load-work accounting while refusing optional proofs.
#[test]
fn optional_passes_share_exhaustion_without_resetting_load_work() {
    let source = "module Local; def self.safe; end; end; register = method(:Given); register.call('step') { work() }";
    let units = [test_unit(source)];
    let graph = Graph::new(&units, &[]);
    graph.set_proof_limit(2);
    assert!(!graph.charge_proof_work(1));
    assert!(!graph.charge_load_work(1));
    assert!(!graph.budget_exhausted());
    assert!(graph.charge_proof_work(2));
    assert!(graph.begin_proof_pass());
    assert_eq!(graph.budget.get().spent(), (1, 0));
    assert!(graph.budget_exhausted());
    let nodes = descendants(units[0].tree.root_node());
    let references = identifier_references(&nodes, source);
    let mut result = Providers::default();
    graph.isolated_instances(&mut result);
    graph.isolated_namespaces(&mut result);
    graph.local_export_aliases(0, &nodes, &references, &mut BTreeSet::new(), &mut result);
    assert!(result.0.is_empty());
}

/// Verify constructors used before class initialization do not gain isolated-instance proofs.
#[test]
fn isolated_constructor_requires_preceding_class_initialization() {
    let units = [test_unit(
        "instance = Local.new; class Local; def run; end; end; instance.run",
    )];
    let mut result = Providers::default();
    Graph::new(&units, &[]).isolated_instances(&mut result);
    assert!(result.0.is_empty());
}

/// Exercises every optional collector at both exhausted and completed work limits.
#[test]
fn optional_proof_collectors_fail_closed_at_each_work_boundary() {
    let source = "def handler; work(); end; callable = proc { work() }; Given('one', &method(:handler)); Then('two', &callable); register = method(:Given); register.call('three') { work() }; module Local; def self.safe; end; end; Local.safe; factory = -> { Given('four') { work() }; Then('five') { work() } }; factory.call; class LocalInstance; def safe; end; end; instance = LocalInstance.new; instance.safe # binding";
    let units = [test_unit(source)];
    let nodes = descendants(units[0].tree.root_node());
    let references = identifier_references(&nodes, source);
    let assertions = super::super::assertions::AssertionProviders::default();
    let mut exhausted = [false; 5];
    let mut completed = [false; 5];
    for limit in 0..nodes.len() * 4 {
        for collector in 0..5 {
            let graph = Graph::new(&units, &[]);
            graph.set_proof_limit(limit);
            let mut result = Providers::default();
            match collector {
                0 => graph.named_handlers(&mut result, Some(&assertions)),
                1 => graph.isolated_namespaces(&mut result),
                2 => graph.local_export_aliases(
                    0,
                    &nodes,
                    &references,
                    &mut BTreeSet::new(),
                    &mut result,
                ),
                3 => graph.closed_execution(&mut result),
                _ => graph.isolated_instances(&mut result),
            }
            exhausted[collector] |= graph.budget_exhausted();
            completed[collector] |= !graph.budget_exhausted();
            if limit == 0 {
                assert!(
                    result.0.is_empty(),
                    "collector {collector} gained trust without work"
                );
            }
            if !graph.budget_exhausted() {
                let proof = &result.0[&units[0].file.path];
                assert_eq!(
                    match collector {
                        0 => proof.handlers.len(),
                        1 => proof.isolated_constants.len(),
                        2 => proof.calls.len(),
                        3 => proof.executed.len(),
                        _ => proof.isolated_constants.len(),
                    },
                    if matches!(collector, 0 | 1 | 3) { 2 } else { 1 }
                );
            }
        }
    }
    assert_eq!(exhausted, [true; 5]);
    assert_eq!(completed, [true; 5]);
}

/// Verify load-proof exhaustion leaves no cached authority and dependency-only targets require explicit marking.
#[test]
fn load_proof_work_exhaustion_withdraws_optional_trust() {
    let graph = Graph::new(&[], &[]);
    let limit = crate::resource_limits::MAX_ASSERTION_RESOLUTION_WORK;
    assert!(!graph.charge_load_work(limit));
    assert!(!graph.available((0, 1, 1), (1, 1, 1), &mut BTreeSet::new()));
    assert_eq!(graph.budget.get().spent(), (limit + 1, 0));
    assert!(graph.available_proofs.borrow().is_empty());
    assert!(graph.budget_exhausted());
    let edge = SourceDependency::new(
        crate::model::SourceLocation::new(PathBuf::from("entry.rb"), 1, 1, 1, 1),
        PathBuf::from("consumer.rb"),
    );
    assert!(!edge.dependency_only);
    assert!(edge.with_dependency_only_target(true).dependency_only);
}

/// Verify exhausted load proofs report session uncertainty without losing direct definitions unless later effects invalidate them.
#[test]
fn exhausted_load_proof_reports_incompleteness_without_losing_direct_definitions() {
    use crate::source_adapter::{SourceAdapter, SourceExtractionSession};
    let file = SourceFile {
        path: PathBuf::from("steps.rb"),
        language: SourceLanguage::Ruby,
    };
    let mut session = SourceExtractionSession::default();
    session
        .state::<super::super::RubySession>()
        .unwrap()
        .providers = Providers(BTreeMap::new(), true);
    let result = super::super::RubyAdapter
        .extract_with_session(
            "Given('first') { work() }; Then('second') { work() }",
            &file,
            &mut session,
        )
        .unwrap();
    assert_eq!(result.definitions.len(), 2);
    assert!(!result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("work limit")));
    let neighbor = SourceFile {
        path: PathBuf::from("neighbor.rb"),
        language: SourceLanguage::Ruby,
    };
    let result = super::super::RubyAdapter
        .extract_with_session("Given('neighbor') { work() }", &neighbor, &mut session)
        .unwrap();
    assert_eq!(result.definitions.len(), 1);
    assert!(!result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("work limit")));
    let finalized = super::super::RubyAdapter
        .finalize_session(&mut session)
        .unwrap();
    assert_eq!(finalized.uncertainties.len(), 1);
    assert!(finalized.uncertainties[0].message.contains("work limit"));
    session
        .state::<super::super::RubySession>()
        .unwrap()
        .effects
        .invalidate(None);
    let finalized = super::super::RubyAdapter
        .finalize_session(&mut session)
        .unwrap();
    let mut definitions = result.definitions;
    finalized.apply(&mut definitions);
    assert!(definitions.is_empty());
    assert_eq!(finalized.uncertainties.len(), 1);
}

/// Provider proofs need a resolvable edge, are absent on cycles and respect file and byte budgets.
#[test]
fn optional_provider_proofs_respect_both_resource_limits() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("alias")).unwrap();
    let provider = "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end";
    let consumer =
        "require_relative 'provider'; register = Provider::GIVEN; register.call('same') { work() }";
    let files: Vec<_> = [("provider.rb", provider), ("entry.rb", consumer)]
        .into_iter()
        .map(|(name, source)| {
            let path = dir.path().join("alias").join("..").join(name);
            std::fs::write(&path, source).unwrap();
            SourceFile {
                path,
                language: SourceLanguage::Ruby,
            }
        })
        .collect();
    let edges = [SourceDependency::new(
        crate::model::SourceLocation::new(files[1].path.clone(), 1, 1, 1, 28),
        crate::config::canonical_platform_path(&files[0].path).unwrap(),
    )];
    let bytes = provider.len() + consumer.len();
    let positive = Providers::collect_with_budget(&files, &edges, (2, bytes)).unwrap();
    assert_eq!(
        positive.get(&files[1].path, consumer).unwrap().calls.len(),
        1
    );
    let mut unavailable = edges.clone();
    unavailable[0].target = dir.path().join("absent.rb");
    assert!(!Providers::collect(&files, &unavailable)
        .unwrap()
        .get(&files[1].path, consumer)
        .is_some_and(|p| !p.calls.is_empty()));
    let mut cycle = edges.to_vec();
    cycle.push(SourceDependency::new(
        edges[0].location.clone(),
        crate::config::canonical_platform_path(&files[1].path).unwrap(),
    ));
    assert!(Providers::collect(&files, &cycle).unwrap().0.is_empty());
    for budget in [(1, bytes), (2, bytes - 1)] {
        assert!(Providers::collect_with_budget(&files, &edges, budget)
            .unwrap()
            .0
            .is_empty());
    }
}

/// Verify a provider unlinked during reading withdraws optional proofs rather than retaining an unusable snapshot.
#[cfg(unix)]
#[test]
fn provider_path_disappearing_during_read_withdraws_optional_proofs() {
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: root.path().join("steps.rb"),
        language: SourceLanguage::Ruby,
    };
    assert!(std::process::Command::new("mkfifo")
        .arg(&file.path)
        .status()
        .unwrap()
        .success());
    let path = file.path.clone();
    // Opening pairs the writer and reader; unlink before sending bytes makes the later
    // canonicalization failure deterministic, without sleeps or filesystem racing.
    let writer = std::thread::spawn(move || {
        let mut pipe = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        pipe.write_all(b"Given('one') { work() }").unwrap();
    });
    let actual = load_units(&[file], (1, 1024)).unwrap();
    writer.join().unwrap();
    assert!(actual.is_none());
}
