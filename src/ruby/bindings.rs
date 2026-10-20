//! Source-order local bindings for handler identity; names never establish framework trust.
use super::{descendants, literal_string, regex_literal, text};
use std::cell::OnceCell;
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

/// Blocks and lambdas open a scope that still closes over the enclosing locals.
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

/// Names a scope binds through its parameters, including `|; locals|`.
fn parameter_names(scope: Node<'_>, source: &str) -> BTreeSet<String> {
    scope
        .child_by_field_name("parameters")
        .map(binding_nodes)
        .unwrap_or_default()
        .iter()
        .map(|name| text(*name, source).to_owned())
        .collect()
}

/// Immutable literals whose token text is the value.
fn scalar_literal(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "integer" | "float" | "simple_symbol" | "true" | "false" | "nil"
    )
}

/// Decoded text of a string literal without interpolation.
fn string_literal(node: Node<'_>, source: &str) -> Option<String> {
    (node.kind() == "string"
        && !descendants(node)
            .iter()
            .any(|part| part.kind() == "interpolation"))
    .then(|| literal_string(text(node, source)))
    .flatten()
}

/// A `->`, `lambda` or `proc` literal; the returned node owns the whole callable syntax.
pub(super) fn callable_literal<'a>(node: Node<'a>, source: &str) -> Option<Node<'a>> {
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

/// Whether a lambda literal declares parameters; their values are never substituted.
pub(super) fn callable_parameterized(callable: Node<'_>) -> bool {
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

/// Named children in source order.
pub(super) fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

/// Named children that carry syntax; comments are extras and never count.
pub(super) fn semantic_children(node: Node<'_>) -> Vec<Node<'_>> {
    named_children(node)
        .into_iter()
        .filter(|child| !child.is_extra())
        .collect()
}

/// Unwraps transparent parentheses around a single expression.
pub(super) fn unparenthesized(mut node: Node<'_>) -> Node<'_> {
    while node.kind() == "parenthesized_statements" {
        match semantic_children(node).as_slice() {
            [inner] => node = *inner,
            _ => break,
        }
    }
    node
}

/// Declared callable literals by node id; their bodies are syntax until an invocation expands them.
pub(super) fn declared_callables<'a>(
    block: Node<'a>,
    bindings: &Bindings,
) -> BTreeMap<usize, Node<'a>> {
    descendants(block)
        .into_iter()
        .filter(|node| bindings.elided.contains(&node.id()))
        .map(|node| (node.id(), node))
        .collect()
}

/// Statements of the declaration a proven invocation runs, one level deep.
pub(super) fn expansion_statements<'a>(
    bindings: &Bindings,
    declared: &BTreeMap<usize, Node<'a>>,
    call: Node<'a>,
    expanded: bool,
) -> Option<Vec<Node<'a>>> {
    bindings
        .expansions
        .get(&call.id())
        .filter(|_| !expanded)
        .and_then(|callable| declared.get(callable).copied())
        .and_then(callable_body)
        .map(named_children)
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

