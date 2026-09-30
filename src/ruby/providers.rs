//! Closed constant registration exports over resolved, preceding source loads.
use super::registration_aliases::{closed_call, identifier_references};
use super::{descendants, registration, static_method_name, text};
use crate::resource_limits::{
    read_utf8, MAX_PROJECT_INPUT_BYTES, MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES,
};
use crate::source_adapter::{SourceDependency, SourceFile, SourceLanguage};
use anyhow::{Context, Result};
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
    pub isolated_constants: BTreeSet<usize>,
    pub unresolved_calls: BTreeSet<usize>,
}

#[derive(Default)]
pub(super) struct Providers(BTreeMap<PathBuf, Proof>);

struct Unit {
    file: SourceFile,
    source: String,
    tree: Tree,
}

struct Definition<'a> {
    unit: usize,
    assignment: Node<'a>,
    value: Node<'a>,
}

struct Graph<'a> {
    units: &'a [Unit],
    edges: &'a [SourceDependency],
    definitions: BTreeMap<String, Vec<Definition<'a>>>,
    namespaces: BTreeMap<String, usize>,
    protected_modules: BTreeSet<usize>,
}

impl Providers {
    /// Builds bounded optional proofs; extraction remains authoritative when preparation is unavailable.
    pub fn collect(files: &[SourceFile], edges: &[SourceDependency]) -> Result<Self> {
        Self::collect_with_budget(
            files,
            edges,
            (MAX_REGISTRATION_MODULES, MAX_REGISTRATION_MODULE_BYTES),
        )
    }

    fn collect_with_budget(
        files: &[SourceFile],
        edges: &[SourceDependency],
        (max_files, max_bytes): (usize, usize),
    ) -> Result<Self> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_ruby::LANGUAGE.into())?;
        let mut units = Vec::new();
        let mut bytes = 0;
        for file in files.iter().filter(|f| f.language == SourceLanguage::Ruby) {
            if units.len() >= max_files {
                return Ok(Self::default());
            }
            let Ok(source) = read_utf8(&file.path, "Ruby provider source", MAX_PROJECT_INPUT_BYTES)
            else {
                // Extraction reports per-file failures and preserves trustworthy neighbors.
                return Ok(Self::default());
            };
            bytes += source.len();
            if bytes > max_bytes {
                return Ok(Self::default());
            }
            let tree = parser
                .parse(&source, None)
                .context("Ruby provider parser returned no tree")?;
            units.push(Unit {
                file: file.clone(),
                source,
                tree,
            });
        }
        let mut graph = Graph {
            units: &units,
            edges,
            definitions: BTreeMap::new(),
            namespaces: BTreeMap::new(),
            protected_modules: BTreeSet::new(),
        };
        for (unit, input) in units.iter().enumerate() {
            for node in descendants(input.tree.root_node()) {
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
                if node.kind() == "module" {
                    if let Some(name) = node
                        .child_by_field_name("name")
                        .and_then(|n| constant(n, &input.source))
                    {
                        *graph.namespaces.entry(name).or_default() += 1;
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
        if !graph.acyclic() {
            return Ok(Self::default());
        }
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
                let Some(registrar) = graph.value(&key, unit, receiver, &mut BTreeSet::new())
                else {
                    continue;
                };
                if !node
                    .child_by_field_name("method")
                    .is_some_and(|n| text(n, &input.source) == "call")
                {
                    continue;
                }
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
        let owners: BTreeSet<_> = exports
            .iter()
            .filter_map(|key| key.split_once("::").map(|(owner, _)| owner))
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
                {
                    return Ok(Self::default());
                }
            }
        }
        graph.isolated_instances(&mut result);
        for unit in units {
            if let Some(proof) = result.0.get_mut(&unit.file.path) {
                proof.source = unit.source;
            }
        }
        Ok(result)
    }

    /// Reuses offsets only for the exact source from which the proof was derived.
    pub fn get(&self, file: &Path, source: &str) -> Option<&Proof> {
        self.0.get(file).filter(|proof| proof.source == source)
    }
}

