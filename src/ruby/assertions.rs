//! Explicit provider provenance and value-sensitive assertion evidence, without execution.

use super::{descendants, literal_string, location, text};
use crate::model::BehaviorEvent;

#[cfg(test)]
use crate::source_adapter::SourceFile;
use crate::source_adapter::{SourceAdvisory, SourceDependency};
#[cfg(test)]
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tree_sitter::Node;

mod origins;

/// Unwraps bounded literal dispatch into its method and effective arguments.
fn call_parts<'a>(node: Node<'a>, source: &str) -> Option<(String, Vec<Node<'a>>)> {
    let mut method = text(node.child_by_field_name("method")?, source).to_owned();
    let mut cursor = node.walk();
    let mut args: Vec<_> = node
        .child_by_field_name("arguments")
        .map(|arguments| {
            arguments
                .named_children(&mut cursor)
                .filter(|node| !node.is_extra())
                .collect()
        })
        .unwrap_or_default();
    for _ in 0..16 {
        if !matches!(method.as_str(), "send" | "public_send" | "__send__") {
            return Some((method, args));
        }
        let first = args.first().copied()?;
        method = super::static_method_name(first, source)?;
        args.remove(0);
    }
    None
}

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
    origins: origins::Origins,
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
        Ok(Self::from_units(&loaded, edges, configured, budget.2))
    }

    pub(super) fn from_units(
        loaded: &[super::providers::Unit],
        edges: &[SourceDependency],
        configured: &[String],
        work_budget: usize,
    ) -> Self {
        Self::from_units_with_registrations(loaded, edges, configured, work_budget, None)
    }

    /// Builds assertion proofs from loaded sources and independent registration evidence.
    pub(super) fn from_units_with_registrations(
        loaded: &[super::providers::Unit],
        edges: &[SourceDependency],
        configured: &[String],
        work_budget: usize,
        registrations: Option<&super::providers::Providers>,
    ) -> Self {
        let Some(mut origins) =
            origins::Origins::collect(loaded, edges, configured, registrations, work_budget)
        else {
            return if configured.is_empty() {
                Self::default()
            } else {
                Self::unavailable()
            };
        };
        let publish_origins = |mut origins: BTreeMap<PathBuf, origins::Origins>| Self {
            proofs: loaded
                .iter()
                .map(|unit| {
                    let origins = origins.remove(&unit.file.path).unwrap_or_default();
                    let factories = origins
                        .factories
                        .iter()
                        .filter(|(_, callable)| callable.trusted && !callable.denied)
                        .map(|(position, _)| *position)
                        .collect();
                    (
                        unit.file.path.clone(),
                        (
                            unit.source.clone(),
                            AssertionBindings { factories, origins },
                        ),
                    )
                })
                .collect(),
            ..Self::default()
        };
        if configured.is_empty() {
            return publish_origins(origins);
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
            return Self::unavailable();
        }
        // A configured JS/TS-only module cannot make an unrelated Ruby graph incomplete.
        work = work.saturating_add(total_nodes);
        if work > work_budget {
            return Self::unavailable();
        }
        if !units.iter().zip(&nodes).any(|((_, source, tree), nodes)| {
            nodes.iter().any(|node| {
                node.parent() == Some(tree.root_node())
                    && configured_load(*node, source, configured)
            })
        }) {
            return publish_origins(origins);
        }
        if units
            .iter()
            .any(|(_, _, tree)| tree.root_node().has_error())
        {
            return Self::unavailable();
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
            return Self::unavailable();
        }
        let mut result = Self::default();
        for (unit, (file, source, tree)) in units.iter().enumerate() {
            let root = tree.root_node();
            let resolved = origins.remove(&file.path).unwrap_or_default();
            let factories = resolved
                .factories
                .iter()
                .filter(|(_, callable)| callable.trusted && !callable.denied)
                .map(|(position, _)| *position)
                .collect::<BTreeSet<_>>();
            for edge in edges.iter().filter(|edge| edge.location.path == file.path) {
                work = work.saturating_add(nodes[unit].len());
                if work > work_budget {
                    return Self::unavailable();
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
                    return Self::unavailable();
                }
                let mut trusted = resolved.factories.values().any(|callable| {
                    callable.trusted
                        && !callable.denied
                        && nodes[provider].iter().any(|node| {
                            node.kind() == "module"
                                && node.child_by_field_name("name").is_some_and(|name| {
                                    text(name, units[provider].1) == callable.owner
                                })
                        })
                });
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
                        return Self::unavailable();
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
                (
                    source.to_string(),
                    AssertionBindings {
                        factories,
                        origins: resolved,
                    },
                ),
            );
        }
        result
    }

    pub(super) fn advisories(&self) -> &[SourceAdvisory] {
        &self.advisories
    }

    /// Returns a proof only when its captured source still matches the current file.
    pub fn get(&self, file: &Path, source: &str) -> Option<AssertionBindings> {
        self.proofs
            .get(file)
            .filter(|(input, bindings)| {
                input == source
                    && (!bindings.factories.is_empty()
                        || !bindings.origins.factories.is_empty()
                        || !bindings.origins.captures.is_empty()
                        || !bindings.origins.dispatch.is_empty()
                        || !bindings.origins.rejected.is_empty())
            })
            .map(|(_, bindings)| bindings.clone())
    }
}

