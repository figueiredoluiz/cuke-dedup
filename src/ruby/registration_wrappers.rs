//! Proof of transparent local forwarding, separate from selected-source method effects.
use super::{descendants, protected_method, registration, static_method_name, text};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

type Site = (usize, usize);

#[derive(Clone, Default)]
pub(super) struct WrapperEffects {
    wrappers: BTreeSet<String>,
    definitions: BTreeMap<String, usize>,
    unresolved: BTreeSet<String>,
    mutations: BTreeSet<String>,
    dynamic_mutation: bool,
    // Source byte ranges of this source's evidence, per name. They are not merged across
    // sources: only a single source's effects locate its diagnostic.
    definition_sites: BTreeMap<String, Site>,
    unresolved_sites: BTreeMap<String, Site>,
    mutation_sites: BTreeMap<String, Site>,
    dynamic_site: Option<Site>,
}

fn site(node: Node<'_>) -> Site {
    (node.start_byte(), node.end_byte())
}

impl WrapperEffects {
    /// Merges selected-source evidence without losing earlier mutations or unresolved references.
    pub(super) fn extend(&mut self, other: Self) {
        self.wrappers.extend(other.wrappers);
        for (name, count) in other.definitions {
            *self.definitions.entry(name).or_default() += count;
        }
        self.unresolved.extend(other.unresolved);
        self.mutations.extend(other.mutations);
        self.dynamic_mutation |= other.dynamic_mutation;
    }

    /// Rejects wrapper trust when aggregated evidence cannot prove a single unchanged definition.
    pub(super) fn invalidated(&self) -> bool {
        self.invalidating_wrapper().is_some()
    }

    fn invalidating_wrapper(&self) -> Option<&String> {
        self.wrappers.iter().find(|name| {
            self.dynamic_mutation
                || self.definitions.get(*name) != Some(&1)
                || self.unresolved.contains(*name)
                || self.mutations.contains(*name)
        })
    }

    /// Locates this source's evidence for the first wrapper whose trust is withdrawn.
    pub(super) fn invalidation_site(&self) -> Option<Site> {
        let name = self.invalidating_wrapper()?;
        self.dynamic_site
            .or_else(|| self.mutation_sites.get(name).copied())
            .or_else(|| self.unresolved_sites.get(name).copied())
            .or_else(|| self.definition_sites.get(name).copied())
    }

    fn mutation(&mut self, name: Option<String>, node: Node<'_>) {
        match name {
            Some(name) => {
                self.mutation_sites
                    .entry(name.clone())
                    .or_insert(site(node));
                self.mutations.insert(name);
            }
            None => self.dynamic(node),
        }
    }

    fn dynamic(&mut self, node: Node<'_>) {
        self.dynamic_mutation = true;
        self.dynamic_site.get_or_insert(site(node));
    }

    fn unresolved(&mut self, name: &str, node: Node<'_>) {
        self.unresolved_sites
            .entry(name.to_owned())
            .or_insert(site(node));
        self.unresolved.insert(name.to_owned());
    }
}

#[derive(Default)]
pub(super) struct RegistrationWrappers {
    calls: BTreeMap<usize, String>,
    forwarding: BTreeSet<usize>,
    pub(super) effects: WrapperEffects,
}

impl RegistrationWrappers {
    /// Proves local forwarding shapes and records effects for subsequent suite-wide ownership checks.
    pub(super) fn collect(
        root: Node<'_>,
        source: &str,
        aliases: &super::registration_aliases::RegistrationAliases,
    ) -> Self {
        let mut result = Self::default();
        let nodes = descendants(root);
        let mut declarations = BTreeMap::new();
        for node in &nodes {
            if matches!(node.kind(), "method" | "singleton_method") {
                if let Some(name) = node.child_by_field_name("name") {
                    let name = text(name, source).to_owned();
                    // The latest declaration is the one that makes a repeated name ambiguous.
                    let latest = result
                        .effects
                        .definition_sites
                        .entry(name.clone())
                        .or_insert(site(*node));
                    *latest = (*latest).max(site(*node));
                    *result.effects.definitions.entry(name).or_default() += 1;
                }
            }
            if !root.has_error() && node.kind() == "method" && node.parent() == Some(root) {
                if let Some((name, registrar, forwarding)) = transparent(*node, source) {
                    declarations.insert(name.to_owned(), (node.end_byte(), registrar.to_owned()));
                    result.forwarding.insert(forwarding.id());
                    result.effects.wrappers.insert(name.to_owned());
                }
            }
        }
        for node in nodes.iter().copied() {
            if node.kind() == "call" {
                let Some(method) = node.child_by_field_name("method") else {
                    continue;
                };
                let name = text(method, source);
                if let Some((end, registrar)) = declarations.get(name) {
                    if node.parent() == Some(root)
                        && node.start_byte() >= *end
                        && node.child_by_field_name("receiver").is_none()
                    {
                        result.calls.insert(node.id(), registrar.clone());
                    } else {
                        result.effects.unresolved(name, node);
                    }
                } else {
                    // Another selected source may declare a wrapper with this name.
                    result.effects.unresolved(name, node);
                }
            } else if matches!(node.kind(), "alias" | "undef") {
                for name in children(node) {
                    let name = if name.kind() == "identifier" {
                        Some(text(name, source).to_owned())
                    } else {
                        static_method_name(name, source)
                    };
                    result.effects.mutation(name, node);
                }
            } else if node.kind() == "identifier"
                && !node.parent().is_some_and(|parent| {
                    (matches!(parent.kind(), "method" | "singleton_method")
                        && parent.child_by_field_name("name") == Some(node))
                        || (parent.kind() == "call"
                            && parent.child_by_field_name("method") == Some(node))
                })
            {
                // Bare calls, local shadowing and references outside the resolved invocation
                // set cannot establish which method will run.
                result.effects.unresolved(text(node, source), node);
            }
        }
        for node in nodes {
            if node.kind() == "call" && !super::inside_deferred_body(node, source, aliases, &result)
            {
                result.effects.reflective_mutation(node, source);
            }
        }
        result
    }

