//! Syntax-tree helpers and the registration vocabulary shared by every extraction pass.
//!
//! Traversal, string-literal decoding, and the registration name tables were previously copied
//! into each pass. Keeping one definition matters beyond tidiness: a framework or alias added to
//! one copy and not another silently narrows what the analyzer can see, and nothing reports it.

use super::node_text;
use crate::model::Framework;
use std::collections::BTreeMap;
use tree_sitter::Node;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RegistrationExport {
    pub(super) canonical: String,
    pub(super) kind: RegistrationExportKind,
    pub(super) framework: Framework,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegistrationExportKind {
    Call,
    Decorator,
    Factory,
}

pub(super) type RegistrationExports = BTreeMap<String, RegistrationExport>;

/// Every registration name CukeDedup understands, including lowercase aliases.
pub(super) const REGISTRATIONS: [&str; 8] = [
    "Given",
    "When",
    "Then",
    "Step",
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
pub(super) const LEGACY_CUCUMBER_MODULE: &str = "cucumber";
pub(super) const PLAYWRIGHT_MODULE: &str = "playwright-bdd";
pub(super) const PLAYWRIGHT_DECORATORS_MODULE: &str = "playwright-bdd/decorators";
pub(super) const CYPRESS_MODULE: &str = "@badeball/cypress-cucumber-preprocessor";
pub(super) const LEGACY_CYPRESS_STEPS_MODULE: &str = "cypress-cucumber-preprocessor/steps";

/// Modules whose registration exports CukeDedup knows without resolving them.
///
/// Package subpaths are listed exactly. Prefix matching would let an unrelated package such as
/// `playwright-bdd/decorators-extra` or a private suffix masquerade as a known registration API.
/// The legacy Cypress package root is deliberately absent because it exports the file
/// preprocessor, while its documented `/steps` entrypoint exports step registrations.
pub(super) const FRAMEWORK_MODULES: [&str; 6] = [
    CUCUMBER_MODULE,
    LEGACY_CUCUMBER_MODULE,
    PLAYWRIGHT_MODULE,
    PLAYWRIGHT_DECORATORS_MODULE,
    CYPRESS_MODULE,
    LEGACY_CYPRESS_STEPS_MODULE,
];

/// Returns whether `module` is a framework whose exports are known without resolution.
pub(super) fn is_supported_module(module: &str) -> bool {
    FRAMEWORK_MODULES.contains(&module)
}

/// Returns the framework a supported module identifies.
pub(super) fn framework_for_module(module: &str) -> Framework {
    match module {
        PLAYWRIGHT_MODULE | PLAYWRIGHT_DECORATORS_MODULE => Framework::PlaywrightBdd,
        CYPRESS_MODULE | LEGACY_CYPRESS_STEPS_MODULE => Framework::CypressCucumber,
        CUCUMBER_MODULE | LEGACY_CUCUMBER_MODULE => Framework::CucumberJs,
        _ => Framework::Unknown,
    }
}

/// Returns the ordinary registration exports attributed to one framework.
pub(super) fn registration_exports_for_framework(framework: Framework) -> RegistrationExports {
    REGISTRATIONS
        .into_iter()
        .filter(|name| *name != "Step")
        .map(|name| {
            (
                name.to_owned(),
                RegistrationExport {
                    canonical: name.to_owned(),
                    kind: RegistrationExportKind::Call,
                    framework,
                },
            )
        })
        .collect()
}

/// Returns the registration exports known for one exact framework entrypoint.
///
/// `Step` is a Playwright-BDD decorator export, not a general registration export. Keeping it on
/// that exact subpath avoids inventing imports from the other supported packages.
pub(super) fn registration_exports_for_module(module: &str) -> RegistrationExports {
    let framework = framework_for_module(module);
    if module != PLAYWRIGHT_DECORATORS_MODULE {
        let mut exports = registration_exports_for_framework(framework);
        if module == PLAYWRIGHT_MODULE {
            exports.insert(
                "createBdd".to_owned(),
                RegistrationExport {
                    canonical: "createBdd".to_owned(),
                    kind: RegistrationExportKind::Factory,
                    framework,
                },
            );
        }
        return exports;
    }
    ["Given", "When", "Then", "Step"]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                RegistrationExport {
                    canonical: name.to_owned(),
                    kind: RegistrationExportKind::Decorator,
                    framework,
                },
            )
        })
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

