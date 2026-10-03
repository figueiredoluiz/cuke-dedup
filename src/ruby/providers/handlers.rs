//! Source-bound handlers proofs for Ruby registration discovery.
use super::*;

type Assignments<'a> = BTreeMap<&'a str, Vec<(Node<'a>, Node<'a>, Node<'a>)>>;
type References<'a> = BTreeMap<&'a str, Vec<Node<'a>>>;

impl Graph<'_> {
    /// Resolves static block conversion without treating arbitrary to_proc implementations as handlers.
    pub(super) fn named_handlers(
        &self,
        result: &mut Providers,
        assertions: Option<&super::super::assertions::AssertionProviders>,
    ) {
        let nodes: Vec<_> = self
            .units
            .iter()
            .map(|input| descendants(input.tree.root_node()))
            .collect();
        let mut methods = BTreeMap::<&str, Vec<(usize, Node<'_>)>>::new();
        for (unit, input) in self.units.iter().enumerate() {
            for node in nodes[unit]
                .iter()
                .filter(|n| n.kind() == "method" && n.parent() == Some(input.tree.root_node()))
            {
                if let Some(name) = node.child_by_field_name("name") {
                    methods
                        .entry(text(name, &input.source))
                        .or_default()
                        .push((unit, *node));
                }
            }
        }
        let mut work = 0usize;
        let mut method_checks = BTreeMap::new();
        let total_nodes: usize = nodes.iter().map(Vec::len).sum();
        for (unit, input) in self.units.iter().enumerate() {
            let root = input.tree.root_node();
            if root.has_error() {
                continue;
            }
            let aliases = super::super::registration_aliases::RegistrationAliases::collect(
                root,
                &input.source,
            );
            work = work.saturating_add(nodes[unit].len());
            if self.proof_budget_exhausted(work) {
                return;
            }
            let mut assignments = Assignments::new();
            for (assignment, left, value) in root_assignments(&nodes[unit], root) {
                if left.kind() == "identifier" {
                    assignments
                        .entry(text(left, &input.source))
                        .or_default()
                        .push((assignment, left, value));
                }
            }
            let references = identifier_references(&nodes[unit], &input.source);
            for call in nodes[unit]
                .iter()
                .filter(|n| n.kind() == "call" && n.parent() == Some(root))
            {
                if !registration_call(
                    *call,
                    root,
                    &input.source,
                    &aliases,
                    result.0.get(&input.file.path),
                ) {
                    continue;
                }
                let Some(argument) = call
                    .child_by_field_name("arguments")
                    .and_then(|n| n.named_child(1))
                    .filter(|n| n.kind() == "block_argument")
                else {
                    continue;
                };
                work = work.saturating_add(1);
                if self.proof_budget_exhausted(work) {
                    return;
                }
                let Some(mut value) = argument.named_child(0) else {
                    continue;
                };
                if value.kind() == "identifier" {
                    let Some(matching) = assignments.get(text(value, &input.source)) else {
                        continue;
                    };
                    if matching.len() != 1 {
                        continue;
                    }
                    let (assignment, left, rhs) = matching[0];
                    work = work.saturating_add(references[text(left, &input.source)].len());
                    if self.proof_budget_exhausted(work) {
                        return;
                    }
                    if assignment.end_byte() > call.start_byte()
                        || !references[text(left, &input.source)]
                            .iter()
                            .all(|reference| {
                                *reference == left
                                    || reference.parent().is_some_and(|a| {
                                        a.kind() == "block_argument"
                                            && a.parent().and_then(|p| p.parent()).is_some_and(
                                                |c| {
                                                    registration_call(
                                                        c,
                                                        root,
                                                        &input.source,
                                                        &aliases,
                                                        result.0.get(&input.file.path),
                                                    ) && c.start_byte() >= assignment.end_byte()
                                                },
                                            )
                                    })
                            })
                    {
                        continue;
                    }
                    value = rhs;
                }
                let body = if value.kind() == "lambda" {
                    Some(value)
                } else if value.kind() == "call"
                    && value.child_by_field_name("receiver").is_none()
                    && value
                        .child_by_field_name("method")
                        .is_some_and(|n| matches!(text(n, &input.source), "proc" | "lambda"))
                    && value
                        .child_by_field_name("arguments")
                        .is_none_or(|n| n.named_child_count() == 0)
                {
                    value.child_by_field_name("block")
                } else {
                    None
                };
                let (body_unit, body) = if let Some(body) = body {
                    (unit, body)
                } else {
                    if value.kind() != "call"
                        || value.child_by_field_name("block").is_some()
                        || !value
                            .child_by_field_name("method")
                            .is_some_and(|n| text(n, &input.source) == "method")
                    {
                        continue;
                    }
                    let Some(args) = value
                        .child_by_field_name("arguments")
                        .filter(|n| n.named_child_count() == 1)
                    else {
                        continue;
                    };
                    let Some(name) = args
                        .named_child(0)
                        .and_then(|n| static_method_name(n, &input.source))
                    else {
                        continue;
                    };
                    let root_capture = value.child_by_field_name("receiver").is_none();
                    let unchanged = *method_checks
                        .entry((name.clone(), root_capture))
                        .or_insert_with(|| {
                            work = work.saturating_add(total_nodes);
                            !self.proof_budget_exhausted(work)
                                && self.handler_method_unchanged(&name, root_capture, &nodes)
                        });
                    if self.proof_budget_exhausted(work) {
                        return;
                    }
                    if !unchanged {
                        continue;
                    }
                    let resolved = if let Some(receiver) = value.child_by_field_name("receiver") {
                        self.bound_instance_method(
                            unit,
                            receiver,
                            &name,
                            (&assignments, &references),
                            &mut work,
                        )
                    } else {
                        methods
                            .get(name.as_str())
                            .filter(|v| v.len() == 1)
                            .map(|v| v[0])
                    };
                    let Some((body_unit, body)) = resolved else {
                        continue;
                    };
                    if !self.available(
                        self.position(unit, value, false),
                        self.position(body_unit, body, true),
                        &mut BTreeSet::new(),
                    ) {
                        continue;
                    }
                    let proof = result.0.entry(input.file.path.clone()).or_default();
                    proof.isolated_calls.insert(value.start_byte());
                    proof.handler_captures.insert(value.start_byte());
                    (body_unit, body)
                };
                let Some(assertions) = assertions else {
                    continue;
                };
                work = work.saturating_add(descendants(body).len());
                if self.proof_budget_exhausted(work) {
                    return;
                }
                let assertion_bindings = assertions.get(
                    &self.units[body_unit].file.path,
                    &self.units[body_unit].source,
                );
                let mut fingerprint = super::super::handler::fingerprint(
                    body,
                    self.units[body_unit].tree.root_node(),
                    &self.units[body_unit].source,
                    assertion_bindings.as_ref(),
                );
                if value.kind() == "lambda"
                    || (value.kind() == "call"
                        && value
                            .child_by_field_name("method")
                            .is_some_and(|name| text(name, &input.source) == "lambda"))
                {
                    for representation in [
                        &mut fingerprint.exact,
                        &mut fingerprint.normalized,
                        &mut fingerprint.alpha_normalized,
                        &mut fingerprint.structural,
                    ] {
                        representation.insert_str(0, "ruby:lambda;");
                    }
                }
                if value.child_by_field_name("receiver").is_some() {
                    fingerprint.comparable = false;
                }
                if body_unit != unit
                    && !super::super::bindings::Bindings::collect(
                        body,
                        self.units[body_unit].tree.root_node(),
                        &self.units[body_unit].source,
                    )
                    .captures
                    .is_empty()
                {
                    fingerprint.comparable = false;
                }
                result
                    .0
                    .entry(input.file.path.clone())
                    .or_default()
                    .handlers
                    .insert(call.start_byte(), fingerprint);
            }
        }
    }

    fn handler_method_unchanged(
        &self,
        target: &str,
        root_capture: bool,
        nodes: &[Vec<Node<'_>>],
    ) -> bool {
        self.units.iter().zip(nodes).all(|(input, nodes)| {
            nodes.iter().all(|node| {
                if root_capture
                    && matches!(node.kind(), "method" | "singleton_method")
                    && node
                        .child_by_field_name("name")
                        .is_some_and(|name| text(name, &input.source) == target)
                    && !(node.kind() == "method" && node.parent() == Some(input.tree.root_node()))
                {
                    return false;
                }
                if matches!(node.kind(), "alias" | "undef") {
                    return !descendants(*node).iter().any(|name| {
                        text(*name, &input.source) == target
                            || static_method_name(*name, &input.source).as_deref() == Some(target)
                    });
                }
                if node.kind() != "call"
                    || !node.child_by_field_name("method").is_some_and(|name| {
                        super::super::method_table_mutator(text(name, &input.source))
                            || matches!(
                                text(name, &input.source),
                                "send" | "public_send" | "__send__"
                            )
                    })
                {
                    return true;
                }
                !node.child_by_field_name("arguments").is_some_and(|args| {
                    descendants(args).iter().any(|name| {
                        static_method_name(*name, &input.source).as_deref() == Some(target)
                    })
                })
            })
        })
    }

    fn bound_instance_method<'a>(
        &'a self,
        unit: usize,
        mut receiver: Node<'a>,
        target: &str,
        (assignments, references): (&Assignments<'a>, &References<'a>),
        work: &mut usize,
    ) -> Option<(usize, Node<'a>)> {
        if self.custom_allocation {
            return None;
        }
        let input = &self.units[unit];
        if receiver.kind() == "identifier" {
            let matching = assignments.get(text(receiver, &input.source))?;
            if matching.len() != 1 {
                return None;
            }
            let (assignment, left, value) = matching[0];
            if assignment.end_byte() > receiver.start_byte() {
                return None;
            }
            if !references[text(left, &input.source)]
                .iter()
                .all(|reference| {
                    *reference == left
                        || reference.parent().is_some_and(|call| {
                            call.kind() == "call"
                                && call.child_by_field_name("receiver") == Some(*reference)
                                && call
                                    .child_by_field_name("method")
                                    .is_some_and(|n| text(n, &input.source) == "method")
                        })
                })
            {
                return None;
            }
            receiver = value;
        }
        if receiver.kind() != "call"
            || receiver.child_by_field_name("block").is_some()
            || !receiver
                .child_by_field_name("method")
                .is_some_and(|n| text(n, &input.source) == "new")
        {
            return None;
        }
        let key = self.reference(receiver.child_by_field_name("receiver")?, unit)?;
        if self.definitions.contains_key(&key) || super::super::ownership::protected_namespace(&key)
        {
            return None;
        }
        let classes = self.classes.get(&key)?;
        if classes.len() != 1 {
            return None;
        }
        let (owner_unit, class) = classes[0];
        let source = &self.units[owner_unit];
        if class.parent() != Some(source.tree.root_node())
            || class.child_by_field_name("superclass").is_some()
            || !self.available(
                self.position(unit, receiver, false),
                self.position(owner_unit, class, true),
                &mut BTreeSet::new(),
            )
        {
            return None;
        }
        let nodes = descendants(class);
        *work = work.saturating_add(nodes.len());
        if self.proof_budget_exhausted(*work) {
            return None;
        }
        if nodes.iter().any(|n| {
            matches!(n.kind(), "singleton_class" | "alias" | "undef")
                || (matches!(n.kind(), "method" | "singleton_method")
                    && n.child_by_field_name("name").is_some_and(|m| {
                        matches!(
                            text(m, &source.source),
                            "method" | "public_method" | "method_missing"
                        )
                    }))
        }) {
            return None;
        }
        let bodies: Vec<_> = nodes
            .into_iter()
            .filter(|n| {
                n.kind() == "method"
                    && n.parent().and_then(|p| p.parent()) == Some(class)
                    && n.child_by_field_name("name")
                        .is_some_and(|m| text(m, &source.source) == target)
            })
            .collect();
        (bodies.len() == 1).then(|| (owner_unit, bodies[0]))
    }
}

fn registration_call(
    call: Node<'_>,
    root: Node<'_>,
    source: &str,
    aliases: &super::super::registration_aliases::RegistrationAliases,
    proof: Option<&Proof>,
) -> bool {
    call.parent() == Some(root)
        && ((call.child_by_field_name("receiver").is_none()
            && call
                .child_by_field_name("method")
                .is_some_and(|name| registration(text(name, source))))
            || aliases.registration(call).is_some()
            || proof.is_some_and(|p| p.calls.contains_key(&call.start_byte())))
}
