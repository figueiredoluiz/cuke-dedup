//! Explicit provider provenance and value-sensitive assertion evidence, without execution.

use super::{descendants, literal_string, location, text};
use crate::model::BehaviorEvent;
use crate::resource_limits::{MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES};
use crate::source_adapter::{SourceDependency, SourceFile};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tree_sitter::Node;

#[derive(Default)]
pub(super) struct AssertionProviders {
    proofs: BTreeMap<PathBuf, (String, AssertionBindings)>,
    incomplete: bool,
}

#[derive(Clone, Default)]
pub(super) struct AssertionBindings {
    // Source offsets bind trust to a preceding resolved load and a closed constant identity.
    factories: BTreeSet<(usize, usize)>,
}

impl AssertionProviders {
    fn unavailable() -> Self {
        Self {
            incomplete: true,
            ..Self::default()
        }
    }
    pub fn incomplete(&self) -> bool {
        self.incomplete
    }
    pub fn collect(
        files: &[SourceFile],
        edges: &[SourceDependency],
        configured: &[String],
    ) -> Result<Self> {
        Self::collect_with_budget(
            files,
            edges,
            configured,
            (
                MAX_REGISTRATION_MODULES,
                MAX_REGISTRATION_MODULE_BYTES,
                1_000_000,
            ),
        )
    }

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
        let canonical: Vec<_> = loaded
            .iter()
            .map(|unit| unit.canonical_path.clone())
            .collect();
        let units: Vec<_> = loaded
            .into_iter()
            .map(|unit| (unit.file, unit.source, unit.tree))
            .collect();
        let nodes: Vec<_> = units
            .iter()
            .map(|(_, _, tree)| descendants(tree.root_node()))
            .collect();
        let total_nodes: usize = nodes.iter().map(Vec::len).sum();
        let mut work = total_nodes.saturating_mul(2);
        if work > budget.2 {
            return Ok(Self::unavailable());
        }
        // A configured JS/TS-only module cannot make an unrelated Ruby graph incomplete.
        work = work.saturating_add(total_nodes);
        if work > budget.2 {
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
                if work > budget.2 {
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
                if work > budget.2 {
                    return Ok(Self::unavailable());
                }
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
                    if work > budget.2 {
                        return Ok(Self::unavailable());
                    }
                    // Any escape, alias, replacement, or unknown member use removes trust globally.
                    if !units.iter().zip(&nodes).all(|((_, input, _), nodes)| {
                        !nodes.iter().any(|node| reflective_call(*node, input))
                            && nodes
                                .iter()
                                .filter(|node| {
                                    node.kind() == "constant" && text(**node, input) == name
                                })
                                .all(|node| closed_reference(*node, input))
                    }) {
                        continue;
                    }
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
            }
            result.proofs.insert(
                file.path.clone(),
                (source.clone(), AssertionBindings { factories }),
            );
        }
        Ok(result)
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
            &serialize(subjects),
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
}
