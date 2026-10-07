//! Closed constant registration exports over resolved, preceding source loads.
use super::registration_aliases::{closed_call, identifier_references};
use super::{descendants, registration, static_method_name, text};
use crate::resource_limits::{read_utf8, MAX_PROJECT_INPUT_BYTES};
#[cfg(test)]
use crate::resource_limits::{MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES};
use crate::source_adapter::{SourceDependency, SourceFile, SourceLanguage};
use anyhow::{Context, Result};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Tree};

#[derive(Default)]
pub(super) struct Proof {
    source: String,
    pub captures: BTreeSet<usize>,
    pub calls: BTreeMap<usize, String>,
    pub forwarding: BTreeSet<usize>,
    pub isolated_calls: BTreeSet<usize>,
    pub handler_captures: BTreeSet<usize>,
    pub isolated_constants: BTreeSet<usize>,
    pub unresolved_calls: BTreeSet<usize>,
    pub handlers: BTreeMap<usize, crate::model::HandlerFingerprint>,
    pub comparison_rejections: BTreeSet<usize>,
    pub executed: BTreeSet<usize>,
}

#[derive(Default)]
pub(super) struct Providers(BTreeMap<PathBuf, Proof>, bool);

pub(super) struct Unit {
    pub(super) file: SourceFile,
    pub(super) canonical_path: PathBuf,
    pub(super) source: String,
    pub(super) tree: Tree,
}

mod execution;
mod exports;
mod handlers;
mod owners;

struct Definition<'a> {
    unit: usize,
    assignment: Node<'a>,
    value: Node<'a>,
}

type Position = (usize, usize, usize);

struct Graph<'a> {
    units: &'a [Unit],
    edges: &'a [SourceDependency],
    definitions: BTreeMap<String, Vec<Definition<'a>>>,
    namespaces: BTreeMap<String, usize>,
    protected_modules: BTreeSet<usize>,
    forwarders: BTreeMap<(String, String), String>,
    custom_allocation: bool,
    root_modules: BTreeMap<String, (usize, Node<'a>)>,
    classes: BTreeMap<String, Vec<(usize, Node<'a>)>>,
    unknown_loads: bool,
    loads: BTreeSet<Position>,
    unit_ids: BTreeMap<PathBuf, usize>,
    available_proofs: RefCell<BTreeSet<(Position, Position)>>,
    load_work: Cell<usize>,
    proof_exhausted: Cell<bool>,
    proof_work: Cell<usize>,
    #[cfg(test)]
    proof_limit: usize,
    outgoing: BTreeMap<usize, Vec<(usize, Position)>>,
    incoming: BTreeMap<usize, Vec<(Position, bool)>>,
}

/// Proof sources for a selection, or why proofs are unavailable.
pub(super) enum ProofSources {
    /// Every selected Ruby file was read and parsed within the budget.
    Loaded(Vec<Unit>),
    /// The selection exceeds the `(files, bytes)` proof budget.
    OverBudget,
    /// A selected file could not be read or canonicalized; extraction reports it.
    Unreadable,
}

/// Test entry point over [`load_proof_sources`] that keeps only the loaded units.
#[cfg(test)]
pub(super) fn load_units(
    files: &[SourceFile],
    budget: (usize, usize),
) -> Result<Option<Vec<Unit>>> {
    Ok(match load_proof_sources(files, budget)? {
        ProofSources::Loaded(units) => Some(units),
        ProofSources::OverBudget | ProofSources::Unreadable => None,
    })
}

/// Loads proof sources in selection order through the bounded read/parse/canonicalization
/// boundary both proof collectors share; the first condition met decides the outcome, so a
/// read failure before the budget runs out is never reported as the budget.
pub(super) fn load_proof_sources(
    files: &[SourceFile],
    budget: (usize, usize),
) -> Result<ProofSources> {
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_ruby::LANGUAGE.into())?;
    let mut units = Vec::new();
    let mut bytes = 0;
    for file in files.iter().filter(|f| f.language == SourceLanguage::Ruby) {
        if units.len() >= budget.0 {
            return Ok(ProofSources::OverBudget);
        }
        let Ok(source) = read_utf8(&file.path, "Ruby provider source", MAX_PROJECT_INPUT_BYTES)
        else {
            // Extraction reports per-file failures and preserves trustworthy neighbors.
            return Ok(ProofSources::Unreadable);
        };
        bytes += source.len();
        if bytes > budget.1 {
            return Ok(ProofSources::OverBudget);
        }
        let tree = parser
            .parse(&source, None)
            .context("Ruby provider parser returned no tree")?;
        let Ok(canonical_path) = file.path.canonicalize() else {
            return Ok(ProofSources::Unreadable);
        };
        units.push(Unit {
            file: file.clone(),
            canonical_path,
            source,
            tree,
        });
    }
    Ok(ProofSources::Loaded(units))
}

