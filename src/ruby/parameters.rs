//! Static matching patterns, independent of transformer execution and handler fingerprints.
use super::{descendants, literal_string, location, regex_expression, regex_literal, text};
use crate::source_adapter::{SourceFile, SourceParameterType};
use std::collections::BTreeMap;
use tree_sitter::Node;

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
        })
        .collect()
}

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

fn string(node: Node<'_>, source: &str) -> Option<String> {
    (node.kind() == "string"
        && !descendants(node)
            .iter()
            .any(|n| n.kind() == "interpolation"))
    .then(|| literal_string(text(node, source)))
    .flatten()
}

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
            flags
                .is_empty()
                .then(|| regex_expression(&pattern, ""))
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

pub(super) fn invalidates_registry(declaration: &SourceParameterType) -> bool {
    declaration.name.as_deref().is_none_or(|name| {
        matches!(
            name,
            "" | "int"
                | "float"
                | "word"
                | "string"
                | "bigdecimal"
                | "biginteger"
                | "byte"
                | "short"
                | "long"
                | "double"
        )
    })
}

/// Returns only non-colliding known patterns; source declarations supersede manual fallbacks.
pub(crate) fn merge(
    declarations: Vec<SourceParameterType>,
    configured: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut patterns = configured.clone();
    let mut seen = std::collections::BTreeSet::new();
    let mut diagnostics = Vec::new();
    // fail-closed: a declaration that invalidates the registry (a name that is not static, empty,
    // or a reserved built-in) may define any parameter type, so no pattern from this merge is
    // trusted, whichever file or position it comes from. Extraction reports the located cause and
    // invalidates the registry, so no diagnostic is repeated here.
    let untrusted = declarations.iter().any(invalidates_registry);
    for declaration in declarations {
        let Some(name) = declaration.name else {
            continue;
        };
        match (seen.insert(name.clone()), declaration.expression) {
            (true, Some(expression)) => {
                patterns.insert(name, expression);
            }
            _ => {
                patterns.remove(&name);
                diagnostics.push(format!("Ruby parameter type {name:?} has an unresolved or duplicate declaration at {}:{}", declaration.location.path.display(), declaration.location.line));
            }
        }
    }
    if untrusted {
        patterns.clear();
    }
    (patterns, diagnostics)
}

#[cfg(test)]
mod tests {
    use super::merge;
    use crate::model::SourceLocation;
    use crate::source_adapter::SourceParameterType;
    use std::collections::BTreeMap;

    /// A declaration in `file` with an optional static name and pattern.
    fn declaration(file: &str, name: Option<&str>, pattern: Option<&str>) -> SourceParameterType {
        let location = SourceLocation::new(file, 1, 1, 1, 2);
        SourceParameterType::new(
            name.map(str::to_owned),
            pattern.map(str::to_owned),
            location,
        )
    }

    /// A declaration that invalidates the registry withdraws every pattern, configured ones
    /// included, whether its file comes before or after the static declaration.
    #[test]
    fn a_registry_invalidating_declaration_withdraws_every_pattern_in_any_order() {
        let configured = BTreeMap::from([("size".to_owned(), "small|large".to_owned())]);
        let color = || declaration("b_color.rb", Some("color"), Some("red|blue"));
        for invalidating in [
            declaration("a_dynamic.rb", None, None),
            declaration("a_empty.rb", Some(""), Some("x")),
            declaration("a_builtin.rb", Some("bigdecimal"), Some("\\d+")),
        ] {
            for declarations in [
                vec![invalidating.clone(), color()],
                vec![color(), invalidating.clone()],
            ] {
                let (patterns, diagnostics) = merge(declarations, &configured);
                assert!(patterns.is_empty(), "{invalidating:?}: {patterns:?}");
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
            }
        }
        // Control: static declarations alone keep every pattern and report nothing.
        let (patterns, diagnostics) = merge(
            vec![
                color(),
                declaration("c_shape.rb", Some("shape"), Some("round")),
            ],
            &configured,
        );
        assert_eq!(
            patterns.keys().map(String::as_str).collect::<Vec<_>>(),
            ["color", "shape", "size"]
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}
