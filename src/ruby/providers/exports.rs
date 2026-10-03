//! Source-bound exports proofs for Ruby registration discovery.
use super::*;

impl Graph<'_> {
    pub(super) fn namespace_available(&self, key: &str, unit: usize, node: Node<'_>) -> bool {
        self.namespaces.get(key) == Some(&1)
            && !self.definitions.contains_key(key)
            && self
                .root_modules
                .get(key)
                .is_some_and(|(owner_unit, owner)| {
                    self.available(
                        self.position(unit, node, false),
                        self.position(*owner_unit, *owner, true),
                        &mut BTreeSet::new(),
                    )
                })
    }

    pub(super) fn collect_forwarders(&mut self) {
        let mut candidates = BTreeMap::<(String, String), Vec<String>>::new();
        let mut counts = BTreeMap::<String, usize>::new();
        for (unit, input) in self.units.iter().enumerate() {
            for node in descendants(input.tree.root_node()) {
                if node.kind() == "singleton_method" {
                    if let Some(name) = node.child_by_field_name("name") {
                        *counts
                            .entry(text(name, &input.source).to_owned())
                            .or_default() += 1;
                    }
                }
                if node.kind() != "call"
                    || !node
                        .child_by_field_name("method")
                        .is_some_and(|member| text(member, &input.source) == "call")
                    || !transparent_body(node, &input.source)
                {
                    continue;
                }
                let Some(method) = node.parent().and_then(|p| p.parent()) else {
                    continue;
                };
                if !method
                    .child_by_field_name("object")
                    .or_else(|| method.child_by_field_name("receiver"))
                    .is_some_and(|n| n.kind() == "self")
                {
                    continue;
                }
                let Some(owner) = method
                    .parent()
                    .and_then(|p| p.parent())
                    .filter(|n| n.kind() == "module" && n.parent() == Some(input.tree.root_node()))
                else {
                    continue;
                };
                if descendants(owner).iter().any(|n| {
                    n.kind() == "singleton_class"
                        || (n.kind() == "call"
                            && n.child_by_field_name("method").is_some_and(|m| {
                                super::super::method_table_mutator(text(m, &input.source))
                            }))
                }) {
                    continue;
                }
                let Some(key) = owner
                    .child_by_field_name("name")
                    .and_then(|n| constant(n, &input.source))
                else {
                    continue;
                };
                if self.namespaces.get(&key) != Some(&1)
                    || self.protected_modules.contains(&owner.id())
                {
                    continue;
                }
                let Some(receiver) = node.child_by_field_name("receiver") else {
                    continue;
                };
                let Some(export) = self.reference(receiver, unit) else {
                    continue;
                };
                let Some(registrar) = self.value(&export, unit, receiver, &mut BTreeSet::new())
                else {
                    continue;
                };
                let Some(name) = method.child_by_field_name("name") else {
                    continue;
                };
                candidates
                    .entry((key, text(name, &input.source).to_owned()))
                    .or_default()
                    .push(registrar);
            }
        }
        for (key, registrars) in candidates {
            if registrars.len() == 1 && counts.get(&key.1) == Some(&1) {
                self.forwarders.insert(key, registrars[0].clone());
            }
        }
    }

    pub(super) fn local_export_aliases(
        &self,
        unit: usize,
        nodes: &[Node<'_>],
        references: &BTreeMap<&str, Vec<Node<'_>>>,
        safe: &mut BTreeSet<(usize, usize)>,
        result: &mut Providers,
    ) {
        let input = &self.units[unit];
        let root = input.tree.root_node();
        // Namespace=true; otherwise the value carries a proven registrar identity.
        let mut bindings = BTreeMap::<&str, (Node<'_>, Node<'_>, (bool, String))>::new();
        let mut assignments: Vec<_> = root_assignments(nodes, root).collect();
        assignments.sort_by_key(|(node, _, _)| node.start_byte());
        for (assignment, left, value) in assignments {
            if left.kind() != "identifier" {
                continue;
            }
            let origin = if value.kind() == "identifier" {
                bindings
                    .get(text(value, &input.source))
                    .map(|(_, _, origin)| origin.clone())
            } else if value.kind() == "call"
                && value.child_by_field_name("receiver").is_none()
                && value.child_by_field_name("block").is_none()
                && value
                    .child_by_field_name("method")
                    .is_some_and(|n| text(n, &input.source) == "method")
            {
                value
                    .child_by_field_name("arguments")
                    .filter(|n| n.named_child_count() == 1)
                    .and_then(|n| n.named_child(0))
                    .and_then(|n| static_method_name(n, &input.source))
                    .filter(|name| registration(name))
                    .map(|name| (false, name))
            } else {
                self.reference(value, unit).and_then(|key| {
                    self.value(&key, unit, value, &mut BTreeSet::new())
                        .map(|reg| (false, reg))
                        .or_else(|| {
                            (self.namespace_available(&key, unit, value)
                                && self.forwarders.keys().any(|(owner, _)| owner == &key))
                            .then_some((true, key))
                        })
                })
            };
            if let Some(origin) = origin {
                bindings
                    .entry(text(left, &input.source))
                    .or_insert((assignment, value, origin));
            }
        }
        let mut calls = Vec::new();
        let mut work = 0usize;
        let closed = bindings.iter().all(|(name, (assignment, _, origin))| {
            references.get(name).is_some_and(|refs| {
                refs.iter().all(|reference| {
                    work += 1;
                    if self.proof_budget_exhausted(work) {
                        return false;
                    }
                    if assignment.child_by_field_name("left") == Some(*reference) {
                        return true;
                    }
                    if let Some(alias) = alias_assignment(*reference, *assignment, root) {
                        return alias
                            .child_by_field_name("left")
                            .and_then(|left| bindings.get(text(left, &input.source)))
                            .is_some_and(|(definition, _, target)| {
                                *definition == alias && target == origin
                            });
                    }
                    let Some(call) = reference.parent().filter(|n| {
                        n.kind() == "call"
                            && n.parent() == Some(root)
                            && n.start_byte() >= assignment.end_byte()
                            && n.child_by_field_name("receiver") == Some(*reference)
                    }) else {
                        return false;
                    };
                    let Some(member) = call.child_by_field_name("method") else {
                        return false;
                    };
                    let registrar = if origin.0 {
                        self.forwarders
                            .get(&(origin.1.clone(), text(member, &input.source).to_owned()))
                            .cloned()
                    } else {
                        (text(member, &input.source) == "call").then(|| origin.1.clone())
                    };
                    if let Some(registrar) = registrar {
                        calls.push((call.start_byte(), registrar));
                        true
                    } else {
                        false
                    }
                })
            })
        });
        if closed {
            for (_, value, _) in bindings.values() {
                safe.insert((unit, value.id()));
                if value.kind() == "call" {
                    result
                        .0
                        .entry(input.file.path.clone())
                        .or_default()
                        .captures
                        .insert(value.start_byte());
                }
            }
            result
                .0
                .entry(input.file.path.clone())
                .or_default()
                .calls
                .extend(calls);
        }
    }
}