impl Providers {
    /// Builds bounded optional proofs; extraction remains authoritative when preparation is unavailable.
    #[cfg(test)]
    pub fn collect(files: &[SourceFile], edges: &[SourceDependency]) -> Result<Self> {
        Self::collect_with_budget(
            files,
            edges,
            (MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES),
        )
    }

    #[cfg(test)]
    fn collect_with_budget(
        files: &[SourceFile],
        edges: &[SourceDependency],
        (max_files, max_bytes): (usize, usize),
    ) -> Result<Self> {
        let Some(units) = load_units(files, (max_files, max_bytes))? else {
            return Ok(Self::default());
        };
        Self::from_units(&units, edges)
    }

    #[cfg(test)]
    pub(super) fn from_units(units: &[Unit], edges: &[SourceDependency]) -> Result<Self> {
        Ok(Self::from_units_with_assertions(units, edges, &[]).0)
    }

    pub(super) fn from_units_with_assertions(
        units: &[Unit],
        edges: &[SourceDependency],
        configured: &[String],
    ) -> (Self, super::assertions::AssertionProviders) {
        Self::collect_proofs(units, configured, Graph::new(units, edges))
    }

    /// Collects bounded registration proofs using assertion origins only for independent isolation.
    fn collect_proofs<'a>(
        units: &'a [Unit],
        configured: &[String],
        mut graph: Graph<'a>,
    ) -> (Self, super::assertions::AssertionProviders) {
        let edges = graph.edges;
        let without_registration_proof = |providers| {
            (
                providers,
                super::assertions::AssertionProviders::from_units(
                    units,
                    edges,
                    configured,
                    crate::resource_limits::MAX_ASSERTION_RESOLUTION_WORK,
                ),
            )
        };
        for (unit, input) in units.iter().enumerate() {
            graph
                .unit_ids
                .entry(input.file.path.clone())
                .or_insert(unit);
            graph
                .unit_ids
                .entry(input.canonical_path.clone())
                .or_insert(unit);
            for node in descendants(input.tree.root_node()) {
                graph.custom_allocation |= allocation_override(node, &input.source);
                if matches!(node.kind(), "method" | "singleton_method")
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|n| super::protected_method(text(n, &input.source)))
                {
                    let mut parent = node.parent();
                    while let Some(scope) = parent {
                        if scope.kind() == "module" {
                            graph.protected_modules.insert(scope.id());
                        }
                        parent = scope.parent();
                    }
                }
                if node.kind() == "call"
                    && node.parent() == Some(input.tree.root_node())
                    && node.child_by_field_name("receiver").is_none()
                    && node.child_by_field_name("method").is_some_and(|name| {
                        matches!(text(name, &input.source), "require" | "require_relative")
                    })
                {
                    graph.loads.insert(graph.position(unit, node, false));
                }
                if matches!(node.kind(), "module" | "class") {
                    if let Some(key) = node
                        .child_by_field_name("name")
                        .and_then(|name| qualified(name, &input.source))
                    {
                        *graph.namespaces.entry(key.clone()).or_default() += 1;
                        if node.kind() == "class" {
                            graph
                                .classes
                                .entry(key.clone())
                                .or_default()
                                .push((unit, node));
                        }
                        if node.kind() == "module" && node.parent() == Some(input.tree.root_node())
                        {
                            graph.root_modules.insert(key, (unit, node));
                        }
                    }
                }
                if matches!(node.kind(), "assignment" | "operator_assignment") {
                    if let (Some(left), Some(value)) = (
                        node.child_by_field_name("left"),
                        node.child_by_field_name("right"),
                    ) {
                        if let Some(key) = qualified(left, &input.source) {
                            graph.definitions.entry(key).or_default().push(Definition {
                                unit,
                                assignment: node,
                                value,
                            });
                        }
                    }
                }
            }
        }
        for edge in edges {
            if let (Some(&from), Some(&to)) = (
                graph.unit_ids.get(&edge.location.path),
                graph.unit_ids.get(&edge.target),
            ) {
                let position = (from, edge.location.line, edge.location.column);
                graph.outgoing.entry(from).or_default().push((to, position));
                graph
                    .incoming
                    .entry(to)
                    .or_default()
                    .push((position, edge.dependency_only));
            }
        }
        graph.unknown_loads = units.iter().enumerate().any(|(unit, input)| {
            descendants(input.tree.root_node()).iter().any(|node| {
                node.kind() == "call"
                    && node.child_by_field_name("method").is_some_and(|name| {
                        matches!(
                            text(name, &input.source),
                            "require" | "require_relative" | "load" | "autoload"
                        )
                    })
                    && super::has_operands(*node)
                    && (!graph.loads.contains(&graph.position(unit, *node, false))
                        || !graph.outgoing.get(&unit).is_some_and(|edges| {
                            edges.iter().any(|(_, position)| {
                                *position == graph.position(unit, *node, false)
                            })
                        }))
            })
        });
        if !graph.acyclic() {
            return without_registration_proof(Self::default());
        }
        graph.collect_forwarders();
        let mut result = Self::default();
        let mut safe = BTreeSet::new();
        let mut exports = BTreeSet::new();
        for (key, definitions) in &graph.definitions {
            if graph.resolve(key, &mut BTreeSet::new()).is_some() {
                exports.insert(key.clone());
                let def = &definitions[0];
                safe.insert((
                    def.unit,
                    def.assignment.child_by_field_name("left").unwrap().id(),
                ));
                safe.insert((def.unit, def.value.id()));
                let proof = result
                    .0
                    .entry(units[def.unit].file.path.clone())
                    .or_default();
                if def.value.kind() == "call" {
                    proof.captures.insert(def.value.start_byte());
                }
            }
        }
        // Constant exports are accepted only if every reference is a closed forwarding use.
        for (unit, input) in units.iter().enumerate() {
            let root = input.tree.root_node();
            let nodes = descendants(root);
            let references = identifier_references(&nodes, &input.source);
            graph.local_export_aliases(unit, &nodes, &references, &mut safe, &mut result);
            for (assignment, left, value) in root_assignments(&nodes, root) {
                if left.kind() != "identifier" {
                    continue;
                }
                let Some(key) = graph.reference(value, unit) else {
                    continue;
                };
                let registrar = graph.value(&key, unit, value, &mut BTreeSet::new());
                let mut calls = Vec::new();
                let closed = references[text(left, &input.source)]
                    .iter()
                    .copied()
                    .filter(|n| *n != left)
                    .all(|reference| {
                        let Some(call) = closed_call(reference, assignment, &input.source) else {
                            return false;
                        };
                        calls.push(call.start_byte());
                        true
                    });
                if closed {
                    safe.insert((unit, value.id()));
                    let proof = result.0.entry(input.file.path.clone()).or_default();
                    if let Some(registrar) = registrar {
                        proof
                            .calls
                            .extend(calls.into_iter().map(|call| (call, registrar.clone())));
                    } else if graph.definitions.contains_key(&key) {
                        proof.unresolved_calls.extend(calls);
                    }
                }
            }
            for node in &nodes {
                if node.kind() != "call" {
                    continue;
                }
                let Some(receiver) = node.child_by_field_name("receiver") else {
                    continue;
                };
                let Some(key) = graph.reference(receiver, unit) else {
                    continue;
                };
                let member = node
                    .child_by_field_name("method")
                    .map(|n| text(n, &input.source));
                if member != Some("call") {
                    if let Some(registrar) = member
                        .and_then(|member| graph.forwarders.get(&(key.clone(), member.to_owned())))
                    {
                        if node.parent() == Some(root)
                            && graph.namespace_available(&key, unit, *node)
                        {
                            safe.insert((unit, receiver.id()));
                            result
                                .0
                                .entry(input.file.path.clone())
                                .or_default()
                                .calls
                                .insert(node.start_byte(), registrar.clone());
                        }
                    }
                    continue;
                }
                let Some(registrar) = graph.value(&key, unit, receiver, &mut BTreeSet::new())
                else {
                    continue;
                };
                if node.parent() == Some(root) {
                    safe.insert((unit, receiver.id()));
                    result
                        .0
                        .entry(input.file.path.clone())
                        .or_default()
                        .calls
                        .insert(node.start_byte(), registrar);
                } else if transparent_body(*node, &input.source) {
                    safe.insert((unit, receiver.id()));
                    result
                        .0
                        .entry(input.file.path.clone())
                        .or_default()
                        .forwarding
                        .insert(node.start_byte());
                }
            }
        }
        graph.isolated_namespaces(&mut result);
        let owners: BTreeSet<_> = exports
            .iter()
            .filter_map(|key| key.split_once("::").map(|(owner, _)| owner))
            .chain(graph.forwarders.keys().map(|(owner, _)| owner.as_str()))
            .collect();
        for (unit, input) in units.iter().enumerate() {
            for node in descendants(input.tree.root_node()) {
                if !matches!(node.kind(), "constant" | "scope_resolution")
                    || node
                        .parent()
                        .is_some_and(|p| p.kind() == "scope_resolution")
                {
                    continue;
                }
                if node.parent().is_some_and(|p| {
                    p.kind() == "module" && p.child_by_field_name("name") == Some(node)
                }) {
                    continue;
                }
                let Some(key) = graph.reference(node, unit) else {
                    continue;
                };
                if (exports.contains(&key) || owners.contains(key.as_str()))
                    && !safe.contains(&(unit, node.id()))
                    && !result
                        .0
                        .get(&input.file.path)
                        .is_some_and(|proof| proof.isolated_constants.contains(&node.start_byte()))
                {
                    return without_registration_proof(Self(
                        BTreeMap::new(),
                        graph.budget_exhausted(),
                    ));
                }
            }
        }
        graph.isolated_instances(&mut result);
        graph.named_handlers(&mut result, None);
        graph.closed_execution(&mut result);
        for input in units {
            if let Some(proof) = result.0.get_mut(&input.file.path) {
                proof.source = input.source.clone();
            }
        }
        let assertions = super::assertions::AssertionProviders::from_units_with_registrations(
            units,
            edges,
            configured,
            crate::resource_limits::MAX_ASSERTION_RESOLUTION_WORK,
            Some(&result),
        );
        for unit in units {
            if let Some(assertions) = assertions.get(&unit.file.path, &unit.source) {
                result
                    .0
                    .entry(unit.file.path.clone())
                    .or_default()
                    .isolated_calls
                    .extend(assertions.isolated_dispatch());
            }
        }
        graph.named_handlers(&mut result, Some(&assertions));
        for unit in units {
            if let Some(proof) = result.0.get_mut(&unit.file.path) {
                proof.source = unit.source.clone();
            }
        }
        if graph.budget_exhausted() {
            return without_registration_proof(Self(BTreeMap::new(), true));
        }
        (result, assertions)
    }

    pub fn work_exhausted(&self) -> bool {
        self.1
    }

    /// Reuses offsets only for the exact source from which the proof was derived.
    pub fn get(&self, file: &Path, source: &str) -> Option<&Proof> {
        self.0.get(file).filter(|proof| proof.source == source)
    }
}