/// The block, lambda or method that directly owns a node.
fn nearest_scope(node: Node<'_>) -> Option<Node<'_>> {
    std::iter::successors(node.parent(), |node| node.parent())
        .find(|node| scope_node(*node) || WALLS.contains(&node.kind()))
}

/// A `.call(...)` or `.()` dispatch on the binding, through transparent parentheses.
fn invocation<'a>(read: Node<'a>, source: &str) -> Option<(Node<'a>, usize)> {
    let mut value = read;
    while let Some(parent) = value.parent().filter(|node| {
        node.kind() == "parenthesized_statements" && semantic_children(*node).len() == 1
    }) {
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

/// Whether the leading comment block enables `frozen_string_literal`.
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
        // Every assignment has a target; a bare constant is the write itself, anything else
        // (qualified or destructured) poisons each constant it names.
        let targets = node
            .child_by_field_name("left")
            .into_iter()
            .flat_map(|left| {
                if left.kind() == "constant" {
                    vec![left]
                } else {
                    descendants(left)
                        .into_iter()
                        .filter(|part| part.kind() == "constant")
                        .collect()
                }
            });
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

/// Fingerprint value of an immutable constant initializer; strings need `.freeze` or the pragma.
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

/// Syntax that binds locals without an assignment and is not modelled: `for` and pattern
/// variables, `rescue => e`, and a regex literal with a named group on the left of `=~`.
/// A bare `rescue`, a regex on the right, an interpolated regex or a regex held in a variable
/// binds nothing (ruby 4.0: `"a" =~ /(?<y>a)/; y` is a NameError).
fn binds_locals(node: Node<'_>, source: &str) -> bool {
    match node.kind() {
        "for" | "in_clause" | "match_pattern" | "test_pattern" => true,
        "rescue" => node.child_by_field_name("variable").is_some(),
        "binary" => {
            node.child_by_field_name("operator")
                .is_some_and(|n| text(n, source) == "=~")
                && node.child_by_field_name("left").is_some_and(|left| {
                    left.kind() == "regex"
                        && !descendants(left)
                            .iter()
                            .any(|part| part.kind() == "interpolation")
                        // fail-closed: a spelling regex_literal does not model may bind a group
                        && regex_literal(text(left, source))
                            .is_none_or(|(pattern, _)| named_group(&pattern))
                })
        }
        _ => false, // fail-closed: assignments are tracked separately; no other syntax binds a local
    }
}

/// `(?<name>...)` or `(?'name'...)` as live regex syntax; escaped text, character classes,
/// and lookbehind `(?<=`/`(?<!` bind nothing.
fn named_group(pattern: &str) -> bool {
    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    let mut in_class = false;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 1,
            '[' => in_class = true,
            ']' => in_class = false,
            '(' if !in_class && chars.get(index + 1) == Some(&'?') => {
                let opener = chars.get(index + 2);
                if opener == Some(&'\'')
                    || (opener == Some(&'<')
                        && chars
                            .get(index + 3)
                            .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_'))
                {
                    return true;
                }
            }
            _ => {} // fail-closed: only an unescaped `(?` outside a class can open a named group
        }
        index += 1;
    }
    false
}

/// Locals assigned in `scope` before `before`, outside nested scopes; flags unresolved syntax.
fn locals_before(
    scope: Node<'_>,
    before: Node<'_>,
    source: &str,
    uncertain: &mut bool,
) -> BTreeSet<String> {
    let locals = RootLocals::walk(scope, before.start_byte(), source);
    *uncertain |= locals.first_uncertain.is_some();
    locals
        .assignments
        .into_iter()
        .flat_map(|(_, names)| names)
        .collect()
}

/// File-level binding facts shared by every handler in one parsed file, computed on first use.
///
/// Recomputing them per handler made extraction quadratic in the number of handlers per file.
pub(super) struct RootScope<'a> {
    root: Node<'a>,
    source: &'a str,
    constants: OnceCell<BTreeMap<String, String>>,
    locals: OnceCell<RootLocals>,
    /// Start byte of the first program-level assignment of each name.
    first_assignment: OnceCell<BTreeMap<String, usize>>,
    /// Capture facts of the whole program, for blocks a registration call defers.
    stability: OnceCell<CaptureFacts>,
}

/// Program-level assignments and local-binding syntax in source order, outside nested scopes.
struct RootLocals {
    /// Start byte and target names of each program-level assignment.
    assignments: Vec<(usize, Vec<String>)>,
    /// Start byte of the first program-level node that binds locals without an assignment.
    first_uncertain: Option<usize>,
}

/// Program-level locals assigned before one handler, answered by name.
///
/// Materializing the set for every handler copied every earlier assignment per handler, which
/// made extraction quadratic in the number of handlers per file.
struct OuterLocals<'a> {
    first_assignment: &'a BTreeMap<String, usize>,
    limit: usize,
}

impl OuterLocals<'_> {
    /// Whether `name` is assigned at program level before the handler.
    fn contains(&self, name: &str) -> bool {
        self.first_assignment
            .get(name)
            .is_some_and(|start| *start < self.limit)
    }
}

