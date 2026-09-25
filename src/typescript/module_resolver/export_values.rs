//! Decides what a CommonJS export value can carry.
//!
//! `module.exports = …` and `exports.x = …` accept any expression, so the analyzer can model only
//! some of them. Everything it cannot model must fail closed and mark the module incomplete. This
//! module narrows "cannot model" to the values that could actually hide a registration. A value is
//! inert when it provably cannot:
//!
//! - a literal;
//! - a function or class whose body never names a registration or a non-builtin module;
//! - an array or object made only of inert values.
//!
//! An identifier resolves through its single top-level declaration. It stays opaque once its value
//! may have changed: it was reassigned, given a property that is not inert, or reached another
//! binding or a call, where a registration could be attached to it.

use super::super::ast::{
    call_string_argument, is_const_declaration, push_named_children_reverse, string_literal,
};
use super::super::frameworks::REGISTRATIONS;
use super::super::matcher::static_string_key;
use super::super::node_text;
use super::{commonjs_export_target, require_specifier};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::Node;

/// How far an identifier is followed through `const` aliases before the value counts as opaque.
const MAX_ALIAS_DEPTH: usize = 8;

/// Member names that read as a registration even on an unknown object (`cucumber.Given`). The
/// lowercase forms are left out because `.then(…)` is ordinary promise code.
const REGISTRATION_MEMBERS: [&str; 5] = ["Given", "When", "Then", "Step", "defineStep"];

/// Node's built-in modules. They cannot register steps, so requiring one does not taint a value.
/// Any other module might re-export a registration.
const NODE_BUILTIN_MODULES: [&str; 32] = [
    "assert",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "crypto",
    "dgram",
    "dns",
    "events",
    "fs",
    "fs/promises",
    "http",
    "http2",
    "https",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "querystring",
    "readline",
    "stream",
    "string_decoder",
    "timers",
    "tls",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "zlib",
];

/// What an export value is, as far as registrations are concerned.
pub(super) enum ExportValue<'tree> {
    /// Provably cannot carry a registration.
    Inert,
    /// An object literal whose properties are read as individual exports.
    Object(Node<'tree>),
    /// `require('…')`: the whole module is re-exported.
    Module(&'tree str),
    /// Anything else. It may hide a registration, so the module fails closed.
    Opaque,
}

/// What module scope knows about the names an export value can refer to.
pub(super) struct ExportScope<'tree> {
    /// A top-level name's value: a `const` initializer or a function or class declaration. `None`
    /// marks a name with no single static value — `let`, `var`, a destructured binding, an import,
    /// or a name declared twice.
    declarations: BTreeMap<String, Option<Node<'tree>>>,
    /// Names that may carry a registration: an undeclared registration global, a binding from a
    /// non-builtin module, or a declaration that refers to one.
    tainted: BTreeSet<String>,
    /// Top-level names whose value may have changed after declaration; see `mutated_names`.
    mutated: BTreeSet<String>,
}