/// Returns whether an import or export declaration carries a top-level type-only modifier.
///
/// The modifier is an anonymous direct child in tree-sitter-typescript. Inspecting that token
/// avoids confusing comments or a default import whose identifier happens to be named `type`
/// with a type-only declaration.
pub(super) fn is_type_only_declaration(node: Node<'_>) -> bool {
    has_direct_token(node, "type") || has_direct_token(node, "typeof")
}

/// Returns whether one named import or export specifier is type-only.
pub(super) fn is_type_only_specifier(node: Node<'_>) -> bool {
    has_direct_token(node, "type") || has_direct_token(node, "typeof")
}

/// Returns whether an export declaration is an `export * from ...` form.
pub(super) fn is_star_export(node: Node<'_>) -> bool {
    has_direct_token(node, "*") && !has_direct_named_child(node, "namespace_export")
}

/// Returns whether an import introduces at least one runtime binding.
pub(super) fn import_has_runtime_bindings(node: Node<'_>) -> bool {
    if is_type_only_declaration(node) {
        return false;
    }
    // TypeScript's `import value = require("module")` places the require clause directly under
    // the import statement rather than inside an `import_clause` node.
    if direct_named_child(node, "import_require_clause").is_some() {
        return true;
    }
    let Some(clause) = direct_named_child(node, "import_clause") else {
        return false;
    };
    let mut cursor = clause.walk();
    let has_runtime_binding = clause
        .named_children(&mut cursor)
        .any(|child| match child.kind() {
            "identifier" | "namespace_import" | "import_require_clause" => true,
            "named_imports" => {
                let mut cursor = child.walk();
                let has_runtime_specifier = child.named_children(&mut cursor).any(|specifier| {
                    specifier.kind() == "import_specifier" && !is_type_only_specifier(specifier)
                });
                has_runtime_specifier
            }
            _ => false,
        });
    has_runtime_binding
}

/// Returns whether an import executes a module at runtime.
///
/// Side-effect imports have no bindings but still identify a framework. Imports containing only
/// type specifiers do neither.
pub(super) fn import_has_runtime_module_reference(node: Node<'_>) -> bool {
    if is_type_only_declaration(node) {
        return false;
    }
    direct_named_child(node, "import_clause").is_none() || import_has_runtime_bindings(node)
}

/// Returns whether an export forwards at least one runtime binding.
pub(super) fn export_has_runtime_bindings(node: Node<'_>) -> bool {
    if is_type_only_declaration(node) {
        return false;
    }
    if has_direct_token(node, "*") {
        return true;
    }
    let Some(clause) = direct_named_child(node, "export_clause") else {
        return true;
    };
    let mut cursor = clause.walk();
    let has_runtime_specifier = clause.named_children(&mut cursor).any(|specifier| {
        specifier.kind() == "export_specifier" && !is_type_only_specifier(specifier)
    });
    has_runtime_specifier
}

fn has_direct_token(node: Node<'_>, token: &str) -> bool {
    (0..node.child_count()).any(|index| {
        node.child(index)
            .is_some_and(|child| !child.is_named() && child.kind() == token)
    })
}

fn has_direct_named_child(node: Node<'_>, kind: &str) -> bool {
    direct_named_child(node, kind).is_some()
}