/// The constants a `def` handler can read: none, since it opens a fresh local scope.
static NO_CONSTANTS: BTreeMap<String, String> = BTreeMap::new();

/// The file locals a `def` handler closes over: none, for the same reason.
static NO_LOCALS: BTreeMap<String, usize> = BTreeMap::new();

impl<'a> RootScope<'a> {
    /// Wraps the `program` root of `source`; nothing is computed until a handler asks.
    pub(super) fn new(root: Node<'a>, source: &'a str) -> Self {
        Self {
            root,
            source,
            constants: OnceCell::new(),
            locals: OnceCell::new(),
            first_assignment: OnceCell::new(),
            stability: OnceCell::new(),
        }
    }

    /// The program-level locals assigned before `limit`, equal by name to
    /// `locals_before(root, before)` for a handler starting at `limit`; `None` for a handler
    /// that closes over no file local, which reads nothing from the file.
    fn outer_locals(&self, limit: Option<usize>, uncertain: &mut bool) -> OuterLocals<'_> {
        let Some(limit) = limit else {
            return OuterLocals {
                first_assignment: &NO_LOCALS,
                limit: 0,
            };
        };
        *uncertain |= self
            .locals()
            .first_uncertain
            .is_some_and(|start| start < limit);
        let first_assignment = self.first_assignment.get_or_init(|| {
            let mut first = BTreeMap::new();
            for (start, names) in &self.locals().assignments {
                for name in names {
                    first.entry(name.clone()).or_insert(*start);
                }
            }
            first
        });
        OuterLocals {
            first_assignment,
            limit,
        }
    }

    /// Whether `name`, captured by a block that a registration call defers, is stable in the
    /// file: the verdict of `stable_captures` for every such block, computed once.
    fn stable_capture(&self, name: &str) -> bool {
        name == "<source-file>"
            || self
                .stability
                .get_or_init(|| CaptureFacts::walk(None, self.root, self.source, &|_| true))
                .stable(name, &BTreeSet::new())
    }

    /// The file's program-level locals, indexed on first use.
    fn locals(&self) -> &RootLocals {
        self.locals
            .get_or_init(|| RootLocals::walk(self.root, usize::MAX, self.source))
    }

    /// The file's constants, as `file_constants` computes them.
    fn constants(&self) -> &BTreeMap<String, String> {
        self.constants
            .get_or_init(|| file_constants(self.root, self.source))
    }

    /// Program-level locals assigned before `before`, equal to `locals_before(root, before)`.
    ///
    /// That walk visits exactly the nodes reachable from the root without entering a block,
    /// lambda or wall that start before `before`, and whether a node is entered depends only on
    /// its kind; so the visit set is one source-ordered list cut at `before`.
    #[cfg(test)]
    fn locals_before(&self, before: Node<'_>, uncertain: &mut bool) -> BTreeSet<String> {
        let locals = self.locals();
        let limit = before.start_byte();
        *uncertain |= locals.first_uncertain.is_some_and(|start| start < limit);
        locals
            .assignments
            .iter()
            .take_while(|(start, _)| *start < limit)
            .flat_map(|(_, names)| names.iter().cloned())
            .collect()
    }
}

impl RootLocals {
    /// Walks `scope` without entering nested scopes or walls, skipping nodes at or after `limit`.
    fn walk(scope: Node<'_>, limit: usize, source: &str) -> Self {
        let mut assignments: Vec<(usize, Vec<String>)> = Vec::new();
        let mut first_uncertain = None::<usize>;
        let mut pending = vec![scope];
        while let Some(node) = pending.pop() {
            if node.start_byte() >= limit
                || (node != scope && (scope_node(node) || WALLS.contains(&node.kind())))
            {
                continue;
            }
            if binds_locals(node, source) {
                first_uncertain = Some(
                    first_uncertain.map_or(node.start_byte(), |start| start.min(node.start_byte())),
                );
            }
            match node.kind() {
                "assignment" | "operator_assignment" => assignments.push((
                    node.start_byte(),
                    node.child_by_field_name("left")
                        .map(binding_nodes)
                        .unwrap_or_default()
                        .iter()
                        .map(|name| text(*name, source).to_owned())
                        .collect(),
                )),
                _ => {} // fail-closed: binding syntax without an assignment is reported by binds_locals
            }
            let mut cursor = node.walk();
            pending.extend(node.named_children(&mut cursor));
        }
        assignments.sort_by_key(|(start, _)| *start);
        Self {
            assignments,
            first_uncertain,
        }
    }
}

