//! Exact Ruby handler trees with explicit lexical capture identity.
use super::{descendants, text};
use crate::model::{BehaviorEvent, ControlFlowOperation, HandlerFingerprint, HandlerSemantics};
use std::borrow::Cow;
use std::collections::BTreeSet;
use tree_sitter::Node;

/// Builds handler identity from bound syntax, captures, and assertion effects.
pub(super) fn fingerprint(
    block: Node<'_>,
    root: Node<'_>,
    source: &str,
    assertions: Option<&super::assertions::AssertionBindings>,
) -> HandlerFingerprint {
    let nodes = descendants(block);
    let bindings = assertions.map_or_else(
        || super::bindings::Bindings::collect(block, root, source),
        |assertions| assertions.bindings(block, root, source),
    );
    let comparable = !bindings.uncertain
        && !assertions.is_some_and(|assertions| assertions.known_ineligible(block, root, source))
        && !nodes.iter().any(|node| {
            (block.kind() == "method"
                && matches!(
                    node.kind(),
                    "self" | "instance_variable" | "class_variable" | "global_variable"
                )
                && !assertions.is_some_and(|assertions| assertions.known_field(*node)))
                || (node != &block
                    && matches!(
                        node.kind(),
                        "method"
                            | "singleton_method"
                            | "class"
                            | "module"
                            | "singleton_class"
                            | "heredoc_body"
                            | "heredoc_beginning"
                            | "optional_parameter"
                            | "keyword_parameter"
                            | "ERROR"
                    ))
        });
    let (tokens, alpha) = syntax_tokens(block, source, &bindings);
    let captures = &bindings.captures;
    let exact = if captures.is_empty() {
        serde_json::to_string(&tokens)
    } else {
        serde_json::to_string(&(tokens, captures))
    }
    .expect("string tokens serialize");
    // Literal abstraction needs the complete operation stream that configured assertion
    // evidence computes; ordinary handlers without it keep exact identity.
    let structural = parameterized_calls(block, source, &bindings)
        .or_else(|| {
            assertions
                .and_then(|assertions| assertions.ordinary_structural(block, source, &bindings))
        })
        .or_else(|| assertions.map(|_| structural_tokens(&alpha, captures)))
        .unwrap_or_else(|| exact.clone());
    let mut behavior_signature = if bindings.lexical {
        vec![BehaviorEvent::Method("ruby:lexical-file".to_owned())]
    } else {
        vec![]
    };
    let events = assertions.map(|assertions| assertions.events(block, source, &bindings));
    let assertion_evidence = events.as_ref().is_some_and(|events| {
        events
            .iter()
            .any(|event| matches!(event, BehaviorEvent::Assertion { .. }))
    });
    // An untrusted `expect` chain anywhere in the handler keeps the exact-handler policy under
    // either regime: duplicates by alpha identity stay, near and action similarity are withdrawn.
    let exact_policy = untrusted_expect(&nodes, source, &bindings);
    if assertion_evidence {
        // Configured assertion evidence keeps the complete-syntax stream bound to its values.
        behavior_signature.extend(events.unwrap_or_default());
        if !exact_policy {
            behavior_signature.push(BehaviorEvent::Method("ruby:complete-events".to_owned()));
        }
    } else if let Some(actions) = (!exact_policy)
        .then(|| action_stream(block, source, &bindings))
        .flatten()
    {
        behavior_signature.extend(actions);
        behavior_signature.push(BehaviorEvent::Method("ruby:action-events".to_owned()));
    } else {
        // An unmodeled construct keeps the exact-handler policy: no marker, no near overlap.
        behavior_signature.extend(events.unwrap_or_default());
    }
    let trivial = bindings.calls.is_empty()
        && !nodes
            .iter()
            .any(|node| matches!(node.kind(), "call" | "assignment" | "operator_assignment"));
    let alpha_normalized =
        serde_json::to_string(&(alpha, &captures)).expect("binding tokens serialize");
    HandlerSemantics {
        exact: exact.clone(),
        normalized: exact.clone(),
        alpha: alpha_normalized,
        structural,
        events: behavior_signature,
        source_snippet: text(block, source).chars().take(2000).collect(),
        comparable,
        trivial,
    }
    .finish(str::to_owned)
}

