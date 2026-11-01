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
                diagnostics.push(format!(
                    "{} parameter type {name:?} has an unresolved or duplicate declaration at {}:{}",
                    declaration.domain.display_name,
                    declaration.location.path.display(),
                    declaration.location.line
                ));
            }
        }
    }
    if untrusted {
        patterns.clear();
    }
    (patterns, diagnostics)
}

#[cfg(test)]
mod tests;
