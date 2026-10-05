//! Source-order local bindings for handler identity; names never establish framework trust.
use super::{descendants, literal_string, text};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

#[derive(Default)]
pub(super) struct Bindings {
    pub names: BTreeMap<usize, String>,
    pub captures: BTreeSet<String>,
    /// A capture whose identity holds only inside its file.
    pub lexical: bool,
    pub calls: BTreeSet<usize>,
    pub uncertain: bool,
    /// Lambda literals assigned to handler locals: syntax, not behaviour.
    pub elided: BTreeSet<usize>,
    /// Zero-argument invocations whose single declaration executes in place.
    pub expansions: BTreeMap<usize, usize>,
    /// Invocations of parameterized declared lambdas; the body's values stay unproven.
    pub unresolved_invocations: BTreeSet<usize>,
}

const WALLS: [&str; 5] = [
    "method",
    "singleton_method",
    "class",
    "module",
    "singleton_class",
];

fn scope_node(node: Node<'_>) -> bool {
    matches!(node.kind(), "block" | "do_block" | "lambda")
}

/// The statements a block, lambda or method scope executes.
fn scope_body(scope: Node<'_>) -> Option<Node<'_>> {
    let body = scope.child_by_field_name("body")?;
    if scope.kind() == "lambda" {
        body.child_by_field_name("body")
    } else {
        Some(body)
    }
}

fn parameter_names(scope: Node<'_>, source: &str) -> BTreeSet<String> {
    let Some(parameters) = scope.child_by_field_name("parameters") else {
        return BTreeSet::new();
    };
    binding_nodes(parameters)
        .iter()
        .map(|name| text(*name, source).to_owned())
        .collect()
}

fn scalar_literal(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "integer" | "float" | "simple_symbol" | "true" | "false" | "nil"
    )
}

fn string_literal(node: Node<'_>, source: &str) -> Option<String> {
    (node.kind() == "string"
        && !descendants(node)
            .iter()
            .any(|part| part.kind() == "interpolation"))
    .then(|| literal_string(text(node, source)))
    .flatten()
}

/// A `->`, `lambda` or `proc` literal; the returned node owns the whole callable syntax.
fn callable_literal<'a>(node: Node<'a>, source: &str) -> Option<Node<'a>> {
    if node.kind() == "lambda" {
        return Some(node);
    }
    (node.kind() == "call"
        && node.child_by_field_name("receiver").is_none()
        && node.child_by_field_name("arguments").is_none()
        && node.child_by_field_name("block").is_some()
        && node
            .child_by_field_name("method")
            .is_some_and(|method| matches!(text(method, source), "lambda" | "proc")))
    .then_some(node)
}

fn callable_parameterized(callable: Node<'_>) -> bool {
    let owner = if callable.kind() == "lambda" {
        Some(callable)
    } else {
        callable.child_by_field_name("block")
    };
    owner.is_some_and(|owner| {
        owner
            .child_by_field_name("parameters")
            .is_some_and(|parameters| parameters.named_child_count() != 0)
    })
}

/// The statements a declared callable runs when invoked.
pub(super) fn callable_body(callable: Node<'_>) -> Option<Node<'_>> {
    if callable.kind() == "lambda" {
        scope_body(callable)
    } else {
        callable
            .child_by_field_name("block")
            .and_then(|block| block.child_by_field_name("body"))
    }
}

/// A `.call(...)` or `.()` dispatch on the binding, through transparent parentheses.
fn invocation<'a>(read: Node<'a>, source: &str) -> Option<(Node<'a>, usize)> {
    let mut value = read;
    while let Some(parent) = value
        .parent()
        .filter(|node| node.kind() == "parenthesized_statements" && node.named_child_count() == 1)
    {
        value = parent;
    }
    let call = value.parent().filter(|call| {
        call.kind() == "call"
            && call.child_by_field_name("receiver") == Some(value)
            && call.child_by_field_name("block").is_none()
            && call
                .child_by_field_name("method")
                .is_none_or(|method| text(method, source) == "call")
    })?;
    let arity = call
        .child_by_field_name("arguments")
        .map_or(0, |arguments| {
            let mut cursor = arguments.walk();
            arguments
                .named_children(&mut cursor)
                .filter(|node| !node.is_extra())
                .count()
        });
    Some((call, arity))
}