/// Abstract only whole straight-line handlers; any unmodeled effect preserves exact identity.
pub(super) fn parameterized_calls(
    block: Node<'_>,
    source: &str,
    bindings: &super::bindings::Bindings,
) -> Option<String> {
    if block.child_by_field_name("parameters").is_some() {
        return None;
    }
    let body = block.child_by_field_name("body")?;
    let mut cursor = body.walk();
    let mut calls = Vec::new();
    let mut literals = 0;
    for call in body.named_children(&mut cursor).filter(|n| !n.is_extra()) {
        if call.kind() != "call" || call.child_by_field_name("block").is_some() {
            return None;
        }
        let receiver = call.child_by_field_name("receiver")?;
        if !matches!(
            receiver.kind(),
            "identifier" | "self" | "instance_variable" | "global_variable" | "constant"
        ) {
            return None;
        }
        let mut arguments = Vec::new();
        if let Some(args) = call.child_by_field_name("arguments") {
            let mut cursor = args.walk();
            for arg in args.named_children(&mut cursor).filter(|n| !n.is_extra()) {
                match arg.kind() {
                    "string"
                        if !descendants(arg).iter().any(|n| n.kind() == "interpolation")
                            && super::literal_string(text(arg, source)).is_some() => {}
                    "integer" | "float" => {}
                    _ => return None,
                }
                arguments.push(arg.kind());
                literals += 1;
            }
        }
        calls.push((
            receiver.kind(),
            bindings
                .names
                .get(&receiver.id())
                .map_or(text(receiver, source), String::as_str),
            text(call.child_by_field_name("operator")?, source),
            text(call.child_by_field_name("method")?, source),
            arguments,
        ));
    }
    (literals > 0).then(|| {
        serde_json::to_string(&("ruby:literal-calls", calls, &bindings.captures))
            .expect("call structure serializes")
    })
}

/// Abstracts literal values out of alpha tokens; behaviour events keep the values.
fn structural_tokens(alpha: &SyntaxTokens<'_, '_>, captures: &BTreeSet<String>) -> String {
    let abstracted: Vec<(&str, &str)> = alpha
        .iter()
        .map(|(kind, value)| match *kind {
            "integer" | "float" | "string_content" | "escape_sequence" => (*kind, "<literal>"),
            _ => value
                .strip_prefix("const:")
                .and_then(|rest| rest.split_once(':'))
                .map_or((*kind, value.as_ref()), |(kind, _)| ("constant", kind)),
        })
        .collect();
    serde_json::to_string(&("ruby:structural", abstracted, captures))
        .expect("structural tokens serialize")
}

// Preserve topology as well as leaves; Ruby whitespace can alter call nesting.
type SyntaxTokens<'tree, 'source> = Vec<(&'tree str, Cow<'source, str>)>;
/// Exact and alpha-renamed token streams of a subtree, with declared callable bodies kept.
pub(super) fn syntax_tokens<'tree, 'source>(
    root: Node<'tree>,
    source: &'source str,
    bindings: &super::bindings::Bindings,
) -> (SyntaxTokens<'tree, 'source>, SyntaxTokens<'tree, 'source>) {
    tokens_with_elision(root, source, bindings, false)
}

/// Alpha tokens for behaviour events: declared callable bodies are syntax, not behaviour.
pub(super) fn event_tokens<'tree, 'source>(
    root: Node<'tree>,
    source: &'source str,
    bindings: &super::bindings::Bindings,
) -> SyntaxTokens<'tree, 'source> {
    tokens_with_elision(root, source, bindings, true).1
}

