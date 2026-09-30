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
    unknown: bool,
    mutated_owners: BTreeSet<OwnerPath>,
    exposed_owners: BTreeSet<OwnerPath>,
}

impl RegistrationEffects {
    pub(super) fn collect(
        root: Node<'_>,
        source: &str,
        aliases: &super::registration_aliases::RegistrationAliases,
    ) -> Self {
        let mut effects = Self::default();
        let declarations = Declarations::collect(root, source);
        for node in descendants(root) {
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
                effects.unknown = true;
            }
            if matches!(node.kind(), "constant" | "scope_resolution")
                && !node
                    .parent()
                    .is_some_and(|p| p.kind() == "scope_resolution")
                && !declaration_name(node)
                && !world_argument(node, source)
            {
                if let Some((absolute, parts)) = constant_path(node, source) {
                    if let Some(paths) = declarations.lookup_candidates(node, absolute, &parts) {
                        effects.exposed_owners.extend(paths);
                    } else {
                        // An unresolved lexical scope cannot provide a disjointness proof.
                        effects.unknown = true;
                    }
                } else {
                    effects.unknown = true;
                }
            }
            // Passing the lexical owner to arbitrary code can install or alias its method table.
            // Keep this conservative even in deferred bodies until their effects are modeled.
            if node.kind() == "self" {
                if let Some(owner) = declarations.owner(node) {
                    effects.exposed_owners.insert(owner);
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
                "method" | "singleton_method" => node
                    .child_by_field_name("name")
                    .is_some_and(|name| protected_method(text(name, source))),
                "call" => {
                    !aliases.capture(node)
                        && !inside_deferred_body(node, source, aliases)
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
            if !explicit_receiver && node.kind() != "call" {
                if let Some(owner) = declarations.mutation_owner(node) {
                    effects.mutated_owners.insert(owner);
                    continue;
                }
            }
            effects.unknown = true;
        }
        effects
    }

    pub(super) fn invalidate(&mut self) {
        self.unknown = true;
    }

    pub(super) fn extend(&mut self, other: Self) {
        self.unknown |= other.unknown;
        self.mutated_owners.extend(other.mutated_owners);
        self.exposed_owners.extend(other.exposed_owners);
    }

    pub(super) fn invalidated(&self) -> bool {
        self.unknown
            || self.mutated_owners.iter().any(|owner| {
                self.exposed_owners
                    .iter()
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
        if path.0.first().is_some_and(|name| {
            matches!(
                name.as_str(),
                "Cucumber" | "Object" | "BasicObject" | "Kernel" | "Module" | "Class" | "Method"
            )
        }) {
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
