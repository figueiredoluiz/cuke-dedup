//! Syntax-tree helpers and the registration vocabulary shared by every extraction pass.
//!
//! Traversal, string-literal decoding, and the registration name tables were previously copied
//! into each pass. Keeping one definition matters beyond tidiness: a framework or alias added to
//! one copy and not another silently narrows what the analyzer can see, and nothing reports it.

use super::node_text;
use crate::model::Framework;
use std::collections::BTreeMap;
use tree_sitter::Node;

/// Every registration name CukeDedup understands, including lowercase aliases.
pub(super) const REGISTRATIONS: [&str; 7] = [
    "Given",
    "When",
    "Then",
    "defineStep",
    "given",
    "when",
    "then",
];

/// Registration names assumed to be globals when nothing in the file shadows them.
///
/// The lowercase aliases are deliberately absent: `given` is a plausible identifier in ordinary
/// code, so treating a bare call as a registration would invent step definitions.
pub(super) const DEFAULT_REGISTRATIONS: [&str; 4] = ["Given", "When", "Then", "defineStep"];

pub(super) const CUCUMBER_MODULE: &str = "@cucumber/cucumber";
pub(super) const PLAYWRIGHT_MODULE: &str = "playwright-bdd";
pub(super) const CYPRESS_MODULE: &str = "@badeball/cypress-cucumber-preprocessor";

/// Modules whose registration exports CukeDedup knows without resolving them.
pub(super) const FRAMEWORK_MODULES: [&str; 3] =
    [CUCUMBER_MODULE, PLAYWRIGHT_MODULE, CYPRESS_MODULE];

/// Returns whether `module` is a framework whose exports are known without resolution.
pub(super) fn is_supported_module(module: &str) -> bool {
    FRAMEWORK_MODULES.contains(&module)
}

/// Returns the framework a supported module identifies.
pub(super) fn framework_for_module(module: &str) -> Framework {
    match module {
        PLAYWRIGHT_MODULE => Framework::PlaywrightBdd,
        CYPRESS_MODULE => Framework::CypressCucumber,
        CUCUMBER_MODULE => Framework::CucumberJs,
        _ => Framework::Unknown,
    }
}

/// Returns the identity export map for every known registration name.
pub(super) fn default_registration_exports() -> BTreeMap<String, String> {
    REGISTRATIONS
        .into_iter()
        .map(|name| (name.to_owned(), name.to_owned()))
        .collect()
}

/// Returns whether `module` is a relative specifier rather than a package or alias.
pub(super) fn is_relative_module(module: &str) -> bool {
    module == "." || module == ".." || module.starts_with("./") || module.starts_with("../")
}

/// Pushes a node's named children so a stack-based walk visits them in source order.
///
/// Children are appended and reversed in place rather than collected into a temporary vector.
/// Measured against a 8,000-definition corpus this is within noise of the previous form — parsing
/// dominates — so it is kept for the single definition, not for the allocation.
pub(super) fn push_named_children_reverse<'tree>(node: Node<'tree>, stack: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    let start = stack.len();
    stack.extend(node.named_children(&mut cursor));
    stack[start..].reverse();
}

/// Returns the module specifier of an import or export statement.
pub(super) fn import_module<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    string_literal(node.child_by_field_name("source")?, source)
}

/// Returns the contents of a single- or double-quoted string literal node.
pub(super) fn string_literal<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let text = node_text(node, source);
    let quote = text.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') || text.as_bytes().last().copied()? != quote {
        return None;
    }
    text.get(1..text.len() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_module_table_agrees_with_its_named_constants() {
        assert_eq!(
            FRAMEWORK_MODULES,
            [CUCUMBER_MODULE, PLAYWRIGHT_MODULE, CYPRESS_MODULE]
        );
        for module in FRAMEWORK_MODULES {
            assert!(is_supported_module(module), "{module}");
            assert_ne!(framework_for_module(module), Framework::Unknown, "{module}");
        }
        assert!(!is_supported_module("cucumber"));
        assert_eq!(framework_for_module("cucumber"), Framework::Unknown);
    }

    #[test]
    fn default_registrations_are_a_subset_of_every_known_registration() {
        for name in DEFAULT_REGISTRATIONS {
            assert!(REGISTRATIONS.contains(&name), "{name}");
        }
        // Lowercase aliases resolve through imports but are never assumed to be globals.
        for name in ["given", "when", "then"] {
            assert!(REGISTRATIONS.contains(&name));
            assert!(!DEFAULT_REGISTRATIONS.contains(&name));
        }
        assert_eq!(default_registration_exports().len(), REGISTRATIONS.len());
    }

    #[test]
    fn relative_specifiers_are_distinguished_from_packages_and_aliases() {
        for module in [".", "..", "./world", "../support/world"] {
            assert!(is_relative_module(module), "{module}");
        }
        for module in ["@support/world", "#support", "playwright-bdd", ".hidden"] {
            assert!(!is_relative_module(module), "{module}");
        }
    }
}