impl<'tree> ExportScope<'tree> {
    pub(super) fn new(root: Node<'tree>, source: &[u8]) -> Self {
        let mut declarations = BTreeMap::new();
        // Each declaration's names with the subtree that decides whether they are tainted.
        let mut provenance: Vec<(Vec<String>, Node<'tree>)> = Vec::new();
        let mut cursor = root.walk();
        for statement in root.named_children(&mut cursor) {
            let statement = match statement.kind() {
                "export_statement" => match statement.child_by_field_name("declaration") {
                    Some(declaration) => declaration,
                    None => continue,
                },
                _ => statement,
            };
            match statement.kind() {
                "function_declaration"
                | "generator_function_declaration"
                | "class_declaration"
                | "abstract_class_declaration" => {
                    // A declaration always carries its name; error recovery inserts a MISSING node.
                    if let Some(name) = statement.child_by_field_name("name") {
                        let name = node_text(name, source).to_owned();
                        declare(&mut declarations, name.clone(), Some(statement));
                        provenance.push((vec![name], statement));
                    }
                }
                "lexical_declaration" | "variable_declaration" => {
                    let constant = is_const_declaration(statement);
                    let mut cursor = statement.walk();
                    for declarator in statement.named_children(&mut cursor) {
                        let (Some(name), value) = (
                            declarator.child_by_field_name("name"),
                            declarator.child_by_field_name("value"),
                        ) else {
                            continue;
                        };
                        let names = bound_names(name, source);
                        let single = name.kind() == "identifier" && constant;
                        for bound in &names {
                            declare(&mut declarations, bound.clone(), value.filter(|_| single));
                        }
                        provenance.push((names, declarator));
                    }
                }
                "import_statement" => {
                    let names = bound_names(statement, source);
                    for bound in &names {
                        declare(&mut declarations, bound.clone(), None);
                    }
                    provenance.push((names, statement));
                }
                _ => {}
            }
        }

        // A registration name the module does not declare itself is the ambient global.
        let mut tainted: BTreeSet<String> = REGISTRATIONS
            .iter()
            .filter(|name| !declarations.contains_key(**name))
            .map(|name| (*name).to_owned())
            .collect();
        // Propagate until nothing changes. Each pass taints at least one more declaration or
        // stops, so the loop runs at most once per declaration.
        loop {
            let before = tainted.len();
            for (names, subtree) in &provenance {
                if names.iter().any(|name| !tainted.contains(name))
                    && references_taint(*subtree, source, &tainted)
                {
                    tainted.extend(names.iter().cloned());
                }
            }
            if tainted.len() == before {
                break;
            }
        }

        let mut scope = Self {
            declarations,
            tainted,
            mutated: BTreeSet::new(),
        };
        scope.mutated = scope.mutated_names(root, source);
        scope
    }

    /// Classifies one export value.
    pub(super) fn classify(&self, value: Node<'tree>, source: &'tree [u8]) -> ExportValue<'tree> {
        let mut value = value;
        for _ in 0..MAX_ALIAS_DEPTH {
            while value.kind() == "parenthesized_expression" {
                match value.named_child(0) {
                    Some(inner) => value = inner,
                    None => return ExportValue::Opaque,
                }
            }
            if let Some(module) = require_specifier(value, source) {
                return ExportValue::Module(module);
            }
            if self.is_inert(value, source) {
                return ExportValue::Inert;
            }
            match value.kind() {
                "object" => return ExportValue::Object(value),
                "identifier" => {
                    let name = node_text(value, source);
                    if self.mutated.contains(name) {
                        return ExportValue::Opaque;
                    }
                    match self.declarations.get(name) {
                        // A function or class declaration is inert unless it refers to a
                        // registration, which would make it a possible wrapper.
                        Some(Some(declaration)) if is_callable_declaration(*declaration) => {
                            return if self.tainted.contains(name) {
                                ExportValue::Opaque
                            } else {
                                ExportValue::Inert
                            };
                        }
                        // A `const` is followed to its initializer, which is classified in turn.
                        Some(Some(initializer)) => value = *initializer,
                        _ => return ExportValue::Opaque,
                    }
                }
                _ => return ExportValue::Opaque,
            }
        }
        ExportValue::Opaque
    }

    /// Whether `value` provably cannot carry a registration, without following identifiers.
    pub(super) fn is_inert(&self, value: Node<'_>, source: &[u8]) -> bool {
        let mut stack = vec![value];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "string" | "template_string" | "number" | "true" | "false" | "null"
                | "undefined" | "regex" | "comment" => {}
                "identifier" if node_text(node, source) == "undefined" => {}
                "function_expression"
                | "arrow_function"
                | "generator_function"
                | "class"
                | "method_definition" => {
                    if references_taint(node, source, &self.tainted) {
                        return false;
                    }
                }
                "parenthesized_expression" | "array" | "object" => {
                    push_named_children_reverse(node, &mut stack);
                }
                // A key never carries a registration, so only the value is checked.
                "pair" => match node.child_by_field_name("value") {
                    Some(value) => stack.push(value),
                    None => return false,
                },
                _ => return false,
            }
        }
        true
    }

    /// Finds top-level names that may have been changed after declaration, anywhere in the file.
    ///
    /// Only the uses listed below leave a name's value as declared; every other use counts as an
    /// escape, because once the value reaches another binding or a call it can be written through
    /// from there. Only a direct property can hold a registration an importer reaches
    /// (`api.Given`), so a write deeper than that (`api.nested.Given = …`) cannot hide one.
    /// Shadowing is ignored, which only ever makes a name opaque rather than inert.
    fn mutated_names(&self, root: Node<'_>, source: &[u8]) -> BTreeSet<String> {
        let mut mutated = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            push_named_children_reverse(node, &mut stack);
            let escapes = match node.kind() {
                "identifier" => {
                    let name = node_text(node, source);
                    self.declarations.contains_key(name) && !self.keeps_value(node, source)
                }
                // `{ api }` copies the value into another object.
                "shorthand_property_identifier" => {
                    self.declarations.contains_key(node_text(node, source))
                }
                _ => false,
            };
            if escapes {
                mutated.insert(node_text(node, source).to_owned());
            }
        }
        mutated
    }

    /// Whether one occurrence of a declared name leaves its value as declared.
    fn keeps_value(&self, occurrence: Node<'_>, source: &[u8]) -> bool {
        let Some(parent) = occurrence.parent() else {
            return true;
        };
        let is_field = |field: &str| parent.child_by_field_name(field) == Some(occurrence);
        match parent.kind() {
            // The declaration itself.
            "variable_declarator"
            | "function_declaration"
            | "generator_function_declaration"
            | "class_declaration"
            | "abstract_class_declaration" => is_field("name"),
            // `module.exports = api`, `exports.api = api`: the use being classified.
            "assignment_expression" if is_field("right") => parent
                .child_by_field_name("left")
                .is_some_and(|left| commonjs_export_target(left, source).is_some()),
            // `api.x`: a read, unless it is written or called with a value that is not inert.
            "member_expression" | "subscript_expression" if is_field("object") => {
                self.member_use_keeps_value(parent, source)
            }
            // Calling or constructing it cannot change it; its own body is checked separately.
            "call_expression" | "new_expression" => is_field("function") || is_field("constructor"),
            // Reading it as an operand.
            "binary_expression" | "unary_expression" => true,
            _ => false,
        }
    }

    /// Whether a direct property use (`api.x`, `api[x]`) leaves the object as declared.
    fn member_use_keeps_value(&self, member: Node<'_>, source: &[u8]) -> bool {
        let Some(parent) = member.parent() else {
            return true;
        };
        match parent.kind() {
            "assignment_expression" | "augmented_assignment_expression"
                if parent.child_by_field_name("left") == Some(member) =>
            {
                parent
                    .child_by_field_name("right")
                    .is_some_and(|right| self.is_inert(right, source))
            }
            // `api.register(Given)` can attach a registration through a method.
            "call_expression" if parent.child_by_field_name("function") == Some(member) => {
                let mut cursor = parent.walk();
                let arguments = parent.child_by_field_name("arguments");
                arguments.is_none_or(|arguments| {
                    let all_inert = arguments
                        .named_children(&mut cursor)
                        .all(|argument| self.is_inert(argument, source));
                    all_inert
                })
            }
            _ => true,
        }
    }
}

