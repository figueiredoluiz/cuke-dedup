//! Shared syntax-tree helpers; framework registration metadata lives in `frameworks`.

use super::node_text;
use tree_sitter::Node;

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

/// Returns the first string-literal argument of a call expression, if any.
pub(super) fn call_string_argument<'a>(call: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let argument = arguments.named_children(&mut cursor).next()?;
    string_literal(argument, source)
}

/// Returns whether a variable declarator belongs to a top-level declaration, either directly
/// under the program or under a single `export` statement.
///
/// A declarator always sits under a declaration, which always has a parent of its own, so the
/// missing-ancestor cases are folded into the chain rather than returned early: they cannot be
/// reached from a parsed tree and an early return would be an untestable branch.
pub(super) fn is_top_level_variable(declarator: Node<'_>) -> bool {
    declarator
        .parent()
        .and_then(|declaration| declaration.parent())
        .is_some_and(|parent| match parent.kind() {
            "program" => true,
            "export_statement" => parent
                .parent()
                .is_some_and(|ancestor| ancestor.kind() == "program"),
            _ => false,
        })
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