// Registration and assertion providers must agree on whether a resolved load graph closes.
pub(super) fn source_graph_acyclic<'a>(
    files: impl IntoIterator<Item = (&'a Path, &'a Path)>,
    edges: &[SourceDependency],
) -> bool {
    let files: Vec<_> = files.into_iter().collect();
    let mut incoming = vec![0; files.len()];
    let mut outgoing = vec![Vec::new(); files.len()];
    for edge in edges {
        let from = files
            .iter()
            .position(|(source, _)| *source == edge.location.path);
        let to = files.iter().position(|(_, target)| *target == edge.target);
        if let (Some(from), Some(to)) = (from, to) {
            outgoing[from].push(to);
            incoming[to] += 1;
        }
    }
    let mut ready: Vec<_> = incoming
        .iter()
        .enumerate()
        .filter_map(|(i, n)| (*n == 0).then_some(i))
        .collect();
    let mut count = 0;
    while let Some(from) = ready.pop() {
        count += 1;
        for to in &outgoing[from] {
            incoming[*to] -= 1;
            if incoming[*to] == 0 {
                ready.push(*to);
            }
        }
    }
    count == files.len()
}

impl<'a> Graph<'a> {
    fn new(units: &'a [Unit], edges: &'a [SourceDependency]) -> Self {
        Self {
            units,
            edges,
            definitions: BTreeMap::new(),
            namespaces: BTreeMap::new(),
            protected_modules: BTreeSet::new(),
            forwarders: BTreeMap::new(),
            custom_allocation: false,
            root_modules: BTreeMap::new(),
            classes: BTreeMap::new(),
            unknown_loads: false,
            loads: BTreeSet::new(),
            unit_ids: BTreeMap::new(),
            available_proofs: RefCell::default(),
            load_work: Cell::new(0),
            proof_exhausted: Cell::new(false),
            proof_work: Cell::new(0),
            #[cfg(test)]
            proof_limit: crate::resource_limits::MAX_ASSERTION_RESOLUTION_WORK,
            outgoing: BTreeMap::new(),
            incoming: BTreeMap::new(),
        }
    }

