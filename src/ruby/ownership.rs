//! Closed declaration paths and conservative effects across the selected source graph.

use super::{
    descendants, inside_deferred_body, may_replace_dsl, protected_method, static_method_name, text,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct OwnerPath(Vec<String>);

// Paths identify only explicitly declared, unescaped namespaces. Constant references and
// assignments are retained separately: a path spelling alone never proves object identity.
#[derive(Default)]
struct Declarations {
    scopes: BTreeMap<usize, Option<OwnerPath>>,
    known: BTreeSet<OwnerPath>,
}
use tree_sitter::Node;

#[derive(Default)]
pub(super) struct RegistrationEffects {
    wrappers: super::registration_wrappers::WrapperEffects,
    unknown: bool,
    // First node in this source that made ownership unresolved; not merged across sources.
    cause: Option<(usize, usize)>,
    // Owners map to the first node that mutated or exposed them; only keys merge across sources.
    mutated_owners: BTreeMap<OwnerPath, (usize, usize)>,
    exposed_owners: BTreeMap<OwnerPath, (usize, usize)>,
    // Whether some analyzed file's load phase is not closed or some load is unresolved.
    open_load_phase: bool,
    // Dispatch calls deferred because they sit in instance methods of a closed load phase; they
    // invalidate only if the merged suite turns out to be open.
    contained: Vec<crate::model::SourceLocation>,
    // Top-level `require`/`require_relative` calls; only these run unresolved code during load.
    load_requires: Vec<crate::model::SourceLocation>,
}

fn site(node: Node<'_>) -> (usize, usize) {
    (node.start_byte(), node.end_byte())
}

impl RegistrationEffects {
    /// Collects source-local ownership evidence while preserving resolved captures and deferred bodies.
    pub(super) fn collect(
        file: &crate::source_adapter::SourceFile,
        finalized: bool,
        root: Node<'_>,
        source: &str,
        aliases: &super::registration_aliases::RegistrationAliases,
        wrappers: &super::registration_wrappers::RegistrationWrappers,
        proof: Option<&super::providers::Proof>,
    ) -> Self {
        // Containment needs a finalized session to see the whole suite; without one it never applies.
        let closed = finalized && super::load_phase::closed(root, source);
        let mut effects = Self {
            wrappers: wrappers.effects.clone(),
            open_load_phase: !closed,
            load_requires: super::load_phase::load_requires(root, source)
                .map(|call| super::location(file, call, source))
                .collect(),
            ..Self::default()
        };
        let declarations = Declarations::collect(root, source);
        for node in descendants(root) {
            if changes_registrar_implementation(node, source) {
                effects.mark_unknown(node);
                continue;
            }
            if matches!(node.kind(), "class" | "module")
                && node
                    .child_by_field_name("name")
                    .is_some_and(|name| name.kind() == "scope_resolution")
                && declarations
                    .scopes
                    .get(&node.id())
                    .is_some_and(Option::is_none)
            {
                // Namespace lookup itself can execute a hook, even with an empty body.
                effects.mark_unknown(node);
            }
            if matches!(node.kind(), "constant" | "scope_resolution")
                && !node
                    .parent()
                    .is_some_and(|p| p.kind() == "scope_resolution")
                && !declaration_name(node)
                && !world_argument(node, source)
                && !proof.is_some_and(|p| p.isolated_constants.contains(&node.start_byte()))
            {
                if let Some((absolute, parts)) = constant_path(node, source) {
                    if let Some(paths) = declarations.lookup_candidates(node, absolute, &parts) {
                        for path in paths {
                            effects.exposed_owners.entry(path).or_insert(site(node));
                        }
                    } else {
                        // An unresolved lexical scope cannot provide a disjointness proof.
                        effects.mark_unknown(node);
                    }
                } else {
                    effects.mark_unknown(node);
                }
            }
            // Passing the lexical owner to arbitrary code can install or alias its method table.
            // Keep this conservative even in deferred bodies until their effects are modeled.
            if node.kind() == "self"
                && !node.parent().is_some_and(|parent| {
                    parent.kind() == "singleton_method"
                        && parent.child_by_field_name("name") != Some(node)
                        && parent.child_by_field_name("parameters") != Some(node)
                        && parent.child_by_field_name("body") != Some(node)
                })
            {
                if let Some(owner) = declarations.owner(node) {
                    effects.exposed_owners.entry(owner).or_insert(site(node));
                }
            }
            let mutation = match node.kind() {
                "alias" | "undef" => {
                    let mut cursor = node.walk();
                    let protected = node.named_children(&mut cursor).any(|name| {
                        let name = if matches!(name.kind(), "identifier" | "constant") {
                            Some(text(name, source).to_owned())
                        } else {
                            static_method_name(name, source)
                        };
                        name.is_none_or(|name| protected_method(&name))
                    });
                    protected
                }
                "method" | "singleton_method" => {
                    node.child_by_field_name("name").is_some_and(|name| {
                        let name = text(name, source);
                        protected_method(name)
                            && (name != "to_proc"
                                || declarations.owner(node).is_none()
                                || (node.kind() == "singleton_method"
                                    && node
                                        .child_by_field_name("object")
                                        .is_none_or(|object| object.kind() != "self")))
                    })
                }
                "call" => {
                    !aliases.capture(node)
                        && !proof.is_some_and(|p| p.isolated_calls.contains(&node.start_byte()))
                        && !inside_deferred_body(node, source, aliases, wrappers)
                        && may_replace_dsl(node, source)
                }
                _ => false,
            };
            if !mutation {
                continue;
            }
            // Qualified calls/definitions require receiver resolution. Lexical nesting alone
            // cannot prove that an arbitrary receiver is isolated from the registrar.
            let explicit_receiver =
                node.child_by_field_name("receiver").is_some() || node.kind() == "singleton_method";
            let lexical_singleton = node.kind() == "singleton_method"
                && node
                    .child_by_field_name("object")
                    .is_some_and(|n| n.kind() == "self");
            if (!explicit_receiver || lexical_singleton) && node.kind() != "call" {
                if let Some(owner) = declarations.mutation_owner(node) {
                    effects.mutated_owners.entry(owner).or_insert(site(node));
                    continue;
                }
            }
            if closed
                && node.kind() == "call"
                && super::load_phase::contained_scope(node).is_some_and(|scope| {
                    declarations
                        .scopes
                        .get(&scope.id())
                        .is_some_and(Option::is_some)
                })
            {
                effects.contained.push(super::location(file, node, source));
                continue;
            }
            effects.mark_unknown(node);
        }
        effects
    }

    fn mark_unknown(&mut self, node: Node<'_>) {
        self.unknown = true;
        self.cause.get_or_insert(site(node));
    }

    /// Byte range of the node in this source whose evidence withdrew registration trust.
    pub(super) fn invalidation_site(&self) -> Option<(usize, usize)> {
        self.cause
            .or_else(|| self.wrappers.invalidation_site())
            .or_else(|| self.exposed_mutation().map(|(_, site)| *site))
    }

    /// Withdraws trust, locating the cause when the caller has a source node.
    pub(super) fn invalidate(&mut self, cause: Option<Node<'_>>) {
        self.unknown = true;
        if let Some(node) = cause {
            self.cause.get_or_insert(site(node));
        }
    }

    /// Records that the suite's load phase is open: a load-time `require` is unresolved, or an
    /// analyzed file was never extracted.
    pub(super) fn open_load_phase(&mut self) {
        self.open_load_phase = true;
    }

    /// Whether `location` is one of this file's top-level `require` calls.
    pub(super) fn load_require(&self, location: &crate::model::SourceLocation) -> bool {
        self.load_requires.contains(location)
    }

    /// The first contained call, when the merged suite's load phase is open and the call can
    /// therefore run during load.
    pub(super) fn escaped_call(&self) -> Option<&crate::model::SourceLocation> {
        self.contained.first().filter(|_| self.open_load_phase)
    }

    /// Combines file effects so later sources can invalidate registrations extracted earlier.
    pub(super) fn extend(&mut self, other: Self) {
        self.wrappers.extend(other.wrappers);
        self.unknown |= other.unknown;
        self.open_load_phase |= other.open_load_phase;
        self.contained.extend(other.contained);
        for (owner, site) in other.mutated_owners {
            self.mutated_owners.entry(owner).or_insert(site);
        }
        for (owner, site) in other.exposed_owners {
            self.exposed_owners.entry(owner).or_insert(site);
        }
    }

    /// Reports whether wrapper uncertainty or exposed namespace mutations invalidate registration trust.
    pub(super) fn invalidated(&self) -> bool {
        self.unknown || self.wrappers.invalidated() || self.exposed_mutation().is_some()
    }

    fn exposed_mutation(&self) -> Option<(&OwnerPath, &(usize, usize))> {
        self.mutated_owners.iter().find(|(owner, _)| {
            self.exposed_owners
                .keys()
                .any(|exposed| owner.0.starts_with(&exposed.0))
        })
    }
}

impl Declarations {
    fn collect(root: Node<'_>, source: &str) -> Self {
        let mut declarations = Self::default();
        let mut nodes = descendants(root);
        nodes.sort_by_key(Node::start_byte);
        for node in nodes {
            if !matches!(node.kind(), "module" | "class") {
                continue;
            }
            let path = declarations.declaration_path(node, source);
            if let Some(path) = &path {
                declarations.known.insert(path.clone());
            }
            declarations.scopes.insert(node.id(), path);
        }
        declarations
    }

    fn declaration_path(&self, node: Node<'_>, source: &str) -> Option<OwnerPath> {
        // Conditional/deferred declarations and unknown ancestry cannot close a namespace.
        let mut parent = node.parent();
        while let Some(scope) = parent {
            if matches!(scope.kind(), "program" | "class" | "module") {
                break;
            }
            if scope.kind() != "body_statement" {
                return None;
            }
            parent = scope.parent();
        }
        if node.child_by_field_name("superclass").is_some() {
            return None;
        }
        let (absolute, parts) = constant_path(node.child_by_field_name("name")?, source)?;
        let candidates = self.lookup_candidates(node, absolute, &parts)?;
        let path = if parts.len() == 1 {
            candidates.into_iter().next()?
        } else {
            // Every prefix must be established before this declaration. In particular, a
            // missing prefix may invoke const_missing and return an entirely different owner.
            let candidate = candidates.into_iter().find(|candidate| {
                let first = candidate.0.len() - parts.len() + 1;
                self.known
                    .contains(&OwnerPath(candidate.0[..first].to_vec()))
            })?;
            // Once the first component resolves, a missing member invokes lookup on that
            // namespace; it does not restart lookup in an outer same-spelled namespace.
            if !(1..candidate.0.len())
                .all(|end| self.known.contains(&OwnerPath(candidate.0[..end].to_vec())))
            {
                return None;
            }
            candidate
        };
        if path.0.first().is_some_and(|name| protected_namespace(name)) {
            return None;
        }
        Some(path)
    }

    fn lookup_candidates(
        &self,
        node: Node<'_>,
        absolute: bool,
        parts: &[String],
    ) -> Option<Vec<OwnerPath>> {
        if absolute {
            return Some(vec![OwnerPath(parts.to_vec())]);
        }
        let mut paths = Vec::new();
        let mut parent = node.parent();
        while let Some(scope) = parent {
            parent = scope.parent();
            if scope.kind() == "singleton_class" {
                return None;
            }
            if matches!(scope.kind(), "module" | "class") {
                let mut path = self.scopes.get(&scope.id())?.as_ref()?.0.clone();
                path.extend_from_slice(parts);
                paths.push(OwnerPath(path));
            }
        }
        paths.push(OwnerPath(parts.to_vec()));
        Some(paths)
    }

    fn owner(&self, node: Node<'_>) -> Option<OwnerPath> {
        let mut parent = node.parent();
        while let Some(scope) = parent {
            if scope.kind() == "singleton_class" {
                return None;
            }
            if matches!(scope.kind(), "module" | "class") {
                return self.scopes.get(&scope.id())?.clone();
            }
            parent = scope.parent();
        }
        None
    }

    fn mutation_owner(&self, node: Node<'_>) -> Option<OwnerPath> {
        let mut parent = node.parent();
        while let Some(scope) = parent {
            if matches!(scope.kind(), "module" | "class") {
                return self.owner(node);
            }
            if !matches!(scope.kind(), "body_statement" | "program") {
                return None;
            }
            parent = scope.parent();
        }
        None
    }
}

/// Cucumber-Ruby implements registration in `Cucumber::Glue`. Reopening it or holding a reference
/// to it can redirect registrations whatever method names are involved, so the protected-name rule
/// used for other runtime owners is not sufficient there. Other owners, including `Object` for
/// top-level helpers, cannot shadow the DSL that `main` extends.
fn changes_registrar_implementation(node: Node<'_>, source: &str) -> bool {
    match node.kind() {
        "class" | "module" | "singleton_class" => {
            node.child_by_field_name("body")
                .is_some_and(|body| body.named_child_count() > 0)
                && within_registrar_implementation(node, &[], source)
        }
        "constant" | "scope_resolution" => {
            !node
                .parent()
                .is_some_and(|parent| parent.kind() == "scope_resolution")
                && !declaration_name(node)
                && constant_path(node, source)
                    .is_some_and(|(_, parts)| within_registrar_implementation(node, &parts, source))
        }
        _ => false,
    }
}

// Relative constant lookup can reach the top-level `Cucumber`, so any declaration spelled from
// `Cucumber` is treated as a possible reopening rather than resolved lexically.
fn within_registrar_implementation(node: Node<'_>, tail: &[String], source: &str) -> bool {
    let mut chain = vec![tail.to_vec()];
    let mut scope = Some(node);
    while let Some(current) = scope {
        let name = match current.kind() {
            "class" | "module" => current.child_by_field_name("name"),
            "singleton_class" => current.child_by_field_name("value"),
            _ => None,
        };
        if let Some((_, parts)) = name.and_then(|name| constant_path(name, source)) {
            chain.push(parts);
        }
        scope = current.parent();
    }
    chain.reverse();
    chain
        .iter()
        .position(|parts| parts.first().is_some_and(|name| name == "Cucumber"))
        .is_some_and(|start| {
            chain[start..]
                .iter()
                .flatten()
                .nth(1)
                .is_some_and(|name| name == "Glue")
        })
}

// Existing runtime owners are globally exposed, even without a reference in selected source.
pub(super) fn protected_namespace(name: &str) -> bool {
    matches!(
        name,
        "Cucumber" | "Object" | "BasicObject" | "Kernel" | "Module" | "Class" | "Method" | "Proc"
    )
}

fn constant_path(mut node: Node<'_>, source: &str) -> Option<(bool, Vec<String>)> {
    let mut parts = Vec::new();
    let absolute = loop {
        if node.kind() == "constant" {
            parts.push(text(node, source).to_owned());
            break false;
        }
        if node.kind() != "scope_resolution" {
            return None;
        }
        let name = node.child_by_field_name("name")?;
        if name.kind() != "constant" {
            return None;
        }
        parts.push(text(name, source).to_owned());
        match node.child_by_field_name("scope") {
            Some(scope) => node = scope,
            None => break true,
        }
    };
    parts.reverse();
    Some((absolute, parts))
}

fn declaration_name(node: Node<'_>) -> bool {
    let mut child = node;
    while let Some(parent) = child.parent() {
        if parent.kind() == "scope_resolution" {
            child = parent;
            continue;
        }
        return matches!(parent.kind(), "module" | "class")
            && parent.child_by_field_name("name") == Some(child);
    }
    false
}

fn world_argument(node: Node<'_>, source: &str) -> bool {
    let mut child = node;
    while let Some(parent) = child.parent() {
        if parent.kind() == "scope_resolution" {
            child = parent;
            continue;
        }
        if parent.kind() != "argument_list" {
            return false;
        }
        let Some(call) = parent.parent() else {
            return false;
        };
        return call.kind() == "call"
            && call.parent().is_some_and(|p| p.kind() == "program")
            && call.child_by_field_name("receiver").is_none()
            && call.child_by_field_name("block").is_none()
            && call
                .child_by_field_name("method")
                .is_some_and(|m| text(m, source) == "World");
    }
    false
}