fn direct_named_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let child = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == kind);
    child
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

    fn parse_typescript(source: &str) -> tree_sitter::Tree {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&crate::source_adapter::grammar_for_language(
                crate::source_adapter::SourceLanguage::TypeScript,
            ))
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{source}");
        tree
    }

    #[test]
    fn framework_module_table_agrees_with_its_named_constants() {
        assert_eq!(
            FRAMEWORK_MODULES,
            [
                CUCUMBER_MODULE,
                LEGACY_CUCUMBER_MODULE,
                PLAYWRIGHT_MODULE,
                PLAYWRIGHT_DECORATORS_MODULE,
                CYPRESS_MODULE,
                LEGACY_CYPRESS_STEPS_MODULE,
            ]
        );
        for module in FRAMEWORK_MODULES {
            assert!(is_supported_module(module), "{module}");
            assert_ne!(framework_for_module(module), Framework::Unknown, "{module}");
        }
        for unsupported in [
            "cypress-cucumber-preprocessor",
            "cypress-cucumber-preprocessor/steps-extra",
            "playwright-bdd/decorators-extra",
        ] {
            assert!(!is_supported_module(unsupported), "{unsupported}");
            assert_eq!(
                framework_for_module(unsupported),
                Framework::Unknown,
                "{unsupported}"
            );
        }
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
        assert_eq!(
            registration_exports_for_framework(Framework::Unknown).len() + 1,
            REGISTRATIONS.len()
        );
        assert!(!registration_exports_for_framework(Framework::Unknown).contains_key("Step"));
        assert!(registration_exports_for_module(PLAYWRIGHT_DECORATORS_MODULE).contains_key("Step"));
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

    #[test]
    fn type_only_and_star_forms_use_ast_tokens_instead_of_source_prefixes() {
        let tree = parse_typescript(
            r#"
import /* comment */ type { Given } from "types";
import type from "runtime";
import { type Given, When } from "mixed";
import "side-effect";
export /* comment */ type { Then } from "types";
export { type Then, Given } from "mixed";
export /* comment */ * from "runtime";
export * as runtime from "runtime";
"#,
        );
        let mut cursor = tree.root_node().walk();
        let declarations = tree
            .root_node()
            .named_children(&mut cursor)
            .collect::<Vec<_>>();
        assert!(is_type_only_declaration(declarations[0]));
        assert!(!is_type_only_declaration(declarations[1]));
        assert!(!is_type_only_declaration(declarations[2]));
        assert!(is_type_only_declaration(declarations[4]));
        assert!(!is_type_only_declaration(declarations[5]));
        assert!(is_star_export(declarations[6]));
        assert!(!is_star_export(declarations[7]));
        assert!(!import_has_runtime_bindings(declarations[0]));
        assert!(import_has_runtime_bindings(declarations[1]));
        assert!(import_has_runtime_bindings(declarations[2]));
        assert!(!import_has_runtime_bindings(declarations[3]));
        assert!(import_has_runtime_module_reference(declarations[3]));
        assert!(!export_has_runtime_bindings(declarations[4]));
        assert!(export_has_runtime_bindings(declarations[5]));
        assert!(export_has_runtime_bindings(declarations[6]));
        assert!(export_has_runtime_bindings(declarations[7]));

        let import_require = parse_typescript("import check = require('expect');");
        assert!(import_has_runtime_bindings(
            import_require.root_node().named_child(0).unwrap()
        ));

        let mut stack = vec![declarations[2]];
        let mut specifiers = Vec::new();
        while let Some(node) = stack.pop() {
            if node.kind() == "import_specifier" {
                specifiers.push(node);
            }
            push_named_children_reverse(node, &mut stack);
        }
        assert!(is_type_only_specifier(specifiers[0]));
        assert!(!is_type_only_specifier(specifiers[1]));
    }

    #[test]
    fn typescript_namespace_parse_shapes_are_pinned() {
        for (source, expected_shape) in [
            (
                "namespace expect { export const custom = true; }",
                "(expression_statement (internal_module",
            ),
            (
                "module expect { export const custom = true; }",
                "(module name:",
            ),
            (
                "declare namespace expect { const custom: boolean; }",
                "(ambient_declaration (internal_module",
            ),
            (
                "declare const expect: AssertionFactory;",
                "(ambient_declaration (lexical_declaration",
            ),
        ] {
            let tree = parse_typescript(source);
            assert!(
                tree.root_node().to_sexp().contains(expected_shape),
                "{source}: {}",
                tree.root_node().to_sexp()
            );
        }
    }
}