    /// Rejects load cycles as evidence that a provider finished initialization.
    fn acyclic(&self) -> bool {
        source_graph_acyclic(
            self.units
                .iter()
                .map(|unit| (unit.file.path.as_path(), unit.canonical_path.as_path())),
            self.edges,
        )
    }

    /// Resolves the nearest declared lexical prefix without following unknown constant aliases.
    fn reference(&self, node: Node<'_>, unit: usize) -> Option<String> {
        let source = &self.units[unit].source;
        let name = constant(node, source)?;
        if text(node, source).starts_with("::") {
            return Some(name);
        }
        let first = name.split("::").next()?;
        let mut scope = node.parent();
        while let Some(parent) = scope {
            if matches!(parent.kind(), "module" | "class") {
                let owner = qualified(parent.child_by_field_name("name")?, source)?;
                let prefix = format!("{owner}::{first}");
                if self.definitions.contains_key(&prefix) || self.namespaces.contains_key(&prefix) {
                    return Some(format!("{owner}::{name}"));
                }
            }
            scope = parent.parent();
        }
        Some(name)
    }

    /// Resolves immutable exports with bounded alias depth and cycle rejection.
    fn resolve(&self, key: &str, visiting: &mut BTreeSet<String>) -> Option<String> {
        if visiting.len() >= 64 || !visiting.insert(key.to_owned()) {
            return None;
        }
        let result = self.resolve_inner(key, visiting);
        visiting.remove(key);
        result
    }

