use super::*;

/// Parse a TypeScript fixture and require a syntax-error-free tree.
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

/// Verify relative module specifiers exclude packages, aliases, and dot-prefixed names.
#[test]
fn relative_specifiers_are_distinguished_from_packages_and_aliases() {
    for module in [".", "..", "./world", "../support/world"] {
        assert!(is_relative_module(module), "{module}");
    }
    for module in ["@support/world", "#support", "playwright-bdd", ".hidden"] {
        assert!(!is_relative_module(module), "{module}");
    }
}

/// Verify AST tokens distinguish erased declarations, runtime bindings, and plain star exports despite comments.
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

/// Pin parser node shapes for runtime namespaces, modules, and ambient declarations.
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
