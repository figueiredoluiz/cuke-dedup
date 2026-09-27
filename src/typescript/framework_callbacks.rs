//! Lexical provenance for callback-scoped Jest steps and the Vitest plugin's exports.
//!
//! A spelling never confers trust. Imports, immutable aliases and parameters of proven
//! synchronous framework callbacks are the only origins; writes invalidate the binding.

use super::assertions::{
    collect_binding_names, collect_binding_scopes, is_erased_declaration, is_local_scope,
    nearest_function_scope,
};
use super::ast::{
    call_string_argument, import_has_runtime_bindings, import_statement_module,
    is_const_declaration, push_named_children_reverse,
};
use super::frameworks::{
    is_supported_module, registration_exports_for_module, RegistrationExport,
    RegistrationExportKind, RegistrationExports,
};
use super::module_resolver::RegistrationResolver;
use super::node_text;
use super::registrations::{
    for_each_import_binding, registration_callee, unwrap_registration_callee, ImportBinding,
    RegistrationCallee,
};
use crate::model::Framework;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use tree_sitter::Node;

type Key = (usize, String);
type Registration = (String, String, Framework);

#[derive(Default)]
pub(super) struct FrameworkCalls {
    registrations: BTreeMap<usize, Registration>,
    pub(super) handled: BTreeSet<usize>,
    pub(super) incomplete: BTreeSet<usize>,
    pub(super) shadowed: BTreeSet<usize>,
}

impl FrameworkCalls {
    pub(super) fn registration(&self, callee: Node<'_>) -> Option<Registration> {
        self.registrations.get(&callee.start_byte()).cloned()
    }
}

pub(super) fn is_new_framework(framework: Framework) -> bool {
    matches!(
        framework,
        Framework::JestCucumber | Framework::VitestCucumber
    )
}

#[derive(Clone)]
enum Value {
    Export(RegistrationExport),
    Fallback(RegistrationExport),
    Namespace(RegistrationExports),
    Scenario,
    Steps,
    /// Framework evidence survived, but its exact callable value is unproved.
    Unknown,
}

impl Value {
    fn property(self, name: &str) -> Option<Self> {
        match self {
            Self::Namespace(exports) => exports.get(name).cloned().map(Self::Export),
            Self::Scenario if matches!(name, "only" | "skip" | "concurrent") => {
                Some(Self::Scenario)
            }
            Self::Steps
                if matches!(
                    name,
                    "given" | "when" | "then" | "and" | "but" | "defineStep"
                ) =>
            {
                Some(Self::Export(RegistrationExport {
                    canonical: name.to_owned(),
                    kind: RegistrationExportKind::Call,
                    framework: Framework::JestCucumber,
                }))
            }
            _ => None,
        }
    }
}