    /// Captures registrars on main; modules may only re-export proven constants.
    fn resolve_inner(&self, key: &str, visiting: &mut BTreeSet<String>) -> Option<String> {
        let definitions = self.definitions.get(key)?;
        if definitions.len() != 1 {
            return None;
        }
        let def = &definitions[0];
        let input = &self.units[def.unit];
        let root = input.tree.root_node();
        if root.has_error() || def.assignment.kind() != "assignment" {
            return None;
        }
        let top_level = def.assignment.parent() == Some(root) && !key.contains("::");
        if top_level {
            if self.namespaces.contains_key(key) {
                return None;
            }
        } else {
            let (owner, _) = key.split_once("::")?;
            if self.namespaces.get(owner) != Some(&1)
                || self.definitions.contains_key(owner)
                || super::ownership::protected_namespace(owner)
            {
                return None;
            }
            let body = def.assignment.parent()?;
            let module = body.parent()?;
            if body.kind() != "body_statement"
                || module.kind() != "module"
                || module.parent() != Some(root)
                || self.protected_modules.contains(&module.id())
            {
                return None;
            }
        }
        if let Some(reference) = self.reference(def.value, def.unit) {
            return self.value(&reference, def.unit, def.value, visiting);
        }
        if !top_level {
            return None;
        }
        let call = def.value;
        if call.kind() != "call"
            || call.child_by_field_name("receiver").is_some()
            || call.child_by_field_name("block").is_some()
            || !call
                .child_by_field_name("method")
                .is_some_and(|n| text(n, &input.source) == "method")
        {
            return None;
        }
        let args = call.child_by_field_name("arguments")?;
        if args.named_child_count() != 1 {
            return None;
        }
        static_method_name(args.named_child(0)?, &input.source).filter(|name| registration(name))
    }