impl Graph<'_> {
    /// Rejects load cycles as evidence that a provider finished initialization.
    fn acyclic(&self) -> bool {
        let mut incoming = vec![0; self.units.len()];
        let mut outgoing = vec![Vec::new(); self.units.len()];
        for edge in self.edges {
            let from = self
                .units
                .iter()
                .position(|u| u.file.path == edge.location.path);
            let to = self.units.iter().position(|u| u.file.path == edge.target);
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
        count == self.units.len()
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
                let owner = constant(parent.child_by_field_name("name")?, source)?;
                let prefix = format!("{owner}::{first}");
                if self.definitions.contains_key(&prefix) || self.namespaces.contains_key(&prefix) {
                    return Some(format!("{owner}::{name}"));
                }
            }
            scope = parent.parent();
        }
        Some(name)
    }

    /// Proves closed constructor bindings without treating their methods as framework registrations.
    fn isolated_instances(&self, result: &mut Providers) {
        let mut classes = BTreeMap::<String, Vec<(usize, Node<'_>, BTreeSet<&str>)>>::new();
        for (unit, input) in self.units.iter().enumerate() {
            for node in descendants(input.tree.root_node()) {
                if node.kind() == "class"
                    && node.parent() == Some(input.tree.root_node())
                    && node.child_by_field_name("superclass").is_none()
                {
                    if let Some(name) = node
                        .child_by_field_name("name")
                        .filter(|n| n.kind() == "constant")
                    {
                        classes
                            .entry(text(name, &input.source).to_owned())
                            .or_default()
                            .push((
                                unit,
                                node,
                                descendants(node)
                                    .into_iter()
                                    .filter(|n| {
                                        n.kind() == "method"
                                            && n.parent().and_then(|p| p.parent()) == Some(node)
                                    })
                                    .filter_map(|n| n.child_by_field_name("name"))
                                    .map(|n| text(n, &input.source))
                                    .collect(),
                            ));
                    }
                }
                // Any custom factory invalidates this deliberately bounded constructor model.
                if node.kind() == "singleton_method"
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|n| matches!(text(n, &input.source), "new" | "allocate"))
                {
                    return;
                }
            }
        }
        for (unit, input) in self.units.iter().enumerate() {
            let root = input.tree.root_node();
            let nodes = descendants(root);
            let references = identifier_references(&nodes, &input.source);
            for (assignment, binding, constructor) in root_assignments(&nodes, root) {
                if binding.kind() != "identifier"
                    || constructor.kind() != "call"
                    || constructor.child_by_field_name("block").is_some()
                    || constructor
                        .child_by_field_name("arguments")
                        .is_some_and(|a| a.named_child_count() != 0)
                    || !constructor
                        .child_by_field_name("method")
                        .is_some_and(|n| text(n, &input.source) == "new")
                {
                    continue;
                }
                let Some(receiver) = constructor
                    .child_by_field_name("receiver")
                    .filter(|n| n.kind() == "constant")
                else {
                    continue;
                };
                let name = text(receiver, &input.source);
                if matches!(
                    name,
                    "Object" | "BasicObject" | "Class" | "Module" | "Method"
                ) || self.definitions.contains_key(name)
                {
                    continue;
                }
                let Some(declarations) = classes.get(name).filter(|v| v.len() == 1) else {
                    continue;
                };
                let (class_unit, class, methods) = &declarations[0];
                if !self.available(
                    self.position(unit, constructor, false),
                    self.position(*class_unit, *class, true),
                    &mut BTreeSet::new(),
                ) {
                    continue;
                }
                if methods.iter().any(|name| {
                    matches!(
                        *name,
                        "send" | "public_send" | "__send__" | "method_missing"
                    )
                }) {
                    continue;
                }
                let mut calls = BTreeSet::new();
                let closed = references[text(binding, &input.source)]
                    .iter()
                    .filter(|n| **n != binding)
                    .all(|reference| {
                        let Some(call) = reference.parent() else {
                            return false;
                        };
                        if call.kind() != "call"
                            || call.start_byte() < assignment.end_byte()
                            || call.child_by_field_name("receiver") != Some(*reference)
                        {
                            return false;
                        }
                        let Some(method) = call.child_by_field_name("method") else {
                            return false;
                        };
                        let name = text(method, &input.source);
                        if matches!(name, "send" | "public_send" | "__send__") {
                            let Some(target) = call
                                .child_by_field_name("arguments")
                                .and_then(|a| a.named_child(0))
                                .and_then(|n| static_method_name(n, &input.source))
                            else {
                                return false;
                            };
                            if !methods.contains(target.as_str()) || call.parent() != Some(root) {
                                return false;
                            }
                            calls.insert(call.start_byte());
                        } else if !methods.contains(name) {
                            return false;
                        }
                        true
                    });
                if closed {
                    let proof = result.0.entry(input.file.path.clone()).or_default();
                    proof.isolated_constants.insert(receiver.start_byte());
                    proof.isolated_calls.extend(calls);
                }
            }
        }
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

    /// Accepts only direct exports from a single, unchanged top-level module.
    fn resolve_inner(&self, key: &str, visiting: &mut BTreeSet<String>) -> Option<String> {
        let (owner, _) = key.split_once("::")?;
        if self.namespaces.get(owner) != Some(&1) || self.definitions.contains_key(owner) {
            return None;
        }
        if matches!(
            owner,
            "Cucumber" | "Object" | "BasicObject" | "Kernel" | "Module" | "Class" | "Method"
        ) {
            return None;
        }
        let definitions = self.definitions.get(key)?;
        if definitions.len() != 1 {
            return None;
        }
        let def = &definitions[0];
        let input = &self.units[def.unit];
        if input.tree.root_node().has_error() || def.assignment.kind() != "assignment" {
            return None;
        }
        // Only direct statements in a simple top-level module can publish an export.
        let body = def.assignment.parent()?;
        let module = body.parent()?;
        if body.kind() != "body_statement"
            || module.kind() != "module"
            || module.parent() != Some(input.tree.root_node())
        {
            return None;
        }
        if self.protected_modules.contains(&module.id()) {
            return None;
        }
        if let Some(reference) = self.reference(def.value, def.unit) {
            return self.value(&reference, def.unit, def.value, visiting);
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

    /// Searches each unit once per query; recursive loads share an end-of-file cutoff.
    fn available(
        &self,
        (unit, line, column): (usize, usize, usize),
        (target, end_line, end_column): (usize, usize, usize),
        seen: &mut BTreeSet<usize>,
    ) -> bool {
        if unit == target {
            return (end_line, end_column) < (line, column);
        }
        if !seen.insert(unit) {
            return false;
        }
        self.edges
            .iter()
            .filter(|e| {
                e.location.path == self.units[unit].file.path
                    && (e.location.line, e.location.column) < (line, column)
            })
            .any(|edge| {
                self.units
                    .iter()
                    .position(|u| u.file.path == edge.target)
                    .is_some_and(|next| {
                        self.available(
                            (next, usize::MAX, usize::MAX),
                            (target, end_line, end_column),
                            seen,
                        )
                    })
            })
    }
}

/// Extracts only complete root-level assignment statements.
fn root_assignments<'a>(
    nodes: &'a [Node<'a>],
    root: Node<'a>,
) -> impl Iterator<Item = (Node<'a>, Node<'a>, Node<'a>)> {
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
    let mut parent = node.parent();
    while let Some(scope) = parent {
        if matches!(scope.kind(), "module" | "class") {
            let owner = constant(scope.child_by_field_name("name")?, source)?;
            return Some(format!("{owner}::{name}"));
        }
        parent = scope.parent();
    }
    Some(name)
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
mod tests {
    use super::*;

    #[test]
    fn optional_provider_proofs_respect_both_resource_limits() {
        let dir = tempfile::tempdir().unwrap();
        let provider = "module Provider; GIVEN = method(:Given); end";
        let consumer = "require_relative 'provider'; register = Provider::GIVEN; register.call('same') { work() }";
        let files: Vec<_> = [("provider.rb", provider), ("entry.rb", consumer)]
            .into_iter()
            .map(|(name, source)| {
                let path = dir.path().join(name);
                std::fs::write(&path, source).unwrap();
                SourceFile {
                    path,
                    language: SourceLanguage::Ruby,
                }
            })
            .collect();
        let edges = [SourceDependency::new(
            crate::model::SourceLocation::new(files[1].path.clone(), 1, 1, 1, 28),
            files[0].path.clone(),
        )];
        let bytes = provider.len() + consumer.len();
        let positive = Providers::collect_with_budget(&files, &edges, (2, bytes)).unwrap();
        assert_eq!(
            positive.get(&files[1].path, consumer).unwrap().calls.len(),
            1
        );
        for budget in [(1, bytes), (2, bytes - 1)] {
            assert!(Providers::collect_with_budget(&files, &edges, budget)
                .unwrap()
                .0
                .is_empty());
        }
    }
}
