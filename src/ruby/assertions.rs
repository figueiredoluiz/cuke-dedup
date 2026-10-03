//! Explicit provider provenance and value-sensitive assertion evidence, without execution.

use super::{descendants, literal_string, location, text};
use crate::model::BehaviorEvent;

#[cfg(test)]
use crate::source_adapter::SourceFile;
use crate::source_adapter::{SourceAdvisory, SourceDependency};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tree_sitter::Node;

#[derive(Default)]
pub(super) struct AssertionProviders {
    proofs: BTreeMap<PathBuf, (String, AssertionBindings)>,
    incomplete: bool,
    advisories: Vec<SourceAdvisory>,
}

#[derive(Clone, Default)]
pub(super) struct AssertionBindings {
    // Source offsets bind trust to a preceding resolved load and a closed constant identity.
    factories: BTreeSet<(usize, usize)>,
}

impl AssertionProviders {
    pub(super) fn unavailable() -> Self {
        Self {
            incomplete: true,
            ..Self::default()
        }
    }
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }
    #[cfg(test)]
    fn collect_with_budget(
        files: &[SourceFile],
        edges: &[SourceDependency],
        configured: &[String],
        budget: (usize, usize, usize),
    ) -> Result<Self> {
        if configured.is_empty() {
            return Ok(Self::default());
        }
        let Some(loaded) = super::providers::load_units(files, (budget.0, budget.1))? else {
            return Ok(Self::unavailable());
        };
        Self::from_units(&loaded, edges, configured, budget.2)
    }

    pub(super) fn from_units(
        loaded: &[super::providers::Unit],
        edges: &[SourceDependency],
        configured: &[String],
        work_budget: usize,
    ) -> Result<Self> {
        Self::from_units_with_registrations(loaded, edges, configured, work_budget, None)
    }

    pub(super) fn from_units_with_registrations(
        loaded: &[super::providers::Unit],
        edges: &[SourceDependency],
        configured: &[String],
        work_budget: usize,
        registrations: Option<&super::providers::Providers>,
    ) -> Result<Self> {
        if configured.is_empty() {
            return Ok(Self::default());
        }
        let canonical: Vec<_> = loaded
            .iter()
            .map(|unit| unit.canonical_path.clone())
            .collect();
        let units: Vec<_> = loaded
            .iter()
            .map(|unit| (&unit.file, unit.source.as_str(), &unit.tree))
            .collect();
        let nodes: Vec<_> = units
            .iter()
            .map(|(_, _, tree)| descendants(tree.root_node()))
            .collect();
        let total_nodes: usize = nodes.iter().map(Vec::len).sum();
        let mut work = total_nodes.saturating_mul(2);
        if work > work_budget {
            return Ok(Self::unavailable());
        }
        // A configured JS/TS-only module cannot make an unrelated Ruby graph incomplete.
        work = work.saturating_add(total_nodes);
        if work > work_budget {
            return Ok(Self::unavailable());
        }
        if !units.iter().zip(&nodes).any(|((_, source, tree), nodes)| {
            nodes.iter().any(|node| {
                node.parent() == Some(tree.root_node())
                    && configured_load(*node, source, configured)
            })
        }) {
            return Ok(Self::default());
        }
        if units
            .iter()
            .any(|(_, _, tree)| tree.root_node().has_error())
        {
            return Ok(Self::unavailable());
        }
        let mut namespaces = BTreeMap::<String, usize>::new();
        for ((_, source, _), nodes) in units.iter().zip(&nodes) {
            for node in nodes {
                if matches!(node.kind(), "module" | "class") {
                    if let Some(name) = node.child_by_field_name("name") {
                        *namespaces.entry(text(name, source).to_owned()).or_default() += 1;
                    }
                }
            }
        }
        if !super::providers::source_graph_acyclic(
            units
                .iter()
                .zip(&canonical)
                .map(|((file, _, _), canonical)| (file.path.as_path(), canonical.as_path())),
            edges,
        ) {
            return Ok(Self::unavailable());
        }
        let mut result = Self::default();
        for (unit, (file, source, tree)) in units.iter().enumerate() {
            let root = tree.root_node();
            let mut factories = BTreeSet::new();
            for edge in edges.iter().filter(|edge| edge.location.path == file.path) {
                work = work.saturating_add(nodes[unit].len());
                if work > work_budget {
                    return Ok(Self::unavailable());
                }
                let Some(load) = nodes[unit].iter().copied().find(|node| {
                    node.kind() == "call" && location(file, *node, source) == edge.location
                }) else {
                    continue;
                };
                if load.parent() != Some(root) || load.child_by_field_name("receiver").is_some() {
                    continue;
                }
                if !configured_load(load, source, configured) {
                    continue;
                }
                let Some(provider) = canonical.iter().position(|path| path == &edge.target) else {
                    continue;
                };
                work = work.saturating_add(nodes[provider].len().saturating_mul(2));
                if work > work_budget {
                    return Ok(Self::unavailable());
                }
                let mut trusted = false;
                let (_, provider_source, provider_tree) = &units[provider];
                let provider_root = provider_tree.root_node();
                for module in nodes[provider]
                    .iter()
                    .copied()
                    .filter(|node| node.kind() == "module" && node.parent() == Some(provider_root))
                {
                    let Some(name) = module
                        .child_by_field_name("name")
                        .filter(|name| name.kind() == "constant")
                    else {
                        continue;
                    };
                    let name = text(name, provider_source);
                    if namespaces.get(name) != Some(&1)
                        || super::ownership::protected_namespace(name)
                        || !closed_factory(module, provider_source)
                    {
                        continue;
                    }
                    work = work
                        .saturating_add(total_nodes.saturating_mul(2))
                        .saturating_add(nodes[unit].len());
                    if work > work_budget {
                        return Ok(Self::unavailable());
                    }
                    // Any escape, alias, replacement, or unknown member use removes trust globally.
                    if !units.iter().zip(&nodes).all(|((file, input, _), nodes)| {
                        !nodes.iter().any(|node| {
                            reflective_call(*node, input)
                                && !registrations
                                    .and_then(|p| p.get(&file.path, input))
                                    .is_some_and(|p| {
                                        p.handler_captures.contains(&node.start_byte())
                                    })
                        }) && nodes
                            .iter()
                            .filter(|node| node.kind() == "constant" && text(**node, input) == name)
                            .all(|node| closed_reference(*node, input))
                    }) {
                        continue;
                    }
                    trusted = true;
                    for node in &nodes[unit] {
                        if node.start_byte() > load.end_byte()
                            && node.kind() == "call"
                            && node
                                .child_by_field_name("receiver")
                                .is_some_and(|receiver| {
                                    receiver.kind() == "constant" && text(receiver, source) == name
                                })
                            && node
                                .child_by_field_name("method")
                                .is_some_and(|method| text(method, source) == "expect")
                        {
                            factories.insert((node.start_byte(), node.end_byte()));
                        }
                    }
                }
                if !trusted {
                    result.advisories.push(SourceAdvisory::new(
                        edge.location.clone(),
                        "configured Ruby assertion provider lacks a closed factory proof; unsupported declarations, namespace escapes or mutation remove assertion trust",
                    ));
                }
            }
            result.proofs.insert(
                file.path.clone(),
                (source.to_string(), AssertionBindings { factories }),
            );
        }
        Ok(result)
    }

    pub(super) fn advisories(&self) -> &[SourceAdvisory] {
        &self.advisories
    }

    pub fn get(&self, file: &Path, source: &str) -> Option<AssertionBindings> {
        self.proofs
            .get(file)
            .filter(|(input, _)| input == source)
            .map(|(_, bindings)| bindings.clone())
    }
}