/// Token streams of a subtree; with `elide`, declared callable bodies collapse to one placeholder leaf.
fn tokens_with_elision<'tree, 'source>(
    root: Node<'tree>,
    source: &'source str,
    bindings: &super::bindings::Bindings,
    elide: bool,
) -> (SyntaxTokens<'tree, 'source>, SyntaxTokens<'tree, 'source>) {
    // Preserve tree structure: Ruby whitespace can change call nesting without changing leaves.
    let mut tokens = Vec::new();
    let mut alpha = Vec::new();
    let mut pending = vec![(root, false)];
    while let Some((node, closing)) = pending.pop() {
        if elide && node != root && bindings.elided.contains(&node.id()) {
            tokens.push((node.kind(), Cow::Borrowed("<declared>")));
            alpha.push((node.kind(), Cow::Borrowed("<declared>")));
            continue;
        }
        if root.kind() == "method"
            && node.parent() == Some(root)
            && (root.child_by_field_name("name") == Some(node)
                || matches!(node.kind(), "def" | "end"))
        {
            continue;
        }
        if node.kind() == "comment"
            || (node
                .parent()
                .is_some_and(|parent| matches!(parent.kind(), "block" | "do_block"))
                && matches!(node.kind(), "{" | "}" | "do" | "end"))
        {
            continue;
        }
        let kind = match node.kind() {
            "block" | "do_block" => "block",
            "block_body" | "body_statement" => "body",
            other => other,
        };
        if closing {
            tokens.push((")", Cow::Borrowed("")));
            alpha.push((")", Cow::Borrowed("")));
            continue;
        }
        if node.kind() == "line"
            || (node.kind() == "identifier" && text(node, source) == "__LINE__")
        {
            tokens.push((
                node.kind(),
                Cow::Owned((node.start_position().row + 1).to_string()),
            ));
            alpha.push(tokens.last().unwrap().clone());
        } else if node.child_count() == 0 {
            tokens.push((kind, Cow::Borrowed(text(node, source))));
            alpha.push((
                kind,
                bindings.names.get(&node.id()).map_or_else(
                    || Cow::Borrowed(text(node, source)),
                    |name| Cow::Owned(name.clone()),
                ),
            ));
        } else {
            tokens.push((kind, Cow::Borrowed("(")));
            alpha.push((kind, Cow::Borrowed("(")));
            pending.push((node, true));
            let mut cursor = node.walk();
            let children: Vec<_> = node.children(&mut cursor).collect();
            pending.extend(children.into_iter().rev().map(|node| (node, false)));
        }
    }
    (tokens, alpha)
}

/// Receiver-chain root of a call: intermediate calls collapse, a receiverless root call keeps
/// its name as the base (`expect(x).to` → `expect`).
fn chain_base<'a>(call: Node<'a>) -> Option<Node<'a>> {
    let mut root = super::bindings::unparenthesized(call.child_by_field_name("receiver")?);
    while root.kind() == "call" {
        match root.child_by_field_name("receiver") {
            Some(receiver) => root = super::bindings::unparenthesized(receiver),
            None => break,
        }
    }
    Some(root)
}

/// Whether `node`, through transparent parentheses, is the receiver of a call.
fn is_receiver(node: Node<'_>) -> bool {
    let mut value = node;
    while let Some(parent) = value
        .parent()
        .filter(|parent| parent.kind() == "parenthesized_statements")
    {
        value = parent;
    }
    value
        .parent()
        .is_some_and(|parent| parent.child_by_field_name("receiver") == Some(value))
}

/// Whether a callable literal is handed to a call, directly or inside a container argument.
fn in_argument_position(node: Node<'_>) -> bool {
    std::iter::successors(node.parent(), |node| node.parent())
        .take_while(|node| !matches!(node.kind(), "block" | "do_block" | "lambda" | "program"))
        .any(|node| node.kind() == "argument_list")
}