    /// Requires initialization evidence before granting a reference its export identity.
    fn value(
        &self,
        key: &str,
        unit: usize,
        node: Node<'_>,
        visiting: &mut BTreeSet<String>,
    ) -> Option<String> {
        let def = self.definitions.get(key)?.first()?;
        if !self.available(
            self.position(unit, node, false),
            self.position(def.unit, def.assignment, true),
            &mut BTreeSet::new(),
        ) {
            return None;
        }
        self.resolve(key, visiting)
    }

    /// Uses the same character-based columns as resolved dependency locations.
    fn position(&self, unit: usize, node: Node<'_>, end: bool) -> (usize, usize, usize) {
        let input = &self.units[unit];
        let location = super::location(&input.file, node, &input.source);
        if end {
            (unit, location.end_line, location.end_column)
        } else {
            (unit, location.line, location.column)
        }
    }

    fn budget_exhausted(&self) -> bool {
        self.proof_exhausted.get() || self.proof_budget_exhausted(self.load_work.get())
    }

    fn charge_proof_work(&self, additional: usize) -> bool {
        self.proof_work
            .set(self.proof_work.get().saturating_add(additional));
        self.proof_budget_exhausted(self.proof_work.get())
    }

    // Each collector retains its established limit; exhaustion is shared and sticky.
    fn begin_proof_pass(&self) -> bool {
        self.proof_work.set(0);
        self.budget_exhausted()
    }

    fn proof_budget_exhausted(&self, work: usize) -> bool {
        #[cfg(test)]
        let limit = self.proof_limit;
        #[cfg(not(test))]
        let limit = crate::resource_limits::MAX_ASSERTION_RESOLUTION_WORK;
        if work > limit {
            self.proof_exhausted.set(true);
        }
        self.proof_exhausted.get()
    }

    /// Memoizes proven load contexts and bounds unsuccessful graph searches.
    fn available(
        &self,
        (unit, line, column): (usize, usize, usize),
        (target, end_line, end_column): (usize, usize, usize),
        seen: &mut BTreeSet<usize>,
    ) -> bool {
        let query = ((unit, line, column), (target, end_line, end_column));
        if self.available_proofs.borrow().contains(&query) {
            return true;
        }
        if unit == target {
            return (end_line, end_column) < (line, column);
        }
        if !seen.insert(unit) {
            return false;
        }
        let outgoing = self
            .outgoing
            .get(&unit)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let callers = self
            .incoming
            .get(&unit)
            .map(Vec::as_slice)
            .unwrap_or_default();
        self.load_work.set(
            self.load_work
                .get()
                .saturating_add(outgoing.len() + callers.len() + 1),
        );
        if self.budget_exhausted() {
            return false;
        }
        let direct = outgoing.iter().any(|(next, position)| {
            (position.1, position.2) < (line, column)
                && self.available((*next, usize::MAX, usize::MAX), query.1, seen)
        });
        let inherited = !direct
            && !self.unknown_loads
            && !callers.is_empty()
            && callers.iter().all(|(position, dependency_only)| {
                *dependency_only
                    && self.loads.contains(position)
                    && self.available(*position, query.1, &mut seen.clone())
            });
        if direct || inherited {
            self.available_proofs.borrow_mut().insert(query);
        }
        direct || inherited
    }
}

fn allocation_override(node: Node<'_>, source: &str) -> bool {
    ((node.kind() == "singleton_method"
        || (node.kind() == "method"
            && std::iter::successors(node.parent(), |n| n.parent())
                .any(|n| n.kind() == "singleton_class")))
        && node
            .child_by_field_name("name")
            .is_some_and(|name| matches!(text(name, source), "new" | "allocate")))
        || (matches!(node.kind(), "alias" | "undef")
            && descendants(node).iter().any(|name| {
                matches!(
                    text(*name, source).trim_start_matches(':'),
                    "new" | "allocate"
                )
            }))
        || (node.kind() == "call"
            && node
                .child_by_field_name("method")
                .is_some_and(|name| super::method_table_mutator(text(name, source)))
            && node
                .child_by_field_name("arguments")
                .and_then(|args| args.named_child(0))
                .and_then(|arg| static_method_name(arg, source))
                .is_none_or(|target| matches!(target.as_str(), "new" | "allocate")))
}