fn configured_load(node: Node<'_>, source: &str, configured: &[String]) -> bool {
    node.kind() == "call"
        && node.child_by_field_name("receiver").is_none()
        && node
            .child_by_field_name("method")
            .is_some_and(|method| text(method, source) == "require_relative")
        && node
            .child_by_field_name("arguments")
            .filter(|args| semantic_arity(*args) == 1)
            .and_then(|args| args.named_child(0))
            .and_then(|arg| literal_string(text(arg, source)))
            .is_some_and(|specifier| {
                configured
                    .iter()
                    .any(|item| item.trim_start_matches("./") == specifier.trim_start_matches("./"))
            })
}

fn reflective_call(node: Node<'_>, source: &str) -> bool {
    super::may_replace_dsl(node, source)
        || (node.kind() == "call"
            && node.child_by_field_name("method").is_some_and(|method| {
                matches!(
                    text(method, source),
                    "define_method"
                        | "define_singleton_method"
                        | "alias_method"
                        | "remove_method"
                        | "undef_method"
                        | "send"
                        | "public_send"
                        | "__send__"
                )
            }))
}

fn closed_factory(module: Node<'_>, source: &str) -> bool {
    let Some(body) = module.child_by_field_name("body") else {
        return false;
    };
    let mut cursor = body.walk();
    let declarations: Vec<_> = body
        .named_children(&mut cursor)
        .filter(|node| !node.is_extra())
        .collect();
    declarations.len() == 1
        && descendants(declarations[0]).iter().all(|node| {
            node.id() == declarations[0].id()
                || (!matches!(
                    node.kind(),
                    "method"
                        | "singleton_method"
                        | "class"
                        | "module"
                        | "singleton_class"
                        | "alias"
                        | "undef"
                ) && !reflective_call(*node, source))
        })
        && declarations[0].kind() == "singleton_method"
        && declarations[0]
            .child_by_field_name("object")
            .is_some_and(|object| object.kind() == "self")
        && declarations[0]
            .child_by_field_name("name")
            .is_some_and(|name| text(name, source) == "expect")
}

