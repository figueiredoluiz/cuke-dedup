//! Source-bound owners proofs for Ruby registration discovery.
use super::*;

impl Graph<'_> {
    /// Proves closed constructor bindings without treating their methods as framework registrations.
    pub(super) fn isolated_instances(&self, result: &mut Providers) {
        if self.custom_allocation {
            return;
        }
        let mut classes = BTreeMap::<String, Vec<(usize, Node<'_>, BTreeSet<&str>)>>::new();
        for (unit, input) in self.units.iter().enumerate() {
            for node in descendants(input.tree.root_node()) {
                // Inventory every declaration before considering constructor eligibility.
                if node.kind() == "class" {
                    if let Some(name) = node
                        .child_by_field_name("name")
                        .and_then(|n| constant(n, &input.source))
                    {
                        classes.entry(name).or_default().push((
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
                if super::super::ownership::protected_namespace(name)
                    || self.definitions.contains_key(name)
                {
                    continue;
                }
                let Some(declarations) = classes.get(name).filter(|v| v.len() == 1) else {
                    continue;
                };
                let (class_unit, class, methods) = &declarations[0];
                if class.parent() != Some(self.units[*class_unit].tree.root_node())
                    || class.child_by_field_name("superclass").is_some()
                    || !class
                        .child_by_field_name("name")
                        .is_some_and(|n| n.kind() == "constant")
                {
                    continue;
                }
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

    /// Distinguishes source-bound namespace calls from escaped owner values.
    /// These offsets remove registry hazards only; they never grant assertion trust.
    pub(super) fn isolated_namespaces(&self, result: &mut Providers) {
        let mut owners = BTreeMap::<String, Vec<(usize, Node<'_>)>>::new();
        for (unit, input) in self.units.iter().enumerate() {
            for node in descendants(input.tree.root_node()) {
                if matches!(node.kind(), "module" | "class") {
                    if let Some(key) = node
                        .child_by_field_name("name")
                        .and_then(|n| qualified(n, &input.source))
                    {
                        owners.entry(key).or_default().push((unit, node));
                    }
                }
            }
        }
        let custom_allocation = self.custom_allocation;
        let mut summaries = BTreeMap::new();
        let mut work = 0usize;
        for (key, declarations) in &owners {
            if declarations.len() != 1 {
                continue;
            }
            let (unit, owner) = declarations[0];
            let source = &self.units[unit].source;
            let nodes = descendants(owner);
            work = work.saturating_add(nodes.len());
            if self.proof_budget_exhausted(work) {
                return;
            }
            let mut prefix = String::new();
            let closed = key.split("::").all(|part| {
                if !prefix.is_empty() {
                    prefix.push_str("::");
                }
                prefix.push_str(part);
                !super::super::ownership::protected_namespace(part)
                    && !self.definitions.contains_key(&prefix)
                    && owners.get(&prefix).is_some_and(|v| {
                        v.len() == 1
                            && !self.units[v[0].0].tree.root_node().has_error()
                            && v[0].1.child_by_field_name("superclass").is_none()
                            && std::iter::successors(v[0].1.parent(), |n| n.parent()).all(|n| {
                                matches!(
                                    n.kind(),
                                    "program" | "body_statement" | "class" | "module"
                                )
                            })
                    })
            });
            let hooks = nodes.iter().any(|n| {
                n.kind() == "singleton_class"
                    || (matches!(n.kind(), "method" | "singleton_method")
                        && n.child_by_field_name("name").is_some_and(|m| {
                            matches!(
                                text(m, source),
                                "new"
                                    | "allocate"
                                    | "method"
                                    | "public_method"
                                    | "method_missing"
                                    | "const_missing"
                            )
                        }))
            });
            if closed && !hooks {
                let members: BTreeSet<_> = nodes
                    .iter()
                    .filter(|n| {
                        n.kind() == "singleton_method"
                            && n.parent().and_then(|p| p.parent()) == Some(owner)
                            && n.child_by_field_name("object")
                                .or_else(|| n.child_by_field_name("receiver"))
                                .is_some_and(|r| r.kind() == "self")
                    })
                    .filter_map(|n| n.child_by_field_name("name"))
                    .map(|n| text(n, source))
                    .collect();
                summaries.insert(key, members);
            }
        }
        // Local namespace aliases are safe only as closed receiver references, never as values
        // passed to unknown code. Keep this proof separate from registration export identity.
        for (unit, input) in self.units.iter().enumerate() {
            let nodes = descendants(input.tree.root_node());
            let references = identifier_references(&nodes, &input.source);
            let mut bindings = BTreeMap::<&str, (Node<'_>, Node<'_>, String)>::new();
            let mut assignments: Vec<_> =
                root_assignments(&nodes, input.tree.root_node()).collect();
            assignments.sort_by_key(|(n, _, _)| n.start_byte());
            for (assignment, left, value) in assignments {
                if left.kind() != "identifier" {
                    continue;
                }
                let key = if value.kind() == "identifier" {
                    bindings
                        .get(text(value, &input.source))
                        .map(|(_, _, key)| key.clone())
                } else {
                    self.reference(value, unit)
                        .filter(|key| summaries.contains_key(key))
                };
                if let Some(key) = key {
                    if self.namespace_available(&key, unit, value) {
                        bindings
                            .entry(text(left, &input.source))
                            .or_insert((assignment, value, key));
                    }
                }
            }
            let closed = bindings.iter().all(|(name, (assignment, _, key))| {
                references.get(name).is_some_and(|refs| {
                    refs.iter().all(|reference| {
                        if assignment.child_by_field_name("left") == Some(*reference) {
                            return true;
                        }
                        if let Some(alias) =
                            alias_assignment(*reference, *assignment, input.tree.root_node())
                        {
                            return alias
                                .child_by_field_name("left")
                                .and_then(|left| bindings.get(text(left, &input.source)))
                                .is_some_and(|(definition, _, target)| {
                                    *definition == alias && target == key
                                });
                        }
                        reference.parent().is_some_and(|call| {
                            call.kind() == "call"
                                && call.start_byte() >= assignment.end_byte()
                                && call.child_by_field_name("receiver") == Some(*reference)
                                && call.child_by_field_name("method").is_some_and(|member| {
                                    summaries[key].contains(text(member, &input.source))
                                })
                        })
                    })
                })
            });
            if closed {
                let proof = result.0.entry(input.file.path.clone()).or_default();
                proof
                    .isolated_constants
                    .extend(bindings.values().map(|(_, value, _)| value.start_byte()));
            }
        }
        for (unit, input) in self.units.iter().enumerate() {
            for call in descendants(input.tree.root_node())
                .into_iter()
                .filter(|n| n.kind() == "call")
            {
                let Some(method) = call.child_by_field_name("method") else {
                    continue;
                };
                let name = text(method, &input.source);
                let receiver = call.child_by_field_name("receiver");
                let key = receiver.and_then(|n| self.reference(n, unit)).or_else(|| {
                    if receiver.is_some() {
                        return None;
                    }
                    let owner = std::iter::successors(call.parent(), |n| n.parent())
                        .find(|n| matches!(n.kind(), "class" | "module"))?;
                    qualified(owner.child_by_field_name("name")?, &input.source)
                });
                let Some(key) = key else { continue };
                let Some(declarations) = owners.get(&key).filter(|v| v.len() == 1) else {
                    continue;
                };
                let (owner_unit, owner) = declarations[0];
                let Some(members) = summaries.get(&key) else {
                    continue;
                };
                let capture = matches!(name, "method" | "public_method");
                let target = if capture {
                    let Some(args) = call
                        .child_by_field_name("arguments")
                        .filter(|n| n.named_child_count() == 1)
                    else {
                        continue;
                    };
                    let Some(target) = args
                        .named_child(0)
                        .and_then(|n| static_method_name(n, &input.source))
                    else {
                        continue;
                    };
                    target
                } else {
                    name.to_owned()
                };
                let constructor = name == "new" && owner.kind() == "class";
                if constructor && custom_allocation {
                    continue;
                }
                if !constructor
                    && (super::super::protected_method(&target)
                        || !members.contains(target.as_str()))
                {
                    continue;
                }
                // Deferred references require a preceding enclosing declaration; external calls
                // require the complete owner to have loaded before the call site.
                let within = unit == owner_unit
                    && call.start_byte() > owner.start_byte()
                    && call.end_byte() <= owner.end_byte();
                if !within
                    && !self.available(
                        self.position(unit, call, false),
                        self.position(owner_unit, owner, true),
                        &mut BTreeSet::new(),
                    )
                {
                    continue;
                }
                let proof = result.0.entry(input.file.path.clone()).or_default();
                if let Some(receiver) = receiver {
                    proof.isolated_constants.insert(receiver.start_byte());
                }
                if capture {
                    proof.isolated_calls.insert(call.start_byte());
                }
            }
        }
    }
}