/// Extracts only complete root-level assignment statements.
fn root_assignments<'tree: 'nodes, 'nodes>(
    nodes: &'nodes [Node<'tree>],
    root: Node<'tree>,
) -> impl Iterator<Item = (Node<'tree>, Node<'tree>, Node<'tree>)> + 'nodes {
    nodes
        .iter()
        .copied()
        .filter(move |n| n.kind() == "assignment" && n.parent() == Some(root))
        .filter_map(|node| {
            Some((
                node,
                node.child_by_field_name("left")?,
                node.child_by_field_name("right")?,
            ))
        })
}

/// Returns a later root alias assignment only when this reference is its entire value.
fn alias_assignment<'a>(reference: Node<'a>, origin: Node<'a>, root: Node<'a>) -> Option<Node<'a>> {
    reference.parent().filter(|node| {
        node.kind() == "assignment"
            && node.parent() == Some(root)
            && node.child_by_field_name("right") == Some(reference)
            && node.start_byte() >= origin.end_byte()
    })
}

/// Accepts only static constant paths, preserving qualification.
fn constant(node: Node<'_>, source: &str) -> Option<String> {
    if !matches!(node.kind(), "constant" | "scope_resolution") {
        return None;
    }
    if descendants(node)
        .iter()
        .any(|n| !matches!(n.kind(), "constant" | "scope_resolution"))
    {
        return None;
    }
    Some(text(node, source).trim_start_matches("::").to_owned())
}

/// Assigns a declaration its lexical namespace rather than resolving its value.
fn qualified(node: Node<'_>, source: &str) -> Option<String> {
    let name = constant(node, source)?;
    if node.kind() == "scope_resolution" {
        return Some(name);
    }
    let mut parts = vec![name];
    let mut parent = node.parent();
    if parent.is_some_and(|scope| scope.child_by_field_name("name") == Some(node)) {
        parent = parent.and_then(|scope| scope.parent());
    }
    while let Some(scope) = parent {
        if matches!(scope.kind(), "module" | "class") {
            if parts.len() >= 64 {
                return None;
            }
            let owner = scope.child_by_field_name("name")?;
            parts.push(constant(owner, source)?);
            if owner.kind() == "scope_resolution" {
                break;
            }
        }
        parent = scope.parent();
    }
    parts.reverse();
    Some(parts.join("::"))
}

/// Recognizes an unexecuted method body that forwards matcher and block unchanged.
fn transparent_body(call: Node<'_>, source: &str) -> bool {
    let Some(body) = call.parent() else {
        return false;
    };
    let Some(method) = body.parent().filter(|n| n.kind() == "singleton_method") else {
        return false;
    };
    let Some(parameters) = method.child_by_field_name("parameters") else {
        return false;
    };
    if body.named_child_count() != 1 || parameters.named_child_count() != 2 {
        return false;
    }
    let (Some(matcher), Some(block)) = (parameters.named_child(0), parameters.named_child(1))
    else {
        return false;
    };
    let Some(handler) = block.child_by_field_name("name") else {
        return false;
    };
    let Some(args) = call.child_by_field_name("arguments") else {
        return false;
    };
    matcher.kind() == "identifier"
        && block.kind() == "block_parameter"
        && args.named_child_count() == 2
        && args
            .named_child(0)
            .is_some_and(|n| n.kind() == "identifier" && text(n, source) == text(matcher, source))
        && args.named_child(1).is_some_and(|n| {
            n.kind() == "block_argument"
                && n.named_child(0)
                    .is_some_and(|n| text(n, source) == text(handler, source))
        })
}

