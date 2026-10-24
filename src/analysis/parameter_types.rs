//! Merges source-declared parameter types with configured patterns into one matching registry.
use crate::source_adapter::SourceParameterType;
use std::collections::{BTreeMap, BTreeSet};

/// Identify unnamed or built-in parameter-type declarations that invalidate source registry trust.
pub(crate) fn invalidates_registry(declaration: &SourceParameterType) -> bool {
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

/// Merges `declarations`, in source order, over `configured` and returns the patterns that may
/// match steps with one warning per removed name. A declaration supersedes a configured pattern of
/// its name; a duplicate or unresolved declaration removes the name. Any unnamed, empty or
/// built-in name withdraws every pattern without a warning of its own: the adapter that extracted
/// it reports the cause. Without declarations, `configured` is returned as is.
pub fn merge_parameter_types(
    declarations: Vec<SourceParameterType>,
    configured: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut patterns = configured.clone();
    let mut seen = BTreeSet::new();
    let mut diagnostics = Vec::new();
    // fail-closed: a declaration that invalidates the registry (a name that is not static, empty,
    // or a reserved built-in) may define any parameter type, so no pattern from this merge is
    // trusted, whichever file or position it comes from. Extraction reports the located cause and
    // invalidates the registry, so the withdrawal adds no diagnostic; a duplicate or unresolved
    // declaration of such a name still warns below like any other.
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
                // Only the Ruby adapter declares parameter types; the wording is part of the
                // report contract.
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
    use super::merge_parameter_types;
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
                let (patterns, diagnostics) = merge_parameter_types(declarations, &configured);
                assert!(patterns.is_empty(), "{invalidating:?}: {patterns:?}");
                assert!(diagnostics.is_empty(), "{diagnostics:?}");
            }
        }
        // Control: static declarations alone keep every pattern and report nothing.
        let (patterns, diagnostics) = merge_parameter_types(
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

    /// Configured patterns pass through without declarations; a declaration replaces its name,
    /// and a duplicate or unresolved one removes it with one warning per declaration, in order.
    #[test]
    fn declarations_supersede_configured_patterns_and_conflicts_remove_the_name() {
        let configured = BTreeMap::from([
            ("color".to_owned(), "red".to_owned()),
            ("size".to_owned(), "small".to_owned()),
        ]);
        assert_eq!(
            merge_parameter_types(Vec::new(), &configured),
            (configured.clone(), Vec::new())
        );
        let (patterns, diagnostics) = merge_parameter_types(
            vec![declaration("a.rb", Some("color"), Some("blue"))],
            &configured,
        );
        assert_eq!(patterns["color"], "blue");
        assert_eq!(patterns["size"], "small");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let (patterns, diagnostics) = merge_parameter_types(
            vec![
                declaration("z.rb", Some("size"), None),
                declaration("a.rb", Some("color"), Some("blue")),
                declaration("b.rb", Some("color"), Some("blue")),
            ],
            &configured,
        );
        assert!(patterns.is_empty(), "{patterns:?}");
        assert_eq!(
            diagnostics,
            [
                "Ruby parameter type \"size\" has an unresolved or duplicate declaration at z.rb:1",
                "Ruby parameter type \"color\" has an unresolved or duplicate declaration at b.rb:1",
            ]
        );
    }
}
