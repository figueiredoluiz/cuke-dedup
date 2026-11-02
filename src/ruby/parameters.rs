//! Static matching patterns, independent of transformer execution and handler fingerprints.
use super::{descendants, literal_string, location, regex_expression, regex_literal, text};
pub(super) use crate::analysis::parameter_types::invalidates_registry;
use crate::source_adapter::{SourceFile, SourceParameterType};
use std::collections::BTreeMap;
use tree_sitter::Node;

/// Collect receiverless ParameterType declarations, retaining unsupported patterns as unresolved entries.
pub(super) fn collect(
    nodes: &[Node<'_>],
    source: &str,
    file: &SourceFile,
) -> Vec<SourceParameterType> {
    nodes
        .iter()
        .copied()
        .filter(|node| {
            node.kind() == "call"
                && node.child_by_field_name("receiver").is_none()
                && node
                    .child_by_field_name("method")
                    .is_some_and(|m| text(m, source) == "ParameterType")
        })
        .map(|node| {
            let options = options(node, source);
            let name = options
                .as_ref()
                .and_then(|options| options.get("name"))
                .and_then(|name| string(*name, source));
            let expression = options.as_ref().and_then(|options| {
                if !node.parent().is_some_and(|p| p.kind() == "program")
                    || node.child_by_field_name("block").is_some()
                    || !name.as_ref().is_some_and(|n| {
                        !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    })
                    || !options
                        .get("transformer")
                        .is_some_and(|n| deferred_transformer(*n, source))
                {
                    return None;
                }
                if options
                    .get("prefer_for_regexp_match")
                    .is_some_and(|n| n.kind() == "true")
                {
                    return None;
                }
                pattern(*options.get("regexp")?, source)
            });
            SourceParameterType::new(name, expression, location(file, node, source))
                .with_domain(super::semantics::policy().domain())
        })
        .collect()
}

/// Read supported static parameter-type options, rejecting duplicate keys and unsupported values.
fn options<'a>(node: Node<'a>, source: &str) -> Option<BTreeMap<String, Node<'a>>> {
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let children: Vec<_> = arguments
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
        .collect();
    let container = if children.len() == 1 && children[0].kind() == "hash" {
        children[0]
    } else {
        arguments
    };
    let mut result = BTreeMap::new();
    let mut cursor = container.walk();
    for pair in container
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
    {
        if pair.kind() != "pair" {
            return None;
        }
        let key = pair.child_by_field_name("key")?;
        let key = match key.kind() {
            "hash_key_symbol" => text(key, source),
            "simple_symbol" => text(key, source).strip_prefix(':')?,
            _ => return None,
        };
        let value = pair.child_by_field_name("value")?;
        match key {
            "name" | "regexp" | "transformer" => {}
            "type" if matches!(value.kind(), "constant" | "scope_resolution" | "nil") => {}
            "use_for_snippets" | "prefer_for_regexp_match"
                if matches!(value.kind(), "true" | "false" | "nil") => {}
            _ => return None,
        }
        if result.insert(key.to_owned(), value).is_some() {
            return None;
        }
    }
    Some(result)
}

/// Decode a non-interpolated Ruby string literal.
fn string(node: Node<'_>, source: &str) -> Option<String> {
    (node.kind() == "string"
        && !descendants(node)
            .iter()
            .any(|n| n.kind() == "interpolation"))
    .then(|| literal_string(text(node, source)))
    .flatten()
}

/// Convert supported string, regex, or nonempty array patterns within the regex size limit.
fn pattern(node: Node<'_>, source: &str) -> Option<String> {
    if descendants(node)
        .iter()
        .any(|n| n.kind() == "interpolation")
    {
        return None;
    }
    match node.kind() {
        "string" => regex_expression(&string(node, source)?, ""),
        "regex" => {
            let (pattern, flags) = regex_literal(text(node, source))?;
            matches!(flags.as_str(), "" | "m")
                .then(|| regex_expression(&pattern, &flags))
                .flatten()
        }
        "array" => {
            let mut cursor = node.walk();
            let expressions = node
                .named_children(&mut cursor)
                .filter(|n| n.kind() != "comment")
                .map(|n| {
                    matches!(n.kind(), "string" | "regex")
                        .then(|| pattern(n, source))
                        .flatten()
                })
                .collect::<Option<Vec<_>>>()?;
            if expressions.is_empty() {
                return None;
            }
            let joined = format!("(?:{})", expressions.join("|"));
            (joined.len() <= crate::resource_limits::MAX_REGEX_PATTERN_BYTES).then_some(joined)
        }
        _ => None,
    }
}

/// Recognize lambda and receiverless lambda/proc block forms without call arguments.
fn deferred_transformer(node: Node<'_>, source: &str) -> bool {
    node.kind() == "lambda"
        || (node.kind() == "call"
            && node.child_by_field_name("receiver").is_none()
            && node
                .child_by_field_name("method")
                .is_some_and(|m| matches!(text(m, source), "lambda" | "proc"))
            && node
                .child_by_field_name("arguments")
                .is_none_or(|a| a.named_child_count() == 0)
            && node.child_by_field_name("block").is_some())
}
