use super::matcher::decode_js_string;
use super::module_resolver::RegistrationResolver;
use super::node_text;
use crate::model::Framework;
use anyhow::Result;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use tree_sitter::Node;

const REGISTRATIONS: [&str; 7] = [
    "Given",
    "When",
    "Then",
    "defineStep",
    "given",
    "when",
    "then",
];
const DEFAULT_REGISTRATIONS: [&str; 4] = ["Given", "When", "Then", "defineStep"];
const CUCUMBER_MODULE: &str = "@cucumber/cucumber";
const PLAYWRIGHT_MODULE: &str = "playwright-bdd";
const CYPRESS_MODULE: &str = "@badeball/cypress-cucumber-preprocessor";

pub(super) struct RegistrationNames {
    aliases: BTreeMap<String, String>,
    namespaces: BTreeMap<String, BTreeMap<String, String>>,
}

pub(super) enum RegistrationCallee<'tree, 'source> {
    Identifier(&'source str),
    Property {
        object: Node<'tree>,
        name: Cow<'source, str>,
    },
}

#[derive(Default)]
struct RegistrationDiscovery<'tree> {
    aliases: BTreeMap<String, String>,
    namespaces: BTreeMap<String, BTreeMap<String, String>>,
    assignments: Vec<(String, String)>,
    namespace_destructures: Vec<(Node<'tree>, String)>,
    shadowed_defaults: BTreeSet<String>,
}

pub(super) fn registration_name(
    function: Node<'_>,
    source: &[u8],
    registrations: &RegistrationNames,
) -> Option<(String, String)> {
    match registration_callee(function, source)? {
        RegistrationCallee::Identifier(name) => registrations
            .aliases
            .get(name)
            .cloned()
            .map(|registration| (node_text(function, source).to_owned(), registration)),
        RegistrationCallee::Property { object, name } => {
            let exports = registrations.namespaces.get(node_text(object, source))?;
            exports
                .get(name.as_ref())
                .cloned()
                .map(|registration| (node_text(function, source).to_owned(), registration))
        }
    }
}

pub(super) fn registration_callee<'tree, 'source>(
    function: Node<'tree>,
    source: &'source [u8],
) -> Option<RegistrationCallee<'tree, 'source>> {
    let function = unwrap_registration_callee(function)?;
    match function.kind() {
        "identifier" => Some(RegistrationCallee::Identifier(node_text(function, source))),
        "member_expression" => Some(RegistrationCallee::Property {
            object: unwrap_registration_callee(function.child_by_field_name("object")?)?,
            name: Cow::Borrowed(node_text(function.child_by_field_name("property")?, source)),
        }),
        "subscript_expression" => Some(RegistrationCallee::Property {
            object: unwrap_registration_callee(function.child_by_field_name("object")?)?,
            name: Cow::Owned(decode_js_string(node_text(
                function.child_by_field_name("index")?,
                source,
            ))?),
        }),
        _ => None,
    }
}

fn unwrap_registration_callee(mut function: Node<'_>) -> Option<Node<'_>> {
    loop {
        function = match function.kind() {
            "parenthesized_expression"
            | "as_expression"
            | "satisfies_expression"
            | "non_null_expression" => function.named_child(0)?,
            "type_assertion" => {
                let last_child =
                    u32::try_from(function.named_child_count().checked_sub(1)?).ok()?;
                function.named_child(last_child)?
            }
            "instantiation_expression" => function.named_child(0)?,
            _ => return Some(function),
        };
    }
}

pub(super) fn detect_framework(root: Node<'_>, source: &[u8]) -> Framework {
    let mut stack = vec![root];
    let mut cucumber = false;
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "import_statement" | "export_statement") {
            if let Some(module) = import_module(node, source) {
                match module {
                    PLAYWRIGHT_MODULE => return Framework::PlaywrightBdd,
                    CYPRESS_MODULE => return Framework::CypressCucumber,
                    CUCUMBER_MODULE => cucumber = true,
                    _ => {}
                }
            }
        } else if node.kind() == "call_expression" {
            if call_name(node, source) == Some("createBdd") {
                return Framework::PlaywrightBdd;
            }
            if call_name(node, source) == Some("require") {
                match call_string_argument(node, source) {
                    Some(PLAYWRIGHT_MODULE) => return Framework::PlaywrightBdd,
                    Some(CYPRESS_MODULE) => return Framework::CypressCucumber,
                    Some(CUCUMBER_MODULE) => cucumber = true,
                    _ => {}
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
    if cucumber {
        Framework::CucumberJs
    } else {
        Framework::Unknown
    }
}

pub(super) fn detect_registrations(
    root: Node<'_>,
    source: &[u8],
    framework: Framework,
    file_path: &Path,
    resolver: &mut RegistrationResolver,
) -> Result<RegistrationNames> {
    let mut discovered = RegistrationDiscovery::default();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_statement" => {
                let module = import_module(node, source);
                let exports = match module {
                    Some(module) if is_supported_module(module) => {
                        Some(default_registration_exports())
                    }
                    Some(module) if is_relative_module(module) => {
                        resolver.registration_exports(file_path, module)?
                    }
                    _ => None,
                };
                if !is_type_only_import(node, source)
                    && exports.as_ref().is_some_and(|exports| !exports.is_empty())
                {
                    collect_imports(
                        node,
                        source,
                        &exports.unwrap_or_default(),
                        &mut discovered.aliases,
                        &mut discovered.namespaces,
                    );
                } else {
                    collect_shadowing_imports(node, source, &mut discovered.shadowed_defaults);
                }
            }
            "export_statement" if import_module(node, source).is_some_and(is_supported_module) => {
                collect_exports(node, source, &mut discovered.aliases);
            }
            "variable_declarator" => {
                collect_variable_registration(node, source, framework, &mut discovered)
            }
            "function_declaration" | "generator_function_declaration" => {
                shadow_named_declaration(node, source, &mut discovered.shadowed_defaults)
            }
            "class_declaration"
            | "abstract_class_declaration"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration" => {
                shadow_named_declaration(node, source, &mut discovered.shadowed_defaults)
            }
            "assignment_expression" => collect_assignment(node, source, &mut discovered),
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }

    for (pattern, namespace) in &discovered.namespace_destructures {
        if let Some(exports) = discovered.namespaces.get(namespace) {
            collect_pattern_aliases(*pattern, source, exports, &mut discovered.aliases);
        }
    }

    for name in DEFAULT_REGISTRATIONS {
        if !discovered.shadowed_defaults.contains(name) && !discovered.aliases.contains_key(name) {
            discovered.aliases.insert(name.to_owned(), name.to_owned());
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for (alias, target) in &discovered.assignments {
            if let Some(registration) = discovered.aliases.get(target).cloned() {
                changed |= discovered
                    .aliases
                    .insert(alias.clone(), registration)
                    .is_none();
            }
        }
    }

    Ok(RegistrationNames {
        aliases: discovered.aliases,
        namespaces: discovered.namespaces,
    })
}

fn collect_imports(
    import: Node<'_>,
    source: &[u8],
    exports: &BTreeMap<String, String>,
    aliases: &mut BTreeMap<String, String>,
    namespaces: &mut BTreeMap<String, BTreeMap<String, String>>,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "import_specifier" => {
                if node_text(node, source).trim_start().starts_with("type ") {
                    continue;
                }
                let Some(name) = node.child_by_field_name("name") else {
                    continue;
                };
                let original = node_text(name, source);
                if let Some(registration) = exports.get(original) {
                    let alias = node
                        .child_by_field_name("alias")
                        .map_or(original, |alias| node_text(alias, source));
                    aliases.insert(alias.to_owned(), registration.clone());
                }
            }
            "namespace_import" => {
                if let Some(identifier) = first_named_kind(node, "identifier") {
                    namespaces.insert(node_text(identifier, source).to_owned(), exports.clone());
                }
            }
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_exports(export: Node<'_>, source: &[u8], aliases: &mut BTreeMap<String, String>) {
    let mut stack = vec![export];
    while let Some(node) = stack.pop() {
        if node.kind() == "export_specifier" {
            let Some(name) = node.child_by_field_name("name") else {
                continue;
            };
            let original = node_text(name, source);
            if REGISTRATIONS.contains(&original) {
                let alias = node
                    .child_by_field_name("alias")
                    .map_or(original, |alias| node_text(alias, source));
                aliases.insert(alias.to_owned(), original.to_owned());
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_shadowing_imports(
    import: Node<'_>,
    source: &[u8],
    shadowed_defaults: &mut BTreeSet<String>,
) {
    let mut stack = vec![import];
    while let Some(node) = stack.pop() {
        if node.kind() == "import_specifier" {
            let local = node
                .child_by_field_name("alias")
                .or_else(|| node.child_by_field_name("name"));
            if let Some(local) = local {
                let local = node_text(local, source);
                if DEFAULT_REGISTRATIONS.contains(&local) {
                    shadowed_defaults.insert(local.to_owned());
                }
            }
        }
        push_named_children_reverse(node, &mut stack);
    }
}

fn collect_variable_registration<'tree>(
    declaration: Node<'tree>,
    source: &[u8],
    framework: Framework,
    discovered: &mut RegistrationDiscovery<'tree>,
) {
    let Some(name) = declaration.child_by_field_name("name") else {
        return;
    };
    let Some(value) = declaration.child_by_field_name("value") else {
        shadow_default_name(name, source, &mut discovered.shadowed_defaults);
        return;
    };

    if value.kind() == "call_expression" {
        let function = call_name(value, source);
        let supported_require = function == Some("require")
            && call_string_argument(value, source).is_some_and(is_supported_module);
        let create_bdd = framework == Framework::PlaywrightBdd && function == Some("createBdd");
        if supported_require || create_bdd {
            let exports = default_registration_exports();
            match name.kind() {
                "object_pattern" => {
                    collect_pattern_aliases(name, source, &exports, &mut discovered.aliases)
                }
                "identifier" => {
                    discovered
                        .namespaces
                        .insert(node_text(name, source).to_owned(), exports);
                }
                _ => {}
            }
        }
    } else if name.kind() == "object_pattern" && value.kind() == "identifier" {
        discovered
            .namespace_destructures
            .push((name, node_text(value, source).to_owned()));
    } else if name.kind() == "identifier" && value.kind() == "identifier" {
        discovered.assignments.push((
            node_text(name, source).to_owned(),
            node_text(value, source).to_owned(),
        ));
    }

    if name.kind() == "identifier" {
        let local = node_text(name, source);
        if DEFAULT_REGISTRATIONS.contains(&local) {
            discovered.shadowed_defaults.insert(local.to_owned());
        }
    }
}

fn collect_assignment(
    assignment: Node<'_>,
    source: &[u8],
    discovered: &mut RegistrationDiscovery<'_>,
) {
    let (Some(left), Some(right)) = (
        assignment.child_by_field_name("left"),
        assignment.child_by_field_name("right"),
    ) else {
        return;
    };
    if left.kind() != "identifier" {
        return;
    }
    let local = node_text(left, source);
    if DEFAULT_REGISTRATIONS.contains(&local) {
        discovered.shadowed_defaults.insert(local.to_owned());
    }
    if right.kind() == "identifier" {
        discovered
            .assignments
            .push((local.to_owned(), node_text(right, source).to_owned()));
    }
}

fn shadow_named_declaration(
    declaration: Node<'_>,
    source: &[u8],
    shadowed_defaults: &mut BTreeSet<String>,
) {
    if let Some(name) = declaration.child_by_field_name("name") {
        shadow_default_name(name, source, shadowed_defaults);
    }
}

fn shadow_default_name(name: Node<'_>, source: &[u8], shadowed_defaults: &mut BTreeSet<String>) {
    if name.kind() == "identifier" || name.kind() == "type_identifier" {
        let name = node_text(name, source);
        if DEFAULT_REGISTRATIONS.contains(&name) {
            shadowed_defaults.insert(name.to_owned());
        }
    }
}

fn import_module<'a>(import: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let module = import.child_by_field_name("source")?;
    string_literal(module, source)
}

fn call_name<'a>(call: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let function = call.child_by_field_name("function")?;
    (function.kind() == "identifier").then(|| node_text(function, source))
}

fn call_string_argument<'a>(call: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let argument = arguments.named_children(&mut cursor).next()?;
    string_literal(argument, source)
}

fn string_literal<'a>(node: Node<'_>, source: &'a [u8]) -> Option<&'a str> {
    let text = node_text(node, source);
    let quote = text.as_bytes().first().copied()?;
    if !matches!(quote, b'\'' | b'"') || text.as_bytes().last().copied()? != quote {
        return None;
    }
    text.get(1..text.len() - 1)
}

fn is_supported_module(module: &str) -> bool {
    matches!(module, CUCUMBER_MODULE | PLAYWRIGHT_MODULE | CYPRESS_MODULE)
}

fn is_relative_module(module: &str) -> bool {
    module == "." || module == ".." || module.starts_with("./") || module.starts_with("../")
}

fn first_named_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == kind);
    found
}

fn collect_pattern_aliases(
    pattern: Node<'_>,
    source: &[u8],
    exports: &BTreeMap<String, String>,
    aliases: &mut BTreeMap<String, String>,
) {
    let mut cursor = pattern.walk();
    for child in pattern.named_children(&mut cursor) {
        let (original, alias) = match child.kind() {
            "shorthand_property_identifier_pattern" => {
                let name = node_text(child, source);
                (name, name)
            }
            "pair_pattern" => {
                let (Some(key), Some(value)) = (
                    child.child_by_field_name("key"),
                    child.child_by_field_name("value"),
                ) else {
                    continue;
                };
                (node_text(key, source), node_text(value, source))
            }
            _ => continue,
        };
        if let Some(registration) = exports.get(original) {
            if alias.chars().all(is_identifier_character) {
                aliases.insert(alias.to_owned(), registration.clone());
            }
        }
    }
}

fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '$')
}

fn default_registration_exports() -> BTreeMap<String, String> {
    REGISTRATIONS
        .into_iter()
        .map(|name| (name.to_owned(), name.to_owned()))
        .collect()
}

fn is_type_only_import(import: Node<'_>, source: &[u8]) -> bool {
    let Some(after_import) = node_text(import, source)
        .trim_start()
        .strip_prefix("import")
    else {
        return false;
    };
    let after_import = after_import.trim_start();
    after_import.strip_prefix("type").is_some_and(|rest| {
        rest.is_empty() || rest.starts_with(char::is_whitespace) || rest.starts_with('{')
    })
}

fn push_named_children_reverse<'tree>(node: Node<'tree>, stack: &mut Vec<Node<'tree>>) {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    stack.extend(children.into_iter().rev());
}