fn declare<'tree>(
    declarations: &mut BTreeMap<String, Option<Node<'tree>>>,
    name: String,
    value: Option<Node<'tree>>,
) {
    declarations
        .entry(name)
        .and_modify(|existing| *existing = None)
        .or_insert(value);
}

fn is_callable_declaration(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "function_declaration"
            | "generator_function_declaration"
            | "class_declaration"
            | "abstract_class_declaration"
    )
}

/// Every local name a declarator pattern or import statement binds.
fn bound_names(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "identifier" | "shorthand_property_identifier_pattern" => {
                names.push(node_text(node, source).to_owned());
                continue;
            }
            // A default value or a renamed key is not a binding; only the pattern side is.
            "assignment_pattern" | "object_assignment_pattern" => {
                if let Some(left) = node.child_by_field_name("left") {
                    stack.push(left);
                }
                continue;
            }
            "pair_pattern" => {
                if let Some(value) = node.child_by_field_name("value") {
                    stack.push(value);
                }
                continue;
            }
            // An import binds its alias when one is written and its name otherwise.
            "import_specifier" => {
                if let Some(local) = node
                    .child_by_field_name("alias")
                    .or_else(|| node.child_by_field_name("name"))
                {
                    names.push(node_text(local, source).to_owned());
                }
                continue;
            }
            // The module specifier of an import is a string, never a binding.
            "string" => continue,
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }
    names
}

/// Whether `node` refers to anything that may be a registration: a tainted name, a registration
/// member, or a module other than a Node built-in.
fn references_taint(node: Node<'_>, source: &[u8], tainted: &BTreeSet<String>) -> bool {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "identifier" | "shorthand_property_identifier" => {
                if tainted.contains(node_text(node, source)) {
                    return true;
                }
            }
            "property_identifier" => {
                if REGISTRATION_MEMBERS.contains(&node_text(node, source)) {
                    return true;
                }
            }
            // `bdd['Given']` reaches a registration as `bdd.Given` does. A string key that cannot
            // be decoded, or a template key, might spell one, so it counts too. A dynamic key
            // (`list[index]`) is not registration evidence, as on the registration callee path.
            "subscript_expression" => {
                let registration_key =
                    node.child_by_field_name("index")
                        .is_some_and(|index| match index.kind() {
                            "string" => static_string_key(index, source)
                                .is_none_or(|key| REGISTRATION_MEMBERS.contains(&key.as_str())),
                            "template_string" => true,
                            _ => false,
                        });
                if registration_key {
                    return true;
                }
            }
            "call_expression" if is_module_load(node, source) => {
                let builtin = call_string_argument(node, source).is_some_and(is_node_builtin);
                if !builtin {
                    return true;
                }
            }
            "import_statement" => {
                let builtin = node
                    .child_by_field_name("source")
                    .and_then(|module| string_literal(module, source))
                    .is_some_and(is_node_builtin);
                if !builtin {
                    return true;
                }
            }
            _ => {}
        }
        push_named_children_reverse(node, &mut stack);
    }
    false
}

/// `require(…)` or a dynamic `import(…)`.
fn is_module_load(call: Node<'_>, source: &[u8]) -> bool {
    call.child_by_field_name("function")
        .is_some_and(|function| match function.kind() {
            "import" => true,
            "identifier" => node_text(function, source) == "require",
            _ => false,
        })
}

fn is_node_builtin(module: &str) -> bool {
    module.starts_with("node:") || NODE_BUILTIN_MODULES.contains(&module)
}