fn closed_reference(node: Node<'_>, source: &str) -> bool {
    node.parent().is_some_and(|parent| {
        (parent.kind() == "module" && parent.child_by_field_name("name") == Some(node))
            || (parent.kind() == "call"
                && parent.child_by_field_name("receiver") == Some(node)
                && parent
                    .child_by_field_name("method")
                    .is_some_and(|method| text(method, source) == "expect"))
    })
}

impl AssertionBindings {
    pub fn events(
        &self,
        block: Node<'_>,
        source: &str,
        bindings: &super::bindings::Bindings,
    ) -> Vec<BehaviorEvent> {
        let mut events = Vec::new();
        let mut pending = vec![(block, false)];
        while let Some((node, deferred)) = pending.pop() {
            let deferred = deferred
                || (node.id() != block.id()
                    && matches!(node.kind(), "block" | "do_block" | "lambda"));
            if let Some(event) = self.assertion(node, source, bindings) {
                let parameterized = deferred && {
                    let mut parent = node.parent();
                    let mut found = false;
                    while let Some(scope) = parent {
                        if scope.id() == block.id() {
                            break;
                        }
                        found |= matches!(scope.kind(), "block" | "do_block" | "lambda")
                            && scope
                                .child_by_field_name("parameters")
                                .is_some_and(|params| params.named_child_count() != 0);
                        parent = scope.parent();
                    }
                    found
                };
                events.push(if parameterized {
                    BehaviorEvent::unresolved_assertion(true)
                } else if deferred {
                    event.deferred()
                } else {
                    event
                });
                continue;
            }
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            pending.extend(children.into_iter().rev().map(|child| (child, deferred)));
        }
        events
    }