fn frozen_string_literals(root: Node<'_>, source: &str) -> bool {
    let mut cursor = root.walk();
    let leading: Vec<_> = root
        .children(&mut cursor)
        .take_while(|node| node.kind() == "comment")
        .collect();
    leading.iter().any(|comment| {
        let body = text(*comment, source).trim_start_matches('#').trim();
        body.strip_prefix("frozen_string_literal:")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
    })
}

/// Values of constants assigned exactly once at program level with an immutable literal.
fn file_constants(root: Node<'_>, source: &str) -> BTreeMap<String, String> {
    let nodes = descendants(root);
    if nodes.iter().any(|node| {
        node.kind() == "call"
            && node.child_by_field_name("method").is_some_and(|method| {
                matches!(
                    text(method, source),
                    "const_set" | "remove_const" | "const_missing"
                )
            })
    }) {
        return BTreeMap::new();
    }
    let frozen = frozen_string_literals(root, source);
    let mut writes = BTreeMap::<String, Vec<Node<'_>>>::new();
    for node in &nodes {
        if !matches!(node.kind(), "assignment" | "operator_assignment") {
            continue;
        }
        let Some(left) = node.child_by_field_name("left") else {
            continue;
        };
        let targets = if left.kind() == "constant" {
            vec![left]
        } else {
            descendants(left)
                .into_iter()
                .filter(|part| part.kind() == "constant")
                .collect()
        };
        for target in targets {
            writes
                .entry(text(target, source).to_owned())
                .or_default()
                .push(*node);
        }
    }
    writes
        .into_iter()
        .filter_map(|(name, writes)| {
            let [write] = writes.as_slice() else {
                return None;
            };
            if write.kind() != "assignment"
                || write.parent() != Some(root)
                || write
                    .child_by_field_name("left")
                    .is_none_or(|left| left.kind() != "constant")
            {
                return None;
            }
            constant_value(write.child_by_field_name("right")?, source, frozen)
                .map(|value| (name, value))
        })
        .collect()
}

fn constant_value(right: Node<'_>, source: &str, frozen: bool) -> Option<String> {
    if scalar_literal(right) {
        return Some(format!("const:{}:{}", right.kind(), text(right, source)));
    }
    let string = if right.kind() == "string" {
        frozen.then(|| string_literal(right, source)).flatten()
    } else {
        (right.kind() == "call"
            && right.child_by_field_name("arguments").is_none()
            && right.child_by_field_name("block").is_none()
            && right
                .child_by_field_name("method")
                .is_some_and(|method| text(method, source) == "freeze"))
        .then(|| string_literal(right.child_by_field_name("receiver")?, source))
        .flatten()
    };
    string.map(|value| format!("const:string:{value}"))
}

/// Whether a constant node reads a value rather than naming a method, scope or target.
fn constant_read(node: Node<'_>) -> bool {
    node.parent().is_none_or(|parent| {
        !(matches!(
            parent.kind(),
            "scope_resolution" | "class" | "module" | "singleton_class"
        ) || (parent.kind() == "call" && parent.child_by_field_name("method") == Some(node))
            || (matches!(parent.kind(), "assignment" | "operator_assignment")
                && parent.child_by_field_name("left") == Some(node)))
    })
}

