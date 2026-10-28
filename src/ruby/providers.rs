//! Closed constant registration exports over resolved, preceding source loads.
use super::registration_aliases::{closed_call, identifier_references};
use super::{descendants, registration, static_method_name, text};
use crate::resource_limits::{read_utf8, PassBudget, WorkBudget, MAX_PROJECT_INPUT_BYTES};
#[cfg(test)]
use crate::resource_limits::{MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES};
use crate::source_adapter::{SourceDependency, SourceFile};
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
    /// Load-resolution work spans the run; proof work restarts with each collector pass.
    budget: Cell<PassBudget>,
    outgoing: BTreeMap<usize, Vec<(usize, Position)>>,
    incoming: BTreeMap<usize, Vec<(Position, bool)>>,
}

/// Proof sources for a selection, or why proofs are unavailable.
pub(super) enum ProofSources {
    /// Every selected Ruby file was read and parsed within the budget.
    Loaded(Vec<Unit>),
    /// The selection exceeds the `(files, bytes)` proof budget.
    OverBudget,
    /// A selected file could not be read, parsed without syntax errors, or canonicalized;
    /// extraction reports it.
    Untrusted,
}

/// Test entry point over [`load_proof_sources`] that keeps only the loaded units.
#[cfg(test)]
pub(super) fn load_units(
    files: &[SourceFile],
    budget: (usize, usize),
) -> Result<Option<Vec<Unit>>> {
    Ok(match load_proof_sources(files, budget)? {
        ProofSources::Loaded(units) => Some(units),
        ProofSources::OverBudget | ProofSources::Untrusted => None,
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
    let (mut modules, mut bytes) = (WorkBudget::new(budget.0), WorkBudget::new(budget.1));
    for file in files.iter().filter(|f| f.language == super::RUBY) {
        if !modules.admit(1) {
            return Ok(ProofSources::OverBudget);
        }
        let Ok(source) = read_utf8(&file.path, "Ruby provider source", MAX_PROJECT_INPUT_BYTES)
        else {
            // Extraction reports per-file failures and preserves trustworthy neighbors.
            return Ok(ProofSources::Untrusted);
        };
        if !bytes.admit(source.len()) {
            return Ok(ProofSources::OverBudget);
        }
        let source = super::prepare_nuls(&source).into_owned();
        let tree = parser
            .parse(&source, None)
            .context("Ruby provider parser returned no tree")?;
        if super::syntax_error(tree.root_node(), &source).is_some() {
            // A file Ruby rejects can still bind names its recovered tree hides.
            return Ok(ProofSources::Untrusted);
        }
        let Ok(canonical_path) = crate::config::canonical_platform_path(&file.path) else {
            return Ok(ProofSources::Untrusted);
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

    /// Collect test registration proofs within file and byte limits, returning empty proofs when loading is unavailable.
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

    /// Collect registration proofs from test units without configured assertion providers.
    #[cfg(test)]
    pub(super) fn from_units(units: &[Unit], edges: &[SourceDependency]) -> Result<Self> {
        Ok(Self::from_units_with_assertions(units, edges, &[]).0)
    }

    /// Collect registrar and configured assertion proofs from the same loaded-unit graph.
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

    /// Report whether provider proof collection exceeded its work budget.
    pub fn work_exhausted(&self) -> bool {
        self.1
    }

    /// Reuses offsets only for the exact source from which the proof was derived.
    pub fn get(&self, file: &Path, source: &str) -> Option<&Proof> {
        self.0.get(file).filter(|proof| proof.source == source)
    }
}

// Registration and assertion providers must agree on whether a resolved load graph closes.
/// Check the load graph for cycles, ignoring edges whose endpoints are outside the supplied files.
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
    /// Create an empty proof graph over loaded units and dependency edges.
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
            budget: Cell::new(PassBudget::new(
                crate::resource_limits::MAX_ASSERTION_RESOLUTION_WORK,
            )),
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

    /// Applies `charge` to the shared budget and returns its result.
    fn with_budget<T>(&self, charge: impl FnOnce(&mut PassBudget) -> T) -> T {
        let mut budget = self.budget.get();
        let result = charge(&mut budget);
        self.budget.set(budget);
        result
    }

    /// Check shared exhaustion, including the accumulated load-resolution work.
    fn budget_exhausted(&self) -> bool {
        self.budget.get().exhausted()
    }

    /// Charge saturating load-resolution work, which spans every pass, and report exhaustion.
    fn charge_load_work(&self, additional: usize) -> bool {
        self.with_budget(|budget| budget.charge_run(additional))
    }

    /// Charge saturating work to the current proof pass and report sticky budget exhaustion.
    fn charge_proof_work(&self, additional: usize) -> bool {
        self.with_budget(|budget| budget.charge_pass(additional))
    }

    /// Reset per-pass work while preserving shared exhaustion and prior load-resolution work.
    fn begin_proof_pass(&self) -> bool {
        self.with_budget(PassBudget::begin_pass)
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
        if self.charge_load_work(outgoing.len() + callers.len() + 1) {
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

/// Detect definitions or mutations that may replace new or allocate, including unknown mutation targets.
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

/// Parse a Ruby source fixture into a provider unit using the standard step-file path.
#[cfg(test)]
fn test_unit(source: &str) -> Unit {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .unwrap();
    Unit {
        file: SourceFile {
            path: PathBuf::from("steps.rb"),
            language: super::RUBY,
        },
        canonical_path: PathBuf::from("steps.rb"),
        source: source.to_owned(),
        tree: parser.parse(source, None).unwrap(),
    }
}

#[cfg(test)]
mod tests;