    fn assertion(
        &self,
        node: Node<'_>,
        source: &str,
        bindings: &super::bindings::Bindings,
    ) -> Option<BehaviorEvent> {
        if node.kind() != "call" || node.child_by_field_name("block").is_some() {
            return None;
        }
        let method = text(node.child_by_field_name("method")?, source);
        // Configured factory contract; ordinary same-named objects never enter this path.
        if !matches!(
            method,
            "to_be" | "to_equal" | "equal_to" | "to_have_class" | "to_be_visible"
        ) {
            return None;
        }
        let mut receiver = node.child_by_field_name("receiver")?;
        let mut modifiers = Vec::new();
        for _ in 0..16 {
            if self
                .factories
                .contains(&(receiver.start_byte(), receiver.end_byte()))
            {
                break;
            }
            if receiver.kind() != "call"
                || receiver.child_by_field_name("block").is_some()
                || receiver
                    .child_by_field_name("arguments")
                    .is_some_and(|args| args.named_child_count() != 0)
            {
                return None;
            }
            let modifier = text(receiver.child_by_field_name("method")?, source);
            if !matches!(modifier, "not" | "to") {
                return None;
            }
            modifiers.push(modifier);
            receiver = receiver.child_by_field_name("receiver")?;
        }
        if !self
            .factories
            .contains(&(receiver.start_byte(), receiver.end_byte()))
        {
            return None;
        }
        let Some(subjects) = receiver.child_by_field_name("arguments") else {
            return Some(BehaviorEvent::unresolved_assertion(false));
        };
        if semantic_arity(subjects) != 1 {
            return Some(BehaviorEvent::unresolved_assertion(false));
        }
        let expected = node.child_by_field_name("arguments");
        let arity = if method == "to_be_visible" { 0 } else { 1 };
        if expected.map_or(0, semantic_arity) != arity {
            return Some(BehaviorEvent::unresolved_assertion(false));
        }
        if expected.is_some_and(|args| {
            descendants(args).iter().any(|value| {
                !value.is_extra()
                    && !matches!(
                        value.kind(),
                        "argument_list"
                            | "string"
                            | "string_content"
                            | "escape_sequence"
                            | "simple_symbol"
                            | "hash_key_symbol"
                            | "integer"
                            | "float"
                            | "true"
                            | "false"
                            | "nil"
                            | "array"
                            | "hash"
                            | "pair"
                    )
            })
        }) {
            return Some(BehaviorEvent::unresolved_assertion(false));
        }
        let qualifier = std::iter::once("expect")
            .chain(modifiers.into_iter().rev())
            .collect::<Vec<_>>()
            .join(".");
        let serialize = |root: Node<'_>| {
            serde_json::to_string(&super::handler::syntax_tokens(root, source, bindings).1)
                .expect("assertion tokens serialize")
        };
        Some(BehaviorEvent::assertion(
            &qualifier,
            method,
            // A proved factory can still carry a block or conditional dispatch.
            &serialize(receiver),
            &expected.map(serialize).unwrap_or_default(),
        ))
    }
}