/// Recognizes a literal source load explicitly listed in assertion configuration.
fn configured_load(node: Node<'_>, source: &str, configured: &[String]) -> bool {
    node.kind() == "call"
        && node.child_by_field_name("receiver").is_none()
        && node
            .child_by_field_name("method")
            .is_some_and(|method| matches!(text(method, source), "require_relative" | "require"))
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

/// Accepts factory references only at declarations and supported non-mutating calls.
fn closed_reference(node: Node<'_>, source: &str) -> bool {
    node.parent().is_some_and(|parent| {
        (parent.kind() == "module" && parent.child_by_field_name("name") == Some(node))
            || (parent.kind() == "call"
                && !std::iter::successors(Some(parent), |node| node.parent()).any(|node| {
                    matches!(node.kind(), "assignment" | "operator_assignment")
                        && node.child_by_field_name("left").is_some_and(|left| {
                            left.start_byte() <= parent.start_byte()
                                && left.end_byte() >= parent.end_byte()
                        })
                })
                && parent.child_by_field_name("receiver") == Some(node)
                && parent
                    .child_by_field_name("method")
                    .is_some_and(|method| text(method, source) == "expect"))
    })
}

impl AssertionBindings {
    /// Identifies injected fields backed by closed callable origins.
    pub(super) fn known_field(&self, node: Node<'_>) -> bool {
        node.kind() == "instance_variable"
            && node.parent().is_some_and(|call| {
                call.child_by_field_name("receiver") == Some(node)
                    && self
                        .factories
                        .contains(&(call.start_byte(), call.end_byte()))
            })
    }
    /// Yields dispatch sites whose source origins prove handler isolation.
    pub(super) fn isolated_dispatch(&self) -> impl Iterator<Item = usize> + '_ {
        self.origins.dispatch.iter().map(|(start, _)| *start)
    }
    /// Detects handlers whose known assertion origins cannot support comparison.
    pub(super) fn rejected_comparison(
        &self,
        block: Node<'_>,
        root: Node<'_>,
        source: &str,
    ) -> bool {
        self.known_ineligible(block, root, source)
            && !descendants(block).iter().any(|node| {
                (block.kind() == "method"
                    && matches!(
                        node.kind(),
                        "self" | "instance_variable" | "class_variable" | "global_variable"
                    )
                    && !self.known_field(*node))
                    || (node.id() != block.id()
                        && matches!(
                            node.kind(),
                            "method"
                                | "singleton_method"
                                | "class"
                                | "module"
                                | "singleton_class"
                                | "heredoc_body"
                                | "heredoc_beginning"
                                | "optional_parameter"
                                | "keyword_parameter"
                                | "ERROR"
                        ))
            })
    }
    /// Rejects optional comparison for known unsupported origins without hiding discovery errors.
    pub(super) fn known_ineligible(&self, block: Node<'_>, root: Node<'_>, source: &str) -> bool {
        let bindings = self.bindings(block, root, source);
        !bindings.uncertain
            && (self
                .events(block, source, &bindings)
                .iter()
                .any(BehaviorEvent::is_unresolved_assertion)
                || descendants(block).iter().any(|node| {
                    self.origins
                        .rejected
                        .contains(&(node.start_byte(), node.end_byte()))
                        || bindings.names.get(&node.id()).is_some_and(|name| {
                            name.strip_prefix("capture:")
                                .and_then(|name| self.origins.captures.get(name))
                                .is_some_and(|callable| callable.denied)
                        })
                }))
    }
    /// Binds handler names using only independently established callable origins.
    pub(super) fn bindings(
        &self,
        block: Node<'_>,
        root: Node<'_>,
        source: &str,
    ) -> super::bindings::Bindings {
        let captures = self
            .origins
            .captures
            .iter()
            .map(|(name, callable)| (name.clone(), format!("ruby:callable:{}", callable.owner)))
            .collect();
        super::bindings::Bindings::collect_with_origins(
            block,
            root,
            source,
            &captures,
            &self.origins.dispatch,
        )
    }

    /// Preserves literal parameterization for closed ordinary-call handlers.
    pub(super) fn ordinary_structural(
        &self,
        block: Node<'_>,
        source: &str,
        bindings: &super::bindings::Bindings,
    ) -> Option<String> {
        if block
            .child_by_field_name("parameters")
            .is_some_and(|parameters| parameters.named_child_count() != 0)
        {
            return None;
        }
        let body = block.child_by_field_name("body")?;
        let mut cursor = body.walk();
        let statements: Vec<_> = body
            .named_children(&mut cursor)
            .filter(|node| !node.is_extra())
            .collect();
        let mut shapes = Vec::new();
        let mut literals = 0;
        for statement in statements {
            if statement.kind() != "call" || statement.child_by_field_name("block").is_some() {
                return None;
            }
            let receiver = statement.child_by_field_name("receiver")?;
            let callable = self
                .origins
                .factories
                .get(&(receiver.start_byte(), receiver.end_byte()))?;
            if callable.trusted || callable.denied {
                return None;
            }
            let args = statement.child_by_field_name("arguments")?;
            let mut cursor = args.walk();
            let mut arguments = Vec::new();
            for value in args
                .named_children(&mut cursor)
                .filter(|node| !node.is_extra())
            {
                if !matches!(value.kind(), "integer" | "float" | "string")
                    || (value.kind() == "string"
                        && (descendants(value)
                            .iter()
                            .any(|part| part.kind() == "interpolation")
                            || literal_string(text(value, source)).is_none()))
                {
                    return None;
                }
                arguments.push(value.kind());
                literals += 1;
            }
            shapes.push((
                super::handler::syntax_tokens(receiver, source, bindings).1,
                text(statement.child_by_field_name("method")?, source),
                statement
                    .child_by_field_name("operator")
                    .map(|operator| text(operator, source)),
                arguments,
            ));
        }
        (literals != 0)
            .then(|| serde_json::to_string(&shapes).expect("ordinary call shapes serialize"))
    }

    /// Collects ordered assertions and complete ordinary effects; uncertainty withdraws comparison.
    /// A declared callable runs only where a proven invocation expands it, one level deep.
    pub fn events(
        &self,
        block: Node<'_>,
        source: &str,
        bindings: &super::bindings::Bindings,
    ) -> Vec<BehaviorEvent> {
        if let Some(shape) = self.ordinary_structural(block, source, bindings) {
            return vec![BehaviorEvent::Call(format!("ruby:ordinary:{shape}"))];
        }
        let mut events = Vec::new();
        let declared = super::bindings::declared_callables(block, bindings);
        let mut pending = vec![(block, false, false, false)];
        while let Some((node, deferred, covered, expanded)) = pending.pop() {
            if block.kind() == "method" && block.child_by_field_name("name") == Some(node) {
                continue;
            }
            // A declared callable body is behaviour only where its invocation is proven.
            if declared.contains_key(&node.id()) {
                continue;
            }
            if let Some(statements) =
                super::bindings::expansion_statements(bindings, &declared, node, expanded)
            {
                // The declaration executes here; record its body, not a wrapper-call event.
                // One level only: an invocation inside the expanded body is an ordinary call.
                pending.extend(
                    statements
                        .into_iter()
                        .rev()
                        .map(|statement| (statement, deferred, false, true)),
                );
                continue;
            }
            if bindings.unresolved_invocations.contains(&node.id()) {
                // Arguments are not substituted, so the declared body's values are unproven.
                events.push(BehaviorEvent::unresolved_assertion(deferred));
                // A parameterized invocation always carries arguments; they execute now.
                let values = node
                    .child_by_field_name("arguments")
                    .map(super::bindings::named_children)
                    .unwrap_or_default();
                pending.extend(
                    values
                        .into_iter()
                        .rev()
                        .map(|value| (value, deferred, false, expanded)),
                );
                continue;
            }
            let deferred = deferred
                || (node.id() != block.id()
                    && matches!(node.kind(), "block" | "do_block" | "lambda")
                    && !node
                        .parent()
                        .is_some_and(|parent| parent.kind() == "lambda")
                    && !immediately_invoked(node, source));
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
            let opaque = matches!(
                node.kind(),
                "assignment"
                    | "operator_assignment"
                    | "if"
                    | "unless"
                    | "case"
                    | "while"
                    | "until"
                    | "for"
                    | "rescue"
                    | "ensure"
                    | "return"
                    | "yield"
                    | "binary"
                    | "unary"
            ) || (!node.is_extra()
                && node.kind() != "call"
                && node.parent().is_some_and(|parent| {
                    matches!(parent.kind(), "block_body" | "body_statement")
                }));
            if opaque && !covered {
                events.push(BehaviorEvent::Call(format!(
                    "ruby:effect:{}",
                    serialize_tokens(node, source, bindings)
                )));
            }
            let mut operation = opaque;
            if node.kind() == "call" {
                operation = true;
                if !covered {
                    events.push(BehaviorEvent::Call(format!(
                        "ruby:call:{}",
                        serialize_tokens(node, source, bindings)
                    )));
                }
            } else if !covered && node.kind() == "identifier" && bindings.calls.contains(&node.id())
            {
                events.push(BehaviorEvent::Call(format!(
                    "ruby:bare:{}",
                    text(node, source)
                )));
            }
            pending.extend(
                super::bindings::named_children(node)
                    .into_iter()
                    .rev()
                    .map(|child| (child, deferred, covered || operation, expanded)),
            );
        }
        // Ordinary effects cannot outvote a contradictory or unresolved assertion.
        let assertions: Vec<_> = events
            .iter()
            .filter_map(|event| {
                if let BehaviorEvent::Assertion { deferred, payload } = event {
                    Some((*deferred, payload.as_str()))
                } else {
                    None
                }
            })
            .collect();
        if assertions.is_empty() {
            if let Some(shape) = super::handler::parameterized_calls(block, source, bindings) {
                return vec![BehaviorEvent::Call(format!("ruby:ordinary:{shape}"))];
            }
        }
        let context = serde_json::to_string(&assertions).expect("assertion context serializes");
        for event in &mut events {
            if let BehaviorEvent::Call(payload) = event {
                payload.push_str(&context);
            }
        }
        events
    }

    /// Builds value-sensitive assertion evidence from a trusted factory and a literal, resolved
    /// local or resolved constant terminal.
    fn assertion(
        &self,
        node: Node<'_>,
        source: &str,
        bindings: &super::bindings::Bindings,
    ) -> Option<BehaviorEvent> {
        if node.kind() != "call" || node.child_by_field_name("block").is_some() {
            return None;
        }
        let (method, expected) = call_parts(node, source)?;
        // Configured factory contract; ordinary same-named objects never enter this path.
        if !matches!(
            method.as_str(),
            "to_be" | "to_equal" | "equal_to" | "to_have_class" | "to_have_value" | "to_be_visible"
        ) {
            return None;
        }
        let mut receiver = node.child_by_field_name("receiver")?;
        let mut modifiers = Vec::new();
        let mut conditional_dispatch = Vec::new();
        if node
            .child_by_field_name("operator")
            .is_some_and(|operator| text(operator, source) == "&.")
        {
            conditional_dispatch.push(0);
        }
        for _ in 0..16 {
            if self
                .factories
                .contains(&(receiver.start_byte(), receiver.end_byte()))
            {
                break;
            }
            if receiver.kind() != "call" || receiver.child_by_field_name("block").is_some() {
                return None;
            }
            let (modifier, args) = call_parts(receiver, source)?;
            if !args.is_empty() || !matches!(modifier.as_str(), "not" | "to") {
                return None;
            }
            if receiver
                .child_by_field_name("operator")
                .is_some_and(|operator| text(operator, source) == "&.")
            {
                conditional_dispatch.push(modifiers.len() + 1);
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
        let Some((_, subjects)) = call_parts(receiver, source) else {
            return Some(BehaviorEvent::unresolved_assertion(false));
        };
        if subjects.len() != 1 {
            return Some(BehaviorEvent::unresolved_assertion(false));
        }
        let arity = if method == "to_be_visible" { 0 } else { 1 };
        if expected.len() != arity {
            return Some(BehaviorEvent::unresolved_assertion(false));
        }
        let Some(expected) = expected
            .into_iter()
            .map(|value| expected_value(value, node, bindings))
            .collect::<Option<Vec<_>>>()
        else {
            return Some(BehaviorEvent::unresolved_assertion(false));
        };
        if expected.iter().any(|args| {
            if self
                .origins
                .matchers
                .contains(&(args.start_byte(), args.end_byte()))
            {
                return args
                    .child_by_field_name("arguments")
                    .is_none_or(|arguments| {
                        descendants(arguments).iter().any(|value| {
                            !value.is_extra()
                                && !matches!(
                                    value.kind(),
                                    "argument_list"
                                        | "hash"
                                        | "pair"
                                        | "hash_key_symbol"
                                        | "string"
                                        | "string_content"
                                        | "escape_sequence"
                                        | "simple_symbol"
                                        | "integer"
                                        | "float"
                                        | "true"
                                        | "false"
                                        | "nil"
                                        | "array"
                                )
                        })
                    });
            }
            if expected_access(*args, source, bindings) {
                return false;
            }
            descendants(*args).iter().any(|value| {
                !value.is_extra()
                    && !(matches!(value.kind(), "identifier" | "constant")
                        && bindings.names.contains_key(&value.id()))
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
                            | "regex"
                            | "regex_content"
                    )
            })
        }) {
            return Some(BehaviorEvent::unresolved_assertion(false));
        }
        if self
            .origins
            .factories
            .get(&(receiver.start_byte(), receiver.end_byte()))
            .is_some_and(|callable| callable.negated)
        {
            modifiers.push("not".to_owned());
        }
        let qualifier = std::iter::once("expect")
            .map(str::to_owned)
            .chain(modifiers.into_iter().rev())
            .collect::<Vec<_>>()
            .join(".");
        let serialize = |root: Node<'_>| serialize_tokens(root, source, bindings);
        let callable = self
            .origins
            .factories
            .get(&(receiver.start_byte(), receiver.end_byte()))?;
        let (_, arguments) = call_parts(receiver, source)?;
        let mut subject = serde_json::to_string(&(
            &callable.owner,
            arguments.into_iter().map(serialize).collect::<Vec<_>>(),
            receiver
                .child_by_field_name("operator")
                .map(|operator| text(operator, source)),
            receiver.child_by_field_name("block").map(serialize),
        ))
        .expect("factory subjects serialize");
        if !conditional_dispatch.is_empty() {
            subject = format!("ruby:conditional:{conditional_dispatch:?}:{subject}");
        }
        Some(BehaviorEvent::assertion(
            &qualifier,
            &method,
            // A proved factory can still carry a block or conditional dispatch.
            &subject,
            &expected.first().copied().map(serialize).unwrap_or_default(),
        ))
    }
}

/// Distinguishes executed closure bodies from deferred callables.
fn immediately_invoked(scope: Node<'_>, source: &str) -> bool {
    let mut value = scope;
    if scope.kind() != "lambda" {
        let Some(constructor) = scope.parent().filter(|node| {
            node.kind() == "call"
                && node.child_by_field_name("block") == Some(scope)
                && node.child_by_field_name("receiver").is_none()
                && node
                    .child_by_field_name("method")
                    .is_some_and(|method| matches!(text(method, source), "proc" | "lambda"))
        }) else {
            return false;
        };
        value = constructor;
    }
    while let Some(parent) = value
        .parent()
        .filter(|node| node.kind() == "parenthesized_statements" && node.named_child_count() == 1)
    {
        value = parent;
    }
    value.parent().is_some_and(|call| {
        call.kind() == "call"
            && call.child_by_field_name("receiver") == Some(value)
            && call
                .child_by_field_name("method")
                .is_some_and(|method| text(method, source) == "call")
            && call.child_by_field_name("block").is_none()
            && call
                .child_by_field_name("arguments")
                .is_none_or(|args| semantic_arity(args) == 0)
    })
}

/// Detects unresolved captures inside assertion expected-value expressions.
fn expected_access(node: Node<'_>, source: &str, bindings: &super::bindings::Bindings) -> bool {
    if node.kind() == "identifier" {
        return bindings.names.contains_key(&node.id());
    }
    node.kind() == "call"
        && node.child_by_field_name("block").is_none()
        && node
            .child_by_field_name("arguments")
            .is_none_or(|args| semantic_arity(args) == 0)
        && node.child_by_field_name("method").is_some_and(|method| {
            method.kind() == "identifier"
                && !matches!(text(method, source), "send" | "public_send" | "__send__")
        })
        && node
            .child_by_field_name("receiver")
            .is_some_and(|receiver| expected_access(receiver, source, bindings))
}

/// Serializes binding-aware tokens for assertion identity; declared callable bodies are placeholders.
fn serialize_tokens(node: Node<'_>, source: &str, bindings: &super::bindings::Bindings) -> String {
    serde_json::to_string(&super::handler::event_tokens(node, source, bindings))
        .expect("assertion tokens serialize")
}

/// Resolves an expected local only after a unique preceding write in its scope.
fn expected_value<'a>(
    value: Node<'a>,
    call: Node<'a>,
    bindings: &super::bindings::Bindings,
) -> Option<Node<'a>> {
    if value.kind() != "identifier" {
        return Some(value);
    }
    let name = bindings.names.get(&value.id())?;
    if !name.starts_with("local:") {
        return None;
    }
    let scope = std::iter::successors(call.parent(), |node| node.parent())
        .find(|node| matches!(node.kind(), "block" | "do_block" | "lambda" | "method"))?;
    let writes: Vec<_> = descendants(scope)
        .into_iter()
        .filter(|node| {
            matches!(node.kind(), "assignment" | "operator_assignment")
                && node
                    .child_by_field_name("left")
                    .is_some_and(|left| bindings.names.get(&left.id()) == Some(name))
        })
        .collect();
    if writes.is_empty() {
        return Some(value);
    }
    if writes.len() != 1
        || writes[0].start_byte() >= call.start_byte()
        || writes[0].parent() != scope.child_by_field_name("body")
    {
        return None;
    }
    let right = writes[0].child_by_field_name("right")?;
    matches!(
        right.kind(),
        "string" | "integer" | "float" | "simple_symbol" | "true" | "false" | "nil"
    )
    .then_some(right)
}

/// Counts effective arguments while excluding comments and dispatch selectors.
fn semantic_arity(node: Node<'_>) -> usize {
    super::bindings::semantic_children(node).len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rule;
    use crate::source_adapter::{SourceExtractionSession, SourceLanguage};

    /// Checks that optional proof failures retain ordinary findings and correct completeness.
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
        let loaded = super::super::providers::load_units(std::slice::from_ref(&file), (1, 4096))
            .unwrap()
            .unwrap();
        let loop_edge = SourceDependency::new(
            definitions[0].location.clone(),
            file.path.canonicalize().unwrap(),
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
                .is_none_or(|bindings| bindings.factories.is_empty()));
        }
        for provider in ["module Assertions; end", "module Outer::Assertions; end"] {
            std::fs::write(&files[1].path, provider).unwrap();
            assert!(collect(std::slice::from_ref(&valid), 2000)
                .get(&files[0].path, source)
                .is_none_or(|bindings| bindings.factories.is_empty()));
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