    /// Returns the locally resolved registrar; callers must also validate aggregate ownership.
    pub(super) fn registration(&self, node: Node<'_>) -> Option<&str> {
        self.calls.get(&node.id()).map(String::as_str)
    }

    /// Identifies a proven forwarding body, which is not itself an executed registration.
    pub(super) fn forwarding(&self, node: Node<'_>) -> bool {
        self.forwarding.contains(&node.id())
    }
}

/// Keeps semantic children in source order while excluding comments and empty statements.
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|node| !matches!(node.kind(), "comment" | "empty_statement"))
        .collect()
}

/// Accepts only a method body that forwards its distinct matcher and block parameters unchanged.
fn transparent<'a>(node: Node<'a>, source: &'a str) -> Option<(&'a str, &'a str, Node<'a>)> {
    let name = text(node.child_by_field_name("name")?, source);
    if protected_method(name) {
        return None;
    }
    let parameters = children(node.child_by_field_name("parameters")?);
    if parameters.len() != 2
        || parameters[0].kind() != "identifier"
        || parameters[1].kind() != "block_parameter"
    {
        return None;
    }
    let block_name = parameters[1].child_by_field_name("name")?;
    if text(parameters[0], source) == text(block_name, source) {
        return None;
    }
    let mut body = node.child_by_field_name("body")?;
    if body.kind() == "body_statement" {
        let statements = children(body);
        if statements.len() != 1 {
            return None;
        }
        body = statements[0];
    }
    if body.kind() != "call"
        || body.child_by_field_name("receiver").is_some()
        || body.child_by_field_name("block").is_some()
    {
        return None;
    }
    let registrar = text(body.child_by_field_name("method")?, source);
    let arguments = children(body.child_by_field_name("arguments")?);
    if !registration(registrar)
        || arguments.len() != 2
        || arguments[0].kind() != "identifier"
        || text(arguments[0], source) != text(parameters[0], source)
        || arguments[1].kind() != "block_argument"
    {
        return None;
    }
    let forwarded = children(arguments[1]);
    if forwarded.len() != 1
        || forwarded[0].kind() != "identifier"
        || text(forwarded[0], source) != text(block_name, source)
    {
        return None;
    }
    Some((name, registrar, body))
}

impl WrapperEffects {
    /// Records literal reflective mutation targets, treating unresolved dispatch or targets as unknown.
    fn reflective_mutation(&mut self, node: Node<'_>, source: &str) {
        let Some(method) = node.child_by_field_name("method") else {
            return;
        };
        let mut name = text(method, source).to_owned();
        let mut arguments = node
            .child_by_field_name("arguments")
            .map(children)
            .unwrap_or_default()
            .into_iter();
        while matches!(name.as_str(), "send" | "public_send" | "__send__") {
            let Some(target) = arguments
                .next()
                .and_then(|arg| static_method_name(arg, source))
            else {
                self.dynamic(node);
                return;
            };
            name = target;
        }
        let count = match name.as_str() {
            "define_method" | "define_singleton_method" => 1,
            "alias_method" => 2,
            "undef_method" | "remove_method" | "attr_reader" | "attr_writer" | "attr_accessor" => {
                usize::MAX
            }
            _ => return,
        };
        for argument in arguments.take(count) {
            self.mutation(static_method_name(argument, source), node);
        }
    }
}