fn semantic_arity(node: Node<'_>) -> usize {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !child.is_extra())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rule;
    use crate::source_adapter::{SourceExtractionSession, SourceLanguage};

    #[test]
    fn proof_exhaustion_and_cycles_preserve_untrusted_baseline_findings() {
        let root = tempfile::tempdir().unwrap();
        let file = SourceFile {
            path: root.path().join("steps.rb"),
            language: SourceLanguage::Ruby,
        };
        let source = "require_relative './steps'\nGiven('same') { work() }; Then('same') { work() }; Then('other') { work() }";
        std::fs::write(&file.path, source).unwrap();
        let mut session = SourceExtractionSession::new(root.path());
        let definitions = crate::source_adapter::adapter_for_language(file.language)
            .extract_with_session(source, &file, &mut session)
            .unwrap()
            .definitions;
        let config = crate::config::Config::load(root.path(), Default::default()).unwrap();
        let loop_edge = SourceDependency::new(
            definitions[0].location.clone(),
            file.path.canonicalize().unwrap(),
        );
        for (budget, edges) in [
            ((0, 1024, 1024), vec![]),
            ((1, 0, 1024), vec![]),
            ((1, 1024, 0), vec![]),
            ((1, 1024, 1024), vec![loop_edge]),
        ] {
            let providers = AssertionProviders::collect_with_budget(
                std::slice::from_ref(&file),
                &edges,
                &["./steps".into()],
                budget,
            )
            .unwrap();
            assert!(providers.incomplete());
            session
                .state::<crate::ruby::RubySession>()
                .unwrap()
                .assertions = providers;
            let mut actual = definitions.clone();
            let finalized = session
                .finalize(std::slice::from_ref(&file), &mut actual)
                .unwrap();
            assert_eq!(finalized.uncertainties.len(), 1);
            let result = crate::analysis::analyze_with_diagnostics(actual, vec![], &config)
                .unwrap()
                .result;
            assert!(result
                .findings
                .iter()
                .any(|finding| finding.rule == Rule::DuplicateMatcher));
            assert!(result
                .findings
                .iter()
                .any(|finding| finding.rule == Rule::DuplicateHandler));
        }
    }

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
                files[1].path.canonicalize().unwrap(),
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
                .unwrap()
                .factories
                .is_empty());
        }
        for provider in ["module Assertions; end", "module Outer::Assertions; end"] {
            std::fs::write(&files[1].path, provider).unwrap();
            assert!(collect(std::slice::from_ref(&valid), 2000)
                .get(&files[0].path, source)
                .unwrap()
                .factories
                .is_empty());
        }
    }

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
            files[1].path.canonicalize().unwrap(),
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
            AssertionProviders::from_units(&units, &edges, &["./assertions".into()], 2000).unwrap();
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
                false,
            ),
        ] {
            let source = "require_relative 'assertions'\nGiven('one') { Assertions.expect(page).to_be(UNKNOWN) }; Then('two') { Assertions.expect(page).to_be(UNKNOWN) }; Given('ordinary one') { work() }; Then('ordinary two') { work() }; Given('block one') { Assertions.expect(page) { first() }.to_be(true) }; Then('block two') { Assertions.expect(page) { second() }.to_be(true) }; Given('dispatch one') { Assertions.expect(page).to_be(true) }; Then('dispatch two') { Assertions&.expect(page).to_be(true) }";
            std::fs::write(&files[0].path, source).unwrap();
            std::fs::write(&files[1].path, provider).unwrap();
            let edges = [SourceDependency::new(
                crate::model::SourceLocation::new(files[0].path.clone(), 1, 1, 1, 30),
                files[1].path.canonicalize().unwrap(),
            )];
            let mut session =
                SourceExtractionSession::with_options(root.path(), &[], &["./assertions".into()])
                    .with_dependencies(&edges);
            let adapter = crate::source_adapter::adapter_for_language(SourceLanguage::Ruby);
            adapter.prepare_session(&files, &mut session).unwrap();
            let (definitions, finalized) = finish(source, &files, &mut session);
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
    fn finish(
        source: &str,
        files: &[SourceFile],
        session: &mut SourceExtractionSession,
    ) -> (
        Vec<crate::model::StepDefinition>,
        crate::source_adapter::SourceFinalization,
    ) {
        let mut definitions = crate::source_adapter::adapter_for_language(SourceLanguage::Ruby)
            .extract_with_session(source, &files[0], session)
            .unwrap()
            .definitions;
        let finalized = session.finalize(files, &mut definitions).unwrap();
        (definitions, finalized)
    }

    fn provider_files(root: &Path) -> Vec<SourceFile> {
        ["steps.rb", "assertions.rb"]
            .map(|name| SourceFile {
                path: root.join(name),
                language: SourceLanguage::Ruby,
            })
            .into()
    }

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

    #[test]
    fn unavailable_snapshot_preserves_ordinary_findings_and_configured_uncertainty() {
        let root = tempfile::tempdir().unwrap();
        let files = provider_files(root.path());
        let config = crate::config::Config::load(root.path(), Default::default()).unwrap();
        let source = "Given('one') { work() }; Then('two') { work() }";
        let adapter = crate::source_adapter::adapter_for_language(SourceLanguage::Ruby);
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
}