#[derive(Clone)]
enum Origin<'tree> {
    Imported(Value),
    Required(Node<'tree>, Value),
    Initializer(Node<'tree>),
    Parameter(Node<'tree>),
    Forwarded(Node<'tree>),
}

struct Binding<'tree> {
    origin: Origin<'tree>,
    properties: Vec<String>,
    certain: bool,
}

struct Bindings<'tree, 'source> {
    root: usize,
    source: &'source [u8],
    scopes: BTreeMap<usize, BTreeSet<String>>,
    parents: BTreeMap<usize, usize>,
    enclosing: BTreeMap<usize, usize>,
    bindings: BTreeMap<Key, Option<Binding<'tree>>>,
    writes: BTreeSet<Key>,
    requires: BTreeMap<usize, Value>,
    registrations: &'source super::registrations::RegistrationNames,
    forwarded_targets: BTreeSet<usize>,
}

impl<'tree> Bindings<'tree, '_> {
    fn shadow_declaration(&mut self, node: Node<'tree>, root: Node<'tree>) {
        if let Some(name) = node.child_by_field_name("name") {
            let key = self.key(node.parent().unwrap_or(root), node_text(name, self.source));
            let target = (node.kind() == "function_declaration"
                && !node
                    .children(&mut node.walk())
                    .any(|child| child.kind() == "async"))
            .then(|| {
                super::registrations::forwarded_callee(
                    node.child_by_field_name("body")?,
                    node_text(name, self.source),
                    &super::registrations::plain_parameter_names(
                        node.child_by_field_name("parameters")?,
                        self.source,
                    )?,
                    self.source,
                )
            })
            .flatten();
            self.forwarded_targets
                .extend(target.map(|target| target.start_byte()));
            let binding = target.map(|target| Binding {
                origin: Origin::Forwarded(target),
                properties: Vec::new(),
                certain: true,
            });
            self.bindings
                .entry(key)
                .and_modify(|binding| *binding = None)
                .or_insert(binding);
        }
    }

    fn loader_available(&self, node: Node<'_>, ignore_writes: bool) -> bool {
        let key = self.key(node, "require");
        !self.bindings.contains_key(&key)
            && (ignore_writes || !self.writes.contains(&key))
            && !self
                .scopes
                .get(&key.0)
                .is_some_and(|names| names.contains("require"))
    }

    fn key(&self, node: Node<'_>, name: &str) -> Key {
        let mut scope = self.enclosing[&node.id()];
        while scope != self.root {
            if self
                .scopes
                .get(&scope)
                .is_some_and(|names| names.contains(name))
            {
                break;
            }
            scope = self.parents[&scope];
        }
        (scope, name.to_owned())
    }

    fn bind(&mut self, pattern: Node<'tree>, origin: Origin<'tree>, certain: bool) {
        let mut pending = vec![(pattern, Vec::new(), certain)];
        while let Some((pattern, properties, certain)) = pending.pop() {
            match pattern.kind() {
                "identifier" | "shorthand_property_identifier_pattern" => {
                    let key = self.key(pattern, node_text(pattern, self.source));
                    let binding = Binding {
                        origin: origin.clone(),
                        properties,
                        certain,
                    };
                    self.bindings
                        .entry(key)
                        .and_modify(|value| *value = None)
                        .or_insert(Some(binding));
                }
                "required_parameter" => {
                    if let Some(pattern) = pattern.child_by_field_name("pattern") {
                        pending.push((pattern, properties, certain));
                    }
                }
                "object_pattern" => {
                    let mut cursor = pattern.walk();
                    for property in pattern.named_children(&mut cursor) {
                        let (name, target) = match property.kind() {
                            "shorthand_property_identifier_pattern" => (
                                Some(node_text(property, self.source).to_owned()),
                                Some(property),
                            ),
                            "pair_pattern" => (
                                property.child_by_field_name("key").and_then(|key| {
                                    let key = if key.kind() == "computed_property_name" {
                                        key.named_child(0)?
                                    } else {
                                        key
                                    };
                                    if key.kind() == "property_identifier" {
                                        Some(node_text(key, self.source).to_owned())
                                    } else {
                                        super::matcher::static_string_key(key, self.source)
                                    }
                                }),
                                property.child_by_field_name("value"),
                            ),
                            _ => (None, None),
                        };
                        if let (Some(name), Some(target)) = (name, target) {
                            let mut path = properties.clone();
                            path.push(name);
                            pending.push((target, path, certain));
                        } else {
                            pending.push((property, properties.clone(), false));
                        }
                    }
                }
                _ => {
                    let mut names = BTreeSet::new();
                    collect_binding_names(pattern, self.source, &mut names);
                    for name in names {
                        self.bindings.insert(
                            self.key(pattern, &name),
                            Some(Binding {
                                origin: origin.clone(),
                                properties: properties.clone(),
                                certain: false,
                            }),
                        );
                    }
                }
            }
        }
    }

    fn value(&self, node: Node<'tree>, depth: usize, ignore_writes: bool) -> Option<Value> {
        if depth > 32 {
            return None;
        }
        let node = unwrap_registration_callee(node)?;
        let callee = if node.kind() == "shorthand_property_identifier" {
            Some(RegistrationCallee::Identifier(node_text(node, self.source)))
        } else {
            registration_callee(node, self.source)
        };
        match callee {
            Some(RegistrationCallee::Identifier(name)) => {
                let key = self.key(node, name);
                if !ignore_writes && self.writes.contains(&key) {
                    return self.value(node, depth + 1, true).map(|_| Value::Unknown);
                }
                if key.0 == self.root {
                    if let Some(export) = self.registrations.fallback_export(name) {
                        return Some(Value::Fallback(export.clone()));
                    }
                }
                let binding = self.bindings.get(&key)?.as_ref()?;
                let mut value = match &binding.origin {
                    Origin::Imported(value) => Some(value.clone()),
                    Origin::Required(import, value) => self
                        .loader_available(*import, ignore_writes)
                        .then(|| value.clone()),
                    Origin::Initializer(value) if value.end_byte() <= node.start_byte() => {
                        self.value(*value, depth + 1, ignore_writes)
                    }
                    Origin::Parameter(callback) => {
                        self.callback_value(*callback, depth + 1, ignore_writes)
                    }
                    Origin::Forwarded(target) => self
                        .value(*target, depth + 1, ignore_writes)
                        .filter(|value| {
                            matches!(value, Value::Export(export) | Value::Fallback(export)
                            if export.kind == RegistrationExportKind::Call)
                        }),
                    _ => None,
                }?;
                if !binding.certain {
                    return Some(Value::Unknown);
                }
                for property in &binding.properties {
                    value = value.property(property).unwrap_or(Value::Unknown);
                }
                Some(value)
            }
            Some(RegistrationCallee::Property { object, name }) => Some(
                self.value(object, depth + 1, ignore_writes)?
                    .property(&name)
                    .unwrap_or(Value::Unknown),
            ),
            None if node.kind() == "subscript_expression" => self
                .value(
                    node.child_by_field_name("object")?,
                    depth + 1,
                    ignore_writes,
                )
                .map(|_| Value::Unknown),
            None if node.kind() == "call_expression" => self
                .loader_available(node, ignore_writes)
                .then(|| self.requires.get(&node.id()).cloned())
                .flatten(),
            _ => None,
        }
    }

    fn callback_value(
        &self,
        callback: Node<'tree>,
        depth: usize,
        ignore_writes: bool,
    ) -> Option<Value> {
        if !matches!(callback.kind(), "arrow_function" | "function_expression")
            || callback.child_by_field_name("body").is_none()
            || callback
                .children(&mut callback.walk())
                .any(|child| child.kind() == "async")
        {
            return None;
        }
        let arguments = callback
            .parent()
            .filter(|node| node.kind() == "arguments")?;
        let mut cursor = arguments.walk();
        let mut positional = arguments
            .named_children(&mut cursor)
            .filter(|child| child.kind() != "comment");
        if positional.next()?.kind() == "spread_element" || positional.next()?.id() != callback.id()
        {
            return None;
        }
        let call = arguments.parent()?;
        match self.value(
            call.child_by_field_name("function")?,
            depth + 1,
            ignore_writes,
        )? {
            Value::Export(export) if export.kind == RegistrationExportKind::Feature => {
                Some(Value::Scenario)
            }
            Value::Scenario => Some(Value::Steps),
            _ => None,
        }
    }

    fn invalidate(&mut self, target: Node<'tree>, depth: usize) {
        if depth > 32 {
            return;
        }
        let mut stack = vec![target];
        while let Some(node) = stack.pop() {
            match node.kind() {
                "identifier"
                | "shorthand_property_identifier_pattern"
                | "shorthand_property_identifier" => {
                    let key = self.key(node, node_text(node, self.source));
                    if self.writes.insert(key.clone()) {
                        if let Some(Some(Binding {
                            origin: Origin::Initializer(value),
                            ..
                        })) = self.bindings.get(&key)
                        {
                            self.invalidate(*value, depth + 1);
                        }
                    }
                }
                "member_expression" | "subscript_expression" => {
                    stack.extend(node.child_by_field_name("object"))
                }
                "pair_pattern" => stack.extend(node.child_by_field_name("value")),
                "call_expression" => {}
                _ => push_named_children_reverse(node, &mut stack),
            }
        }
    }
}

pub(super) fn discover<'tree>(
    root: Node<'tree>,
    source: &[u8],
    file: &Path,
    resolver: &mut RegistrationResolver,
    registrations: &super::registrations::RegistrationNames,
) -> Result<FrameworkCalls> {
    let (scopes, _, parents) = collect_binding_scopes(root, source);
    let mut bindings = Bindings {
        root: root.id(),
        source,
        scopes,
        parents,
        enclosing: BTreeMap::new(),
        bindings: BTreeMap::new(),
        writes: BTreeSet::new(),
        requires: BTreeMap::new(),
        registrations,
        forwarded_targets: BTreeSet::new(),
    };
    let mut nodes = Vec::new();
    let mut pending = vec![(root, root.id())];
    while let Some((node, scope)) = pending.pop() {
        let scope = if is_local_scope(node) {
            node.id()
        } else {
            scope
        };
        bindings.enclosing.insert(node.id(), scope);
        nodes.push(node);
        super::ast::push_named_children_reverse_scoped(node, scope, &mut pending);
    }
    for node in nodes.iter().copied() {
        match node.kind() {
            "import_statement" if import_has_runtime_bindings(node) => {
                let requires_loader = node
                    .named_children(&mut node.walk())
                    .any(|child| child.kind() == "import_require_clause");
                let Some(module) = import_statement_module(node, source) else {
                    continue;
                };
                let exports = if is_supported_module(module) {
                    registration_exports_for_module(module)
                } else {
                    resolver
                        .registration_exports(file, module)?
                        .resolution
                        .map(|value| value.exports)
                        .unwrap_or_default()
                };
                for_each_import_binding(node, source, |binding| {
                    let (name, value) = match binding {
                        ImportBinding::Named { exported, local } => (
                            local,
                            exports
                                .get(exported)
                                .cloned()
                                .filter(|export| is_new_framework(export.framework))
                                .map(Value::Export),
                        ),
                        ImportBinding::Namespace(name) => (
                            name,
                            exports
                                .values()
                                .any(|export| is_new_framework(export.framework))
                                .then(|| Value::Namespace(exports.clone())),
                        ),
                        ImportBinding::Default(name) => (name, None),
                    };
                    let value = value.filter(|_| {
                        !requires_loader
                            || exports
                                .values()
                                .any(|export| export.framework == Framework::JestCucumber)
                    });
                    bindings
                        .bindings
                        .entry((root.id(), name.to_owned()))
                        .and_modify(|binding| *binding = None)
                        .or_insert_with(|| {
                            value.map(|value| Binding {
                                origin: if requires_loader {
                                    Origin::Required(node, value)
                                } else {
                                    Origin::Imported(value)
                                },
                                properties: Vec::new(),
                                certain: true,
                            })
                        });
                });
            }
            "call_expression" => {
                if node
                    .child_by_field_name("function")
                    .is_some_and(|node| node_text(node, source) == "require")
                {
                    if let Some(module) = call_string_argument(node, source) {
                        let exports = if is_supported_module(module) {
                            registration_exports_for_module(module)
                        } else {
                            resolver
                                .registration_exports(file, module)?
                                .resolution
                                .map(|value| value.exports)
                                .unwrap_or_default()
                        };
                        // The Vitest plugin is ESM-only; a require is not a supported runtime entry.
                        if exports
                            .values()
                            .any(|export| export.framework == Framework::JestCucumber)
                        {
                            bindings
                                .requires
                                .insert(node.id(), Value::Namespace(exports));
                        }
                    }
                }
            }
            "variable_declarator" if !is_erased_declaration(node) => {
                if let Some(pattern) = node.child_by_field_name("name") {
                    let mut names = BTreeSet::new();
                    collect_binding_names(pattern, source, &mut names);
                    if let Some(value) = node.child_by_field_name("value") {
                        bindings.bind(
                            pattern,
                            Origin::Initializer(value),
                            node.parent().is_some_and(is_const_declaration),
                        );
                        for name in names {
                            bindings
                                .bindings
                                .entry(bindings.key(pattern, &name))
                                .or_insert(None);
                        }
                        continue;
                    }
                    for name in names {
                        bindings.bindings.insert(bindings.key(pattern, &name), None);
                    }
                }
            }
            "arrow_function"
            | "function_expression"
            | "generator_function"
            | "function_declaration"
            | "generator_function_declaration"
            | "method_definition" => {
                if let Some(parameter) = node.child_by_field_name("parameter").or_else(|| {
                    node.child_by_field_name("parameters")
                        .and_then(|parameters| parameters.named_child(0))
                }) {
                    bindings.bind(parameter, Origin::Parameter(node), true);
                }
                if matches!(
                    node.kind(),
                    "function_declaration" | "generator_function_declaration"
                ) {
                    bindings.shadow_declaration(node, root);
                }
            }
            "class_declaration" | "abstract_class_declaration" | "enum_declaration"
                if !is_erased_declaration(node) =>
            {
                bindings.shadow_declaration(node, root);
            }
            _ => {}
        }
    }
    for node in nodes.iter().copied() {
        let target = match node.kind() {
            "assignment_expression" | "augmented_assignment_expression" => {
                node.child_by_field_name("left")
            }
            "update_expression" => node.child_by_field_name("argument"),
            "for_in_statement"
                if super::assertions::loop_binding_keyword(
                    node,
                    node.child_by_field_name("left").unwrap_or(node),
                )
                .is_none() =>
            {
                node.child_by_field_name("left")
            }
            _ => None,
        };
        if let Some(target) = target {
            bindings.invalidate(target, 0);
        }
    }
    // Unknown code can mutate a framework object handed to it, including through a container.
    // Collect before invalidating so traversal order cannot change which aliases lose trust.
    let mut escaped = Vec::new();
    for call in nodes
        .iter()
        .copied()
        .filter(|node| node.kind() == "call_expression")
    {
        let known = call
            .child_by_field_name("function")
            .and_then(|function| bindings.value(function, 0, false));
        if matches!(
            known,
            Some(Value::Export(_) | Value::Fallback(_) | Value::Scenario)
        ) {
            continue;
        }
        let mut arguments: Vec<_> = call.child_by_field_name("arguments").into_iter().collect();
        while let Some(argument) = arguments.pop() {
            if bindings.value(argument, 0, false).is_some() {
                escaped.push(argument);
            } else if argument.kind() != "call_expression" {
                super::push_value_positions(argument, source, &mut arguments);
            }
        }
    }
    for argument in escaped {
        bindings.invalidate(argument, 0);
    }
    let mut result = FrameworkCalls::default();
    for call in nodes
        .into_iter()
        .filter(|node| node.kind() == "call_expression")
    {
        let Some(function) = call.child_by_field_name("function") else {
            continue;
        };
        let mut base = function;
        while let Some(RegistrationCallee::Property { object, .. }) =
            registration_callee(base, source)
        {
            base = object;
        }
        if let Some(RegistrationCallee::Identifier(name)) = registration_callee(base, source) {
            if bindings.key(base, name).0 != root.id() {
                result.shadowed.insert(function.start_byte());
            }
        }
        let value = bindings.value(function, 0, false);
        let fallback = matches!(&value, Some(Value::Fallback(_)));
        match value {
            Some(Value::Export(export) | Value::Fallback(export))
                if export.kind == RegistrationExportKind::Call =>
            {
                result.handled.insert(function.start_byte());
                if bindings.forwarded_targets.contains(&function.start_byte()) {
                    continue;
                }
                let executes = fallback
                    || export.framework != Framework::JestCucumber
                    || nearest_function_scope(call.parent()).is_some_and(|callback| {
                        matches!(
                            bindings.callback_value(callback, 0, false),
                            Some(Value::Steps)
                        )
                    });
                if executes {
                    result.registrations.insert(
                        function.start_byte(),
                        (
                            node_text(function, source).to_owned(),
                            export.canonical,
                            export.framework,
                        ),
                    );
                } else {
                    result.incomplete.insert(function.start_byte());
                }
            }
            Some(Value::Export(_)) | Some(Value::Scenario) => {
                result.handled.insert(function.start_byte());
                let callback = call.child_by_field_name("arguments").and_then(|arguments| {
                    arguments
                        .named_children(&mut arguments.walk())
                        .filter(|child| child.kind() != "comment")
                        .nth(1)
                });
                if callback
                    .and_then(|callback| bindings.callback_value(callback, 0, false))
                    .is_none()
                {
                    result.incomplete.insert(function.start_byte());
                }
            }
            _ => {
                // Mutated roots, dynamic properties and escaped framework values are incomplete.
                let mut references = vec![function];
                references.extend(
                    call.child_by_field_name("arguments")
                        .into_iter()
                        .flat_map(|args| args.named_children(&mut args.walk()).collect::<Vec<_>>()),
                );
                if references.into_iter().any(|node| {
                    let base = node.child_by_field_name("object").unwrap_or(node);
                    bindings.value(base, 0, true).is_some()
                }) {
                    result.handled.insert(function.start_byte());
                    result.incomplete.insert(function.start_byte());
                }
            }
        }
    }
    Ok(result)
}