impl Bindings {
    /// Tracks parser-order bindings while keeping unresolved reflection and implicit parameters uncertain.
    pub fn collect(block: Node<'_>, scope: &RootScope<'_>, source: &str) -> Self {
        Self::collect_with_origins(block, scope, source, &BTreeMap::new(), &BTreeSet::new())
    }

    /// Resolves handler bindings with source-proven captures and lexical shadowing.
    pub(super) fn collect_with_origins(
        block: Node<'_>,
        file: &RootScope<'_>,
        source: &str,
        captures: &BTreeMap<String, String>,
        dispatch: &BTreeSet<(usize, usize)>,
    ) -> Self {
        let mut result = Self::default();
        // Ruby def opens a new local scope; it does not close over the file's locals.
        let closure = block.kind() != "method";
        let outer = file.outer_locals(closure.then(|| block.start_byte()), &mut result.uncertain);
        // Enclosing block and lambda scopes up to the nearest scope wall; a lambda's own body
        // block is folded into the lambda.
        let enclosing: Vec<_> = std::iter::successors(block.parent(), |node| node.parent())
            .take(if closure { usize::MAX } else { 0 })
            .take_while(|node| !WALLS.contains(&node.kind()))
            .filter(|node| {
                scope_node(*node)
                    && !node
                        .parent()
                        .is_some_and(|parent| parent.kind() == "lambda")
            })
            .collect();
        // Fingerprinted blocks are top-level or inside executed lambdas, so constant lookup
        // never crosses a class or module body; `def` handlers keep names.
        let constants = if closure {
            file.constants()
        } else {
            &NO_CONSTANTS
        };
        // Scope layers above the file's locals; the bottom layer holds the handler's own
        // bindings when the handler is not itself a scope node.
        let mut scopes = vec![BTreeMap::<String, String>::new()];
        // The nearest binding of a spelling: scope layers innermost first, then the file's
        // locals, which bind as captures.
        let lookup = |scopes: &[BTreeMap<String, String>], spelling: &str| {
            scopes
                .iter()
                .rev()
                .find_map(|s| s.get(spelling))
                .cloned()
                .or_else(|| {
                    outer
                        .contains(spelling)
                        .then(|| format!("capture:{spelling}"))
                })
        };
        let mut enclosing_scopes = Vec::new();
        for scope in enclosing.iter().rev() {
            // An assignment creates a scope-local only when no outer local already owns the
            // name; parameters and block-locals rebind it regardless.
            let visible = |name: &str| lookup(&scopes, name).is_some();
            let parameters = parameter_names(*scope, source);
            let mut names = parameters.clone();
            // The handler sits inside this scope, so the scope has a body.
            names.extend(
                scope_body(*scope)
                    .map_or_else(BTreeSet::new, |body| {
                        locals_before(body, block, source, &mut result.uncertain)
                    })
                    .into_iter()
                    .filter(|name| !visible(name)),
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
            if binds_locals(node, source)
                || (node.kind() == "pair" && node.child_by_field_name("value").is_none())
            {
                result.uncertain = true;
            }
            if matches!(node.kind(), "assignment" | "operator_assignment") {
                if let Some(left) = node.child_by_field_name("left") {
                    for name in binding_nodes(left) {
                        let spelling = text(name, source);
                        let key = lookup(&scopes, spelling).unwrap_or_else(|| {
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
                    if let Some(key) = lookup(&scopes, spelling) {
                        if !result.names.contains_key(&node.id()) {
                            reads.entry(key.clone()).or_default().push(node);
                        }
                        result.names.insert(node.id(), key.clone());
                        if let Some(name) = key.strip_prefix("capture:") {
                            result.captures.insert(name.to_owned());
                            // A resolved alias keeps its spelling and adds its owner, so two
                            // files binding one name to different providers stay distinct.
                            match captures.get(name) {
                                Some(owner) => {
                                    result.captures.insert(owner.clone());
                                }
                                None => result.lexical = true,
                            }
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
            .filter(|name| {
                !name.starts_with("scope:")
                    && !name.starts_with("ruby:callable:")
                    && !captures.contains_key(*name)
            })
            .cloned()
            .collect::<BTreeSet<String>>();
        // A block a registration call defers reads the file's facts, which are the same for
        // every such block; any other handler shape keeps its own walk.
        result.uncertain |= if registration_block(block, source) {
            !unresolved.iter().all(|name| file.stable_capture(name))
        } else {
            !stable_captures(block, file.root, source, &unresolved, &BTreeSet::new())
        };
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
            // One declaration owned by the handler scope runs at every later invocation, in
            // any expression position; inside a nested block the expansion is deferred.
            // Declarations inside nested scopes and reassigned declarations stay syntax.
            if let ([write], [callable]) = (writes.as_slice(), callables.as_slice()) {
                if nearest_scope(*write) == Some(block) {
                    for (call, _) in invocations {
                        if call.start_byte() >= write.end_byte() {
                            self.expansions.insert(call.id(), callable.id());
                        }
                    }
                }
            }
        }
    }
}

/// Whether `node` is the block of a step-registration call, which defers its body.
fn registration_block(node: Node<'_>, source: &str) -> bool {
    scope_node(node)
        && node.parent().is_some_and(|parent| {
            parent.kind() == "call"
                && parent
                    .child_by_field_name("method")
                    .is_some_and(|name| super::registration(text(name, source)))
        })
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
    let facts = CaptureFacts::walk(Some(handler), scope, source, &|name| {
        captures.contains(name)
    });
    captures.iter().all(|name| facts.stable(name, parameters))
}

/// What a scope's body establishes about each tracked name: the facts `stable_captures`
/// judges. Names never interact, so the facts of one name are the same whichever names are
/// tracked alongside it.
#[derive(Default)]
struct CaptureFacts {
    initialized: BTreeSet<String>,
    immutable: BTreeSet<String>,
    reads: BTreeSet<String>,
    unsafe_names: BTreeSet<String>,
}

impl CaptureFacts {
    /// Walks `scope` recording the names `tracked` accepts. `handler`'s own body is deferred,
    /// as is every block a registration call defers; `None` defers only the latter.
    fn walk(
        handler: Option<Node<'_>>,
        scope: Node<'_>,
        source: &str,
        tracked: &dyn Fn(&str) -> bool,
    ) -> Self {
        let body = if scope.kind() == "program" {
            Some(scope)
        } else {
            scope_body(scope)
        };
        let mut facts = Self::default();
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
                    let deferred = handler == Some(node) || registration_block(node, source);
                    if deferred {
                        shadows.push(rebound);
                        deferred_writes(
                            node,
                            source,
                            tracked,
                            &mut shadows,
                            &mut facts.unsafe_names,
                        );
                        shadows.pop();
                        continue;
                    }
                    shadows.push(rebound);
                    pending.push((node, true));
                }
            }
            if node.kind() == "identifier"
                && tracked(text(node, source))
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
                        facts.immutable.insert(name.to_owned());
                    }
                    if !literal || !facts.initialized.insert(name.to_owned()) {
                        facts.unsafe_names.insert(name.to_owned());
                    }
                } else if node.parent().is_some_and(|parent| {
                    matches!(parent.kind(), "assignment" | "operator_assignment")
                        && parent.child_by_field_name("left") == Some(node)
                }) {
                    facts.unsafe_names.insert(name.to_owned());
                } else {
                    facts.reads.insert(name.to_owned());
                }
            }
            let mut cursor = node.walk();
            pending.extend(node.named_children(&mut cursor).map(|child| (child, false)));
        }
        facts
    }

    /// Whether `name` is stable: written once by a literal and either never read or
    /// immutable, or one of the scope's own `parameters`.
    fn stable(&self, name: &str, parameters: &BTreeSet<String>) -> bool {
        name == "<source-file>"
            || (!self.unsafe_names.contains(name)
                && if self.initialized.contains(name) {
                    !self.reads.contains(name) || self.immutable.contains(name)
                } else {
                    parameters.contains(name)
                })
    }
}

/// Writes inside a deferred body reach a capture unless a nearer scope rebinds the name.
fn deferred_writes(
    body: Node<'_>,
    source: &str,
    tracked: &dyn Fn(&str) -> bool,
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
                        tracked(name) && !shadows.iter().any(|names| names.contains(name))
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

#[cfg(test)]
mod tests {
    use super::super::load_phase::tests::parse;
    use super::*;

    /// Asserts the per-file index answers every handler-shaped node exactly as the root walk
    /// does: the locals before it, by set and by name, and the stability of every name the file
    /// mentions for each block a registration call defers. Returns the handler and registration
    /// block counts.
    fn assert_index_matches_walk(source: &str, label: &str) -> (usize, usize) {
        let tree = parse(source);
        let root = tree.root_node();
        let scope = RootScope::new(root, source);
        let mut names: BTreeSet<String> = descendants(root)
            .into_iter()
            .filter(|node| node.kind() == "identifier")
            .map(|node| text(node, source).to_owned())
            .collect();
        names.insert("<source-file>".to_owned());
        let (mut checked, mut deferred) = (0, 0);
        for node in descendants(root) {
            if !matches!(node.kind(), "block" | "do_block" | "lambda" | "method") {
                continue;
            }
            let (mut indexed, mut looked_up, mut walked) = (false, false, false);
            let before = locals_before(root, node, source, &mut walked);
            assert_eq!(
                scope.locals_before(node, &mut indexed),
                before,
                "{label}: names"
            );
            assert_eq!(indexed, walked, "{label}: uncertain");
            let outer = scope.outer_locals(Some(node.start_byte()), &mut looked_up);
            for name in &names {
                assert_eq!(
                    outer.contains(name),
                    before.contains(name),
                    "{label}: outer {name}"
                );
            }
            assert_eq!(looked_up, walked, "{label}: outer uncertain");
            checked += 1;
            if !registration_block(node, source) {
                continue;
            }
            let none = BTreeSet::new();
            for name in &names {
                let single = BTreeSet::from([name.clone()]);
                assert_eq!(
                    scope.stable_capture(name),
                    stable_captures(node, root, source, &single, &none),
                    "{label}: stable {name}"
                );
            }
            assert_eq!(
                names.iter().all(|name| scope.stable_capture(name)),
                stable_captures(node, root, source, &names, &none),
                "{label}: stable set"
            );
            deferred += 1;
        }
        assert_eq!(
            scope.constants(),
            &file_constants(root, source),
            "{label}: constants"
        );
        (checked, deferred)
    }

    /// The index equals the walk for every assignment form, nested scope, enclosing block and
    /// local-binding syntax, before and after each handler.
    #[test]
    fn root_index_matches_the_walk_on_the_matrix() {
        let source = r#"
LIMIT = 5
first, *rest = [1, 2, 3]
total = 0
total += 1
Given('first') { work(total) }
if ready
  inside_if = 1
end
begin
  inside_begin = 2
rescue => error
  handled = error
end
def helper
  hidden_in_def = 3
end
class Holder
  hidden_in_class = 4
end
module Space
  hidden_in_module = 5
end
class << self
  hidden_in_singleton = 6
end
[1].each { |item| hidden_in_block = item }
3.times do |index|
  inside_loop = index
  Given("loop #{index}") { work(inside_loop) }
end
callable = -> { hidden_in_lambda = 7 }
When('middle') do
  work(callable, inside_if)
end
for element in [1]
  looped = element
end
case value
in { name: }
  matched = name
end
/(?<group>x)/ =~ text
after = 8
Then('last') { work(after) }
def step_method
  Given('in method') { work }
end
"#;
        assert!(assert_index_matches_walk(source, "matrix").0 >= 10);
    }

    /// The cached stability verdict equals the walk for every way a file-level local can be
    /// written, read, shadowed or captured, and the handler shapes that keep their own walk
    /// are compared through the fallback.
    #[test]
    fn root_stability_matches_the_walk_on_the_matrix() {
        let source = r#"
literal = 1
Given('literal') { work(literal) }
nonliteral = make
Given('nonliteral') { work(nonliteral) }
twice = 1
twice = 2
Given('twice') { work(twice) }
later = 1
Given('later') { work(later) }
later = 3
plus = 1
plus += 1
Given('plus') { work(plus) }
inner = 1
Given('inner write') { inner = 2 }
Given('inner read') { work(inner) }
text = "mutable"
Given('text') { work(text) }
[1].each { work(text) }
frozen = :symbol
[1].each { work(frozen) }
Given('frozen') { work(frozen) }
shadowed = 1
[1].each { |shadowed| shadowed = 2 }
Given('shadowed') { work(shadowed) }
local = 1
Given('block local') { |;local| local = 5 }
Given('local') { work(local) }
Given('file') { work(__FILE__) }
CONST = 1
Given('const') { work(CONST) }
first, second = 1, 2
Given('destructured') { work(first, second) }
Given('before') { work(after_handler) }
after_handler = 1
step = lambda { work(text) }
Given('lambda', &step)
callable = -> { work(frozen) }
Before { setup = 1 }
Given('setup') { work(setup) }
def helper_step
  Given('in def') { work(literal) }
end
3.times do
  nested = 4
  Given('nested') { work(nested) }
end
"#;
        let (checked, deferred) = assert_index_matches_walk(source, "stability");
        assert!(
            checked >= 24 && deferred >= 18,
            "{checked} handlers, {deferred} deferred"
        );
    }

    /// A handler that no registration call defers keeps its own walk: the reads in its own body
    /// do not count for it, while they do count for every registration block, whose cached
    /// verdict therefore differs from the lambda's.
    #[test]
    fn handlers_outside_registration_calls_keep_their_own_walk() {
        let source = "text = \"mutable\"\nstep = lambda { work(text) }\nGiven('lambda', &step)\nGiven('block') { work(text) }\n";
        let tree = parse(source);
        let root = tree.root_node();
        let scope = RootScope::new(root, source);
        let mut handlers: Vec<_> = descendants(root)
            .into_iter()
            .filter(|node| matches!(node.kind(), "lambda" | "block"))
            .collect();
        handlers.sort_by_key(Node::start_byte);
        let [lambda, block]: [Node<'_>; 2] = handlers.try_into().unwrap();
        assert!(!Bindings::collect(lambda, &scope, source).uncertain);
        assert!(Bindings::collect(block, &scope, source).uncertain);
        assert!(!scope.stable_capture("text"));
    }

    /// The index equals the walk for every handler-shaped node in every Ruby fixture.
    #[test]
    fn root_index_matches_the_walk_on_every_ruby_fixture() {
        let mut files = Vec::new();
        let mut pending =
            vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rb") {
                    files.push(path);
                }
            }
        }
        files.sort();
        assert!(files.len() > 100, "Ruby fixtures not found");
        let (mut checked, mut deferred) = (0, 0);
        for path in &files {
            let source = std::fs::read_to_string(path).unwrap();
            let counts = assert_index_matches_walk(&source, &path.display().to_string());
            checked += counts.0;
            deferred += counts.1;
        }
        assert!(
            checked > 100 && deferred > 100,
            "{checked} handlers, {deferred} deferred"
        );
    }
}