/// Locals assigned in `scope` before `before`, outside nested scopes; flags unresolved syntax.
fn locals_before(
    scope: Node<'_>,
    before: Node<'_>,
    source: &str,
    uncertain: &mut bool,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut pending = vec![scope];
    while let Some(node) = pending.pop() {
        if node.start_byte() >= before.start_byte() {
            continue;
        }
        if node != scope && (scope_node(node) || WALLS.contains(&node.kind())) {
            continue;
        }
        match node.kind() {
            "for" | "rescue" | "in_clause" | "match_pattern" | "test_pattern" => {
                *uncertain = true;
            }
            "binary"
                if node
                    .child_by_field_name("operator")
                    .is_some_and(|n| text(n, source) == "=~") =>
            {
                *uncertain = true;
            }
            "assignment" | "operator_assignment" => {
                if let Some(left) = node.child_by_field_name("left") {
                    names.extend(
                        binding_nodes(left)
                            .iter()
                            .map(|n| text(*n, source).to_owned()),
                    );
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    names
}

impl Bindings {
    /// Tracks parser-order bindings while keeping unresolved reflection and implicit parameters uncertain.
    pub fn collect(block: Node<'_>, root: Node<'_>, source: &str) -> Self {
        Self::collect_with_origins(block, root, source, &BTreeMap::new(), &BTreeSet::new())
    }

    /// Resolves handler bindings with source-proven captures and lexical shadowing.
    pub(super) fn collect_with_origins(
        block: Node<'_>,
        root: Node<'_>,
        source: &str,
        captures: &BTreeMap<String, String>,
        dispatch: &BTreeSet<(usize, usize)>,
    ) -> Self {
        let mut result = Self::default();
        // Ruby def opens a new local scope; it does not close over the file's locals.
        let closure = block.kind() != "method";
        let mut outer = locals_before(root, block, source, &mut result.uncertain);
        if !closure {
            outer.clear();
        }
        let mut enclosing = Vec::new();
        let mut ancestor = block.parent();
        while let (Some(node), true) = (ancestor, closure) {
            if scope_node(node)
                && !node
                    .parent()
                    .is_some_and(|parent| parent.kind() == "lambda")
            {
                enclosing.push(node);
            } else if WALLS.contains(&node.kind()) {
                break;
            }
            ancestor = node.parent();
        }
        // Fingerprinted blocks are top-level or inside executed lambdas, so constant lookup
        // never crosses a class or module body; `def` handlers keep names.
        let constants = if closure {
            file_constants(root, source)
        } else {
            BTreeMap::new()
        };
        let mut scopes = vec![outer
            .into_iter()
            .map(|name| (name.clone(), format!("capture:{name}")))
            .collect::<BTreeMap<_, _>>()];
        let mut enclosing_scopes = Vec::new();
        for scope in enclosing.iter().rev() {
            // An assignment creates a scope-local only when no outer local already owns the
            // name; parameters and block-locals rebind it regardless.
            let visible: BTreeSet<_> = scopes.iter().flat_map(BTreeMap::keys).cloned().collect();
            let parameters = parameter_names(*scope, source);
            let mut names = parameters.clone();
            // The handler sits inside this scope, so the scope has a body.
            names.extend(
                scope_body(*scope)
                    .map_or_else(BTreeSet::new, |body| {
                        locals_before(body, block, source, &mut result.uncertain)
                    })
                    .into_iter()
                    .filter(|name| !visible.contains(name)),
            );
            scopes.push(
                names
                    .iter()
                    .map(|name| (name.clone(), format!("scope:{}:{name}", scope.start_byte())))
                    .collect(),
            );
            enclosing_scopes.push((*scope, parameters));
        }
        let mut writes = BTreeMap::<String, Vec<Node<'_>>>::new();
        let mut reads = BTreeMap::<String, Vec<Node<'_>>>::new();
        let mut next = 0;
        let mut pending = vec![(block, false)];
        while let Some((node, closing)) = pending.pop() {
            let scope = scope_node(node) || (node == block && node.kind() == "method");
            if closing {
                if scope {
                    scopes.pop();
                }
                continue;
            }
            if scope {
                scopes.push(BTreeMap::new());
                if let Some(parameters) = node.child_by_field_name("parameters") {
                    for name in binding_nodes(parameters) {
                        let key = format!("local:{next}");
                        next += 1;
                        scopes
                            .last_mut()
                            .unwrap()
                            .insert(text(name, source).to_owned(), key.clone());
                        result.names.insert(name.id(), key);
                    }
                }
            }
            if matches!(
                node.kind(),
                "for" | "in_clause" | "match_pattern" | "test_pattern"
            ) || (node.kind() == "rescue" && node.child_by_field_name("variable").is_some())
                || (node.kind() == "binary"
                    && node
                        .child_by_field_name("operator")
                        .is_some_and(|n| text(n, source) == "=~"))
                || (node.kind() == "pair" && node.child_by_field_name("value").is_none())
            {
                result.uncertain = true;
            }
            if matches!(node.kind(), "assignment" | "operator_assignment") {
                if let Some(left) = node.child_by_field_name("left") {
                    for name in binding_nodes(left) {
                        let spelling = text(name, source);
                        let key = scopes
                            .iter()
                            .rev()
                            .find_map(|s| s.get(spelling))
                            .cloned()
                            .unwrap_or_else(|| {
                                let key = format!("local:{next}");
                                next += 1;
                                scopes
                                    .last_mut()
                                    .unwrap()
                                    .insert(spelling.to_owned(), key.clone());
                                key
                            });
                        writes.entry(key.clone()).or_default().push(node);
                        result.names.insert(name.id(), key);
                    }
                }
            }
            if node.kind() == "constant" && constant_read(node) {
                if let Some(value) = constants.get(text(node, source)) {
                    result.names.insert(node.id(), value.clone());
                }
            }
            if node.kind() == "identifier" {
                let spelling = text(node, source);
                let method = node.parent().is_some_and(|p| {
                    p.kind() == "call" && p.child_by_field_name("method") == Some(node)
                });
                if !method {
                    if let Some(key) = scopes.iter().rev().find_map(|s| s.get(spelling)) {
                        if !result.names.contains_key(&node.id()) {
                            reads.entry(key.clone()).or_default().push(node);
                        }
                        result.names.insert(node.id(), key.clone());
                        if let Some(name) = key.strip_prefix("capture:") {
                            result.captures.insert(name.to_owned());
                            result.lexical |= !captures.contains_key(name);
                        } else if key.starts_with("scope:") {
                            result.captures.insert(key.clone());
                            result.lexical = true;
                        }
                    } else {
                        result.calls.insert(node.id());
                    }
                }
                if (method || result.calls.contains(&node.id()))
                    && !node.parent().is_some_and(|call| {
                        dispatch.contains(&(call.start_byte(), call.end_byte()))
                    })
                    && matches!(
                        spelling,
                        "caller"
                            | "caller_locations"
                            | "source_location"
                            | "binding"
                            | "eval"
                            | "local_variables"
                            | "local_variable_get"
                            | "local_variable_set"
                            | "method"
                            | "public_method"
                            | "send"
                            | "public_send"
                            | "__send__"
                            | "instance_eval"
                    )
                {
                    result.uncertain = true;
                }
                if spelling
                    .strip_prefix('_')
                    .is_some_and(|n| n.parse::<u8>().is_ok_and(|n| (1..=9).contains(&n)))
                {
                    result.uncertain = true;
                }
                if spelling == "it" && result.calls.contains(&node.id()) {
                    result.uncertain = true;
                }
                if matches!(spelling, "__FILE__" | "__dir__") {
                    result.captures.insert("<source-file>".to_owned());
                    result.lexical = true;
                }
            }
            pending.push((node, true));
            let mut cursor = node.walk();
            let mut children: Vec<_> = node.named_children(&mut cursor).collect();
            // Ruby's lexical declarations follow source order, even for modifier expressions.
            children.sort_by_key(Node::start_byte);
            pending.extend(children.into_iter().rev().map(|n| (n, false)));
        }
        let unresolved = result
            .captures
            .iter()
            .filter(|name| !name.starts_with("scope:") && !captures.contains_key(*name))
            .cloned()
            .collect();
        result.uncertain |= !stable_captures(block, root, source, &unresolved, &BTreeSet::new());
        for (scope, parameters) in enclosing_scopes {
            let prefix = format!("scope:{}:", scope.start_byte());
            let names = result
                .captures
                .iter()
                .filter_map(|name| name.strip_prefix(prefix.as_str()))
                .map(str::to_owned)
                .collect();
            result.uncertain |= !stable_captures(block, scope, source, &names, &parameters);
        }
        result.declare_callables(block, source, &writes, &reads);
        result
    }

    /// Classifies handler locals holding lambda literals; unmodeled uses keep deferred identity.
    fn declare_callables(
        &mut self,
        block: Node<'_>,
        source: &str,
        writes: &BTreeMap<String, Vec<Node<'_>>>,
        reads: &BTreeMap<String, Vec<Node<'_>>>,
    ) {
        let body = scope_body(block);
        for (key, writes) in writes {
            if !key.starts_with("local:") {
                continue;
            }
            // Only a plain `name = <lambda>` write declares a callable; destructuring and
            // operator assignment keep the binding's existing deferred identity.
            let Some(callables) = writes
                .iter()
                .map(|write| {
                    (write.kind() == "assignment"
                        && write
                            .child_by_field_name("left")
                            .is_some_and(|left| left.kind() == "identifier"))
                    .then(|| write.child_by_field_name("right"))
                    .flatten()
                    .and_then(|right| callable_literal(right, source))
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let Some(invocations) = reads
                .get(key)
                .map(|reads| {
                    reads
                        .iter()
                        .map(|read| invocation(*read, source))
                        .collect::<Option<Vec<_>>>()
                })
                .unwrap_or_else(|| Some(Vec::new()))
            else {
                continue;
            };
            let parameterized = callables
                .iter()
                .any(|callable| callable_parameterized(*callable));
            if callables
                .iter()
                .any(|callable| callable_parameterized(*callable) != parameterized)
                || invocations
                    .iter()
                    .any(|(_, arity)| (*arity != 0) != parameterized)
            {
                continue;
            }
            self.elided.extend(callables.iter().map(Node::id));
            if parameterized {
                self.unresolved_invocations
                    .extend(invocations.iter().map(|(call, _)| call.id()));
                continue;
            }
            if let ([write], [callable]) = (writes.as_slice(), callables.as_slice()) {
                if write.parent() == body {
                    for (call, _) in invocations {
                        if call.parent() == body && call.start_byte() >= write.end_byte() {
                            self.expansions.insert(call.id(), callable.id());
                        }
                    }
                }
            }
        }
    }
}

/// Lexical identity alone cannot close mutable factories, property aliases or escaped values.
///
/// `scope` is the program or an enclosing block whose body initializes the captures;
/// `parameters` are the scope's own parameters, stable without a write.
fn stable_captures(
    handler: Node<'_>,
    scope: Node<'_>,
    source: &str,
    captures: &BTreeSet<String>,
    parameters: &BTreeSet<String>,
) -> bool {
    if captures.is_empty() {
        return true;
    }
    let body = if scope.kind() == "program" {
        Some(scope)
    } else {
        scope_body(scope)
    };
    let mut initialized = BTreeSet::new();
    let mut immutable = BTreeSet::new();
    let mut reads = BTreeSet::new();
    let mut unsafe_names = BTreeSet::new();
    let mut shadows: Vec<BTreeSet<String>> = Vec::new();
    let mut pending = vec![(scope, false)];
    while let Some((node, closing)) = pending.pop() {
        if closing {
            shadows.pop();
            continue;
        }
        if node != scope {
            if WALLS.contains(&node.kind()) {
                continue;
            }
            if scope_node(node) {
                let rebound = parameter_names(node, source);
                let deferred = node == handler
                    || node.parent().is_some_and(|parent| {
                        parent.kind() == "call"
                            && parent
                                .child_by_field_name("method")
                                .is_some_and(|name| super::registration(text(name, source)))
                    });
                if deferred {
                    shadows.push(rebound);
                    deferred_writes(node, source, captures, &mut shadows, &mut unsafe_names);
                    shadows.pop();
                    continue;
                }
                shadows.push(rebound);
                pending.push((node, true));
            }
        }
        if node.kind() == "identifier"
            && captures.contains(text(node, source))
            && !shadows
                .iter()
                .any(|names| names.contains(text(node, source)))
        {
            let name = text(node, source);
            let assignment = node.parent().filter(|n| {
                n.kind() == "assignment"
                    && n.parent() == body
                    && n.child_by_field_name("left") == Some(node)
            });
            let right = assignment.and_then(|n| n.child_by_field_name("right"));
            let literal = right.is_some_and(|n| {
                scalar_literal(n)
                    || string_literal(n, source).is_some()
                    || callable_literal(n, source).is_some()
            });
            if assignment.is_some() {
                if right.is_some_and(|n| n.kind() != "string") {
                    immutable.insert(name.to_owned());
                }
                if !literal || !initialized.insert(name.to_owned()) {
                    unsafe_names.insert(name.to_owned());
                }
            } else if node.parent().is_some_and(|parent| {
                matches!(parent.kind(), "assignment" | "operator_assignment")
                    && parent.child_by_field_name("left") == Some(node)
            }) {
                unsafe_names.insert(name.to_owned());
            } else {
                reads.insert(name.to_owned());
            }
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor).map(|child| (child, false)));
    }
    captures.iter().all(|name| {
        name == "<source-file>"
            || (!unsafe_names.contains(name)
                && if initialized.contains(name) {
                    !reads.contains(name) || immutable.contains(name)
                } else {
                    parameters.contains(name)
                })
    })
}

/// Writes inside a deferred body reach a capture unless a nearer scope rebinds the name.
fn deferred_writes(
    body: Node<'_>,
    source: &str,
    captures: &BTreeSet<String>,
    shadows: &mut Vec<BTreeSet<String>>,
    unsafe_names: &mut BTreeSet<String>,
) {
    let mut pending = vec![(body, false)];
    while let Some((node, closing)) = pending.pop() {
        if closing {
            shadows.pop();
            continue;
        }
        if node != body {
            if WALLS.contains(&node.kind()) {
                continue;
            }
            if scope_node(node) {
                shadows.push(parameter_names(node, source));
                pending.push((node, true));
            }
        }
        if matches!(node.kind(), "assignment" | "operator_assignment") {
            unsafe_names.extend(
                node.child_by_field_name("left")
                    .map(binding_nodes)
                    .unwrap_or_default()
                    .iter()
                    .map(|n| text(*n, source).to_owned())
                    .filter(|name| {
                        captures.contains(name) && !shadows.iter().any(|names| names.contains(name))
                    }),
            );
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor).map(|child| (child, false)));
    }
}

/// Collects binding targets without renaming property receivers, keys, or method names.
fn binding_nodes(node: Node<'_>) -> Vec<Node<'_>> {
    let mut names = Vec::new();
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "identifier" => names.push(node),
            "left_assignment_list"
            | "destructured_left_assignment"
            | "rest_assignment"
            | "block_parameters"
            | "lambda_parameters"
            | "method_parameters"
            | "destructured_parameter" => {
                let mut cursor = node.walk();
                pending.extend(node.named_children(&mut cursor));
            }
            "block_parameter" | "splat_parameter" | "hash_splat_parameter" => {
                pending.extend(node.child_by_field_name("name"));
            }
            _ => {}
        }
    }
    names.sort_by_key(Node::start_byte);
    names
}