#[cfg(test)]
fn test_unit(source: &str) -> Unit {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .unwrap();
    Unit {
        file: SourceFile {
            path: PathBuf::from("steps.rb"),
            language: SourceLanguage::Ruby,
        },
        canonical_path: PathBuf::from("steps.rb"),
        source: source.to_owned(),
        tree: parser.parse(source, None).unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn forwarders_reject_dynamic_namespace_names() {
        let units = [test_unit("module self::Forward; def self.register(pattern, &handler); REGISTER.call(pattern, &handler); end; end")];
        assert!(!units[0].tree.root_node().has_error());
        let mut graph = Graph::new(&units, &[]);
        graph.collect_forwarders();
        assert!(graph.forwarders.is_empty());
    }

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
                let mut graph = Graph::new(&units, &[]);
                graph.proof_limit = limit;
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
            let mut graph = Graph::new(&units, &[]);
            graph.proof_limit = nodes.len() * 2
                + 2
                + descendants(first_body).len()
                + descendants(first_body)
                    .iter()
                    .find(|n| matches!(n.kind(), "block" | "do_block"))
                    .map(|n| descendants(*n).len())
                    .unwrap();
            graph.proof_exhausted.set(prior);
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

    #[test]
    fn optional_passes_share_exhaustion_without_resetting_load_work() {
        let source = "module Local; def self.safe; end; end; register = method(:Given); register.call('step') { work() }";
        let units = [test_unit(source)];
        let mut graph = Graph::new(&units, &[]);
        graph.proof_limit = 2;
        assert!(!graph.charge_proof_work(1));
        graph.load_work.set(1);
        assert!(!graph.budget_exhausted());
        assert!(graph.charge_proof_work(2));
        assert!(graph.begin_proof_pass());
        assert_eq!(graph.proof_work.get(), 0);
        assert_eq!(graph.load_work.get(), 1);
        assert!(graph.budget_exhausted());
        let nodes = descendants(units[0].tree.root_node());
        let references = identifier_references(&nodes, source);
        let mut result = Providers::default();
        graph.isolated_instances(&mut result);
        graph.isolated_namespaces(&mut result);
        graph.local_export_aliases(0, &nodes, &references, &mut BTreeSet::new(), &mut result);
        assert!(result.0.is_empty());
    }

    #[test]
    fn isolated_constructor_requires_preceding_class_initialization() {
        let units = [test_unit(
            "instance = Local.new; class Local; def run; end; end; instance.run",
        )];
        let mut result = Providers::default();
        Graph::new(&units, &[]).isolated_instances(&mut result);
        assert!(result.0.is_empty());
    }

    #[test]
    fn optional_proof_collectors_fail_closed_at_each_work_boundary() {
        let source = "def handler; work(); end; callable = proc { work() }; Given('one', &method(:handler)); Then('two', &callable); register = method(:Given); register.call('three') { work() }; module Local; def self.safe; end; end; Local.safe; factory = -> { Given('four') { work() }; Then('five') { work() } }; factory.call; class LocalInstance; def safe; end; end; instance = LocalInstance.new; instance.safe";
        let units = [test_unit(source)];
        let nodes = descendants(units[0].tree.root_node());
        let references = identifier_references(&nodes, source);
        let assertions = super::super::assertions::AssertionProviders::default();
        let mut exhausted = [false; 5];
        let mut completed = [false; 5];
        for limit in 0..nodes.len() * 4 {
            for collector in 0..5 {
                let mut graph = Graph::new(&units, &[]);
                graph.proof_limit = limit;
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

    #[test]
    fn load_proof_work_exhaustion_withdraws_optional_trust() {
        let graph = Graph::new(&[], &[]);
        graph
            .load_work
            .set(crate::resource_limits::MAX_ASSERTION_RESOLUTION_WORK);
        assert!(!graph.available((0, 1, 1), (1, 1, 1), &mut BTreeSet::new()));
        assert_eq!(
            graph.load_work.get(),
            crate::resource_limits::MAX_ASSERTION_RESOLUTION_WORK + 1
        );
        assert!(graph.available_proofs.borrow().is_empty());
        assert!(graph.budget_exhausted());
        let edge = SourceDependency::new(
            crate::model::SourceLocation::new(PathBuf::from("entry.rb"), 1, 1, 1, 1),
            PathBuf::from("consumer.rb"),
        );
        assert!(!edge.dependency_only);
        assert!(edge.with_dependency_only_target(true).dependency_only);
    }

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

    #[test]
    fn optional_provider_proofs_respect_both_resource_limits() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("alias")).unwrap();
        let provider = "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end";
        let consumer = "require_relative 'provider'; register = Provider::GIVEN; register.call('same') { work() }";
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
            files[0].path.canonicalize().unwrap(),
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
            files[1].path.canonicalize().unwrap(),
        ));
        assert!(Providers::collect(&files, &cycle).unwrap().0.is_empty());
        for budget in [(1, bytes), (2, bytes - 1)] {
            assert!(Providers::collect_with_budget(&files, &edges, budget)
                .unwrap()
                .0
                .is_empty());
        }
    }

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
}