/// Whether the handler uses `expect` without configured trust: a receiverless `expect(...)` or
/// `expect { }` (a configured factory always carries a receiver), or a bare `expect` method read.
fn untrusted_expect(
    nodes: &[Node<'_>],
    source: &str,
    bindings: &super::bindings::Bindings,
) -> bool {
    nodes.iter().copied().any(|node| match node.kind() {
        "call" => {
            node.child_by_field_name("receiver").is_none()
                && node
                    .child_by_field_name("method")
                    .is_some_and(|method| text(method, source) == "expect")
        }
        "identifier" => text(node, source) == "expect" && bindings.calls.contains(&node.id()),
        _ => false, // fail-closed: every other node is neither a call nor a method read
    })
}

/// Action events for a handler whose every construct is modeled: `base#method` per call with
/// arguments dropped, bare helper calls, callbacks and control flow. `None` keeps the exact
/// handler policy for anything unmodeled or for an untrusted `expect` chain.
pub(super) fn action_stream(
    block: Node<'_>,
    source: &str,
    bindings: &super::bindings::Bindings,
) -> Option<Vec<BehaviorEvent>> {
    let declared = super::bindings::declared_callables(block, bindings);
    let mut events = Vec::new();
    let mut pending = vec![(block, false)];
    while let Some((node, expanded)) = pending.pop() {
        if node != block
            && (declared.contains_key(&node.id())
                || node.is_extra()
                || (block.kind() == "method" && block.child_by_field_name("name") == Some(node)))
        {
            continue;
        }
        if node != block
            && matches!(node.kind(), "block" | "do_block" | "lambda")
            && node
                .child_by_field_name("parameters")
                .is_some_and(|parameters| parameters.named_child_count() != 0)
        {
            return None;
        }
        // A callable literal is a callback only when handed to a call; anywhere else it is
        // syntax, like a function expression the JavaScript frontend never enters.
        if node != block {
            if let Some(callable) = super::bindings::callable_literal(node, source) {
                if in_argument_position(callable) {
                    // `lambda { |x| }` and `proc { |x| }` carry parameters on their block.
                    if super::bindings::callable_parameterized(callable) {
                        return None;
                    }
                    let statements = super::bindings::callable_body(callable)
                        .map(super::bindings::named_children)
                        .unwrap_or_default();
                    pending.extend(statements.into_iter().rev().map(|node| (node, expanded)));
                }
                continue;
            }
        }
        match node.kind() {
            "call" => {
                if bindings.unresolved_invocations.contains(&node.id()) {
                    return None;
                }
                if let Some(statements) =
                    super::bindings::expansion_statements(bindings, &declared, node, expanded)
                {
                    pending.extend(statements.into_iter().rev().map(|node| (node, true)));
                    continue;
                }
                // A receiverless root call is the base of the chain it starts: `helper.run` and
                // `helper().run` are one call sequence in Ruby.
                if node.child_by_field_name("receiver").is_some() || !is_receiver(node) {
                    let method = node
                        .child_by_field_name("method")
                        .map_or("()", |method| text(method, source));
                    let base = match chain_base(node) {
                        None => String::new(),
                        Some(root) => match root.kind() {
                            "call" => text(root.child_by_field_name("method")?, source).to_owned(),
                            "identifier" | "constant" => bindings
                                .names
                                .get(&root.id())
                                .cloned()
                                .unwrap_or_else(|| text(root, source).to_owned()),
                            "scope_resolution" | "self" | "instance_variable"
                            | "class_variable" | "global_variable" => text(root, source).to_owned(),
                            _ => serde_json::to_string(&event_tokens(root, source, bindings))
                                .expect("root tokens serialize"),
                        },
                    };
                    let operator = node
                        .child_by_field_name("operator")
                        .map_or("", |operator| text(operator, source));
                    events.push(BehaviorEvent::Call(format!(
                        "ruby:action:{base}{operator}#{method}"
                    )));
                }
            }
            "identifier" => {
                if bindings.calls.contains(&node.id()) && !is_receiver(node) {
                    events.push(BehaviorEvent::Call(format!(
                        "ruby:action:#{}",
                        text(node, source)
                    )));
                }
            }
            "if" | "if_modifier" | "elsif" | "conditional" => {
                events.push(BehaviorEvent::ControlFlow(ControlFlowOperation::If));
            }
            // Negated forms are opposite conditions, never the shared `if`/`while` operation.
            "unless" | "unless_modifier" | "until" | "until_modifier" => {
                events.push(BehaviorEvent::Legacy(format!(
                    "ruby:control:{}",
                    node.kind().trim_end_matches("_modifier")
                )));
            }
            "case" => events.push(BehaviorEvent::ControlFlow(ControlFlowOperation::Switch)),
            "while" | "while_modifier" => {
                events.push(BehaviorEvent::ControlFlow(ControlFlowOperation::While));
            }
            "return" => events.push(BehaviorEvent::ControlFlow(ControlFlowOperation::Return)),
            "block"
            | "do_block"
            | "lambda"
            | "block_body"
            | "body_statement"
            | "method"
            | "argument_list"
            | "parenthesized_statements"
            | "then"
            | "else"
            | "when"
            | "pattern"
            | "do"
            | "next"
            | "break"
            | "assignment"
            | "operator_assignment"
            | "array"
            | "hash"
            | "pair"
            | "hash_key_symbol"
            | "string"
            | "string_content"
            | "escape_sequence"
            | "simple_symbol"
            | "delimited_symbol"
            | "integer"
            | "float"
            | "true"
            | "false"
            | "nil"
            | "regex"
            | "constant"
            | "scope_resolution"
            | "self"
            | "instance_variable"
            | "class_variable"
            | "global_variable"
            | "block_parameters"
            | "lambda_parameters"
            | "method_parameters"
            | "identifier_suffix" => {}
            // fail-closed: element references, operators, super, yield, splats,
            // interpolation, rescue and every other construct keep the exact handler policy
            _ => return None,
        }
        pending.extend(
            super::bindings::named_children(node)
                .into_iter()
                .rev()
                .map(|child| (child, expanded)),
        );
    }
    Some(events)
}
