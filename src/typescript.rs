//! Tree-sitter based JavaScript and TypeScript step-definition extraction.

mod assertions;
mod ast;
pub(crate) mod frameworks;
mod handler;
mod matcher;
mod module_resolver;
mod project_resolution;
mod registrations;
mod suppression;

use self::assertions::{collect_binding_names, loop_binding_keyword, AssertionBindings};
use self::handler::{
    bind_arguments, collect_handler_bindings, fingerprint_handler, fingerprint_method_handler,
    resolve_handler, HandlerBinding,
};
#[cfg(test)]
use self::handler::{bounded_source_snippet, MAX_HANDLER_SNIPPET_CHARS};
pub use self::matcher::normalize_matcher;
#[cfg(test)]
use self::matcher::normalize_regular_expression;
pub(crate) use self::matcher::rust_regex_expression;
pub(crate) use self::matcher::semantic_regex_flags;
use self::matcher::{matcher_value, normalize_matcher_with_flags};
use self::matcher::{rust_regex_support, RegexSupport};
use self::module_resolver::RegistrationResolver;
use self::registrations::{
    decorator_registration_name, detect_framework, detect_registrations, registration_callee,
    registration_name, unresolved_registration_module, RegistrationCallee, RegistrationNames,
};
use self::suppression::inline_suppressions;
use crate::model::{Framework, HandlerFingerprint, MatcherKind, SourceLocation, StepDefinition};
use crate::source_adapter::{
    adapter_for_language, grammar_for_language, AdapterSessionState, SourceAdapter,
    SourceExtractionSession, StatefulSourceAdapter, UNPARSEABLE_SOURCE_DIAGNOSTIC_PREFIX,
    UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX,
};
pub use crate::source_adapter::{
    Extraction, ExtractionDiagnostic, ExtractionDiagnosticLevel, SourceFile, SourceLanguage,
};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::{Node, Parser};

pub(crate) struct TreeSitterSourceAdapter {
    name: &'static str,
    language: SourceLanguage,
}

pub(crate) static JAVASCRIPT_ADAPTER: TreeSitterSourceAdapter = TreeSitterSourceAdapter {
    name: "javascript",
    language: SourceLanguage::JavaScript,
};
pub(crate) static TYPESCRIPT_ADAPTER: TreeSitterSourceAdapter = TreeSitterSourceAdapter {
    name: "typescript",
    language: SourceLanguage::TypeScript,
};
pub(crate) static TSX_ADAPTER: TreeSitterSourceAdapter = TreeSitterSourceAdapter {
    name: "tsx",
    language: SourceLanguage::Tsx,
};

#[derive(Default)]
pub(crate) struct TypeScriptExtractionSession {
    resolver: RegistrationResolver,
    configured_registrations: std::collections::BTreeSet<String>,
    configured_assertion_modules: std::collections::BTreeSet<String>,
}

impl TypeScriptExtractionSession {
    pub(crate) fn for_root(
        root: &std::path::Path,
        registrations: &[String],
        assertion_modules: &[String],
    ) -> Self {
        Self {
            resolver: RegistrationResolver::for_root(root),
            configured_registrations: registrations.iter().cloned().collect(),
            configured_assertion_modules: assertion_modules.iter().cloned().collect(),
        }
    }
}

impl AdapterSessionState for TypeScriptExtractionSession {
    fn initialize(
        root: Option<&std::path::Path>,
        registrations: &[String],
        assertion_modules: &[String],
    ) -> Self {
        match root {
            Some(root) => Self::for_root(root, registrations, assertion_modules),
            None => Self::default(),
        }
    }
}

impl StatefulSourceAdapter for TreeSitterSourceAdapter {
    type State = TypeScriptExtractionSession;
}

impl SourceAdapter for TreeSitterSourceAdapter {
    fn name(&self) -> &'static str {
        self.name
    }

    fn language(&self) -> SourceLanguage {
        self.language
    }

    fn extract(&self, source: &str, file: &SourceFile) -> Result<Extraction> {
        let mut session = SourceExtractionSession::default();
        self.extract_with_session(source, file, &mut session)
    }

    fn extract_with_session(
        &self,
        source: &str,
        file: &SourceFile,
        session: &mut SourceExtractionSession,
    ) -> Result<Extraction> {
        if file.language != self.language {
            anyhow::bail!(
                "{} adapter cannot parse {:?} source {}",
                self.name,
                file.language,
                file.path.display()
            );
        }
        extract_detailed_impl(
            source,
            file,
            session.state::<TypeScriptExtractionSession>()?,
        )
    }
}

struct AdapterContext<'source, 'tree> {
    source: &'source [u8],
    source_lines: &'source [&'source str],
    file: &'source SourceFile,
    framework: Framework,
    registrations: &'source RegistrationNames,
    assertions: &'source AssertionBindings,
    handler_bindings: &'source BTreeMap<String, Vec<HandlerBinding<'tree>>>,
}

#[derive(Default)]
struct UnresolvedRegistrationCalls {
    count: usize,
    first_location: Option<SourceLocation>,
    modules: BTreeMap<String, (usize, SourceLocation)>,
    /// Calls that hand a registration to a function the analyzer does not model.
    passed: usize,
    first_passed: Option<SourceLocation>,
}

impl UnresolvedRegistrationCalls {
    fn record(&mut self, location: SourceLocation) {
        self.count = self.count.saturating_add(1);
        self.first_location.get_or_insert(location);
    }

    fn record_passed(&mut self, location: SourceLocation) {
        self.passed = self.passed.saturating_add(1);
        self.first_passed.get_or_insert(location);
    }

    fn record_module(&mut self, module: &str, location: SourceLocation) {
        let entry = self
            .modules
            .entry(module.to_owned())
            .or_insert_with(|| (0, location));
        entry.0 = entry.0.saturating_add(1);
    }
}

/// Reads and extracts step definitions from a discovered JavaScript or TypeScript file.
pub fn extract_file(file: &SourceFile) -> Result<Vec<StepDefinition>> {
    Ok(extract_file_detailed(file)?.definitions)
}

/// Reads a source file and returns definitions plus localized extraction diagnostics.
pub fn extract_file_detailed(file: &SourceFile) -> Result<Extraction> {
    adapter_for_language(file.language).extract_file(file)
}

/// Extracts step definitions from in-memory source using `file` for syntax and locations.
pub fn extract(source: &str, file: &SourceFile) -> Result<Vec<StepDefinition>> {
    Ok(extract_detailed(source, file)?.definitions)
}

/// Extracts definitions and localized diagnostics from in-memory source.
pub fn extract_detailed(source: &str, file: &SourceFile) -> Result<Extraction> {
    adapter_for_language(file.language).extract(source, file)
}

fn extract_detailed_impl(
    source: &str,
    file: &SourceFile,
    session: &mut TypeScriptExtractionSession,
) -> Result<Extraction> {
    let mut parser = Parser::new();
    parser
        .set_language(&grammar_for_language(file.language))
        .context("failed to initialize the JavaScript/TypeScript parser")?;
    let tree = parser
        .parse(source, None)
        .context("Tree-sitter did not return a syntax tree")?;

    let root = tree.root_node();
    let source_bytes = source.as_bytes();
    let source_lines: Vec<_> = source.lines().collect();
    let framework = detect_framework(root, source_bytes);
    let registrations = detect_registrations(
        root,
        source_bytes,
        framework,
        &file.path,
        &mut session.resolver,
        &session.configured_registrations,
    )?;
    let framework = registrations.framework;
    let assertions = AssertionBindings::discover(
        root,
        source_bytes,
        &registrations,
        &session.configured_assertion_modules,
    );
    let mut handler_bindings = BTreeMap::new();
    collect_handler_bindings(root, source_bytes, &mut handler_bindings);
    let context = AdapterContext {
        source: source_bytes,
        source_lines: &source_lines,
        file,
        framework,
        registrations: &registrations,
        assertions: &assertions,
        handler_bindings: &handler_bindings,
    };
    let mut definitions = Vec::new();
    let mut diagnostics = Vec::new();
    let mut unresolved_registration_calls = UnresolvedRegistrationCalls::default();
    if root.has_error() {
        let syntax_node = first_syntax_error(root).unwrap_or(root);
        // A parse failure still leaves error-recovered definitions worth analyzing, so it does not
        // discard the file. It is a completeness signal (via its message prefix), which marks the
        // corpus incomplete; the CLI keeps the run non-fatal unless `--fail-on-unparseable` or
        // `--fail-on-incomplete` is set, so the level here stays a warning.
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location: node_location(file, syntax_node, source_bytes),
            message: format!("{UNPARSEABLE_SOURCE_DIAGNOSTIC_PREFIX}, so analysis is incomplete; fix the syntax, use a .tsx extension for JSX, or narrow definition discovery"),
        });
    }
    collect_calls(
        root,
        &context,
        &mut definitions,
        &mut diagnostics,
        &mut unresolved_registration_calls,
    );
    if let Some(location) = unresolved_registration_calls.first_location {
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location,
            message: format!(
                "{UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX} {} call(s) look like step registrations but could not be resolved to a supported registration import or global; definitions may be missing",
                unresolved_registration_calls.count
            ),
        });
    }
    if let Some(location) = unresolved_registration_calls.first_passed {
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location,
            message: format!(
                "{UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX} {} call(s) pass a step registration to a function the analyzer does not model; definitions may be missing",
                unresolved_registration_calls.passed
            ),
        });
    }
    let mut explained_modules = BTreeSet::new();
    for (module, (count, location)) in unresolved_registration_calls.modules {
        let cause = registrations::unresolved_module_reason(&registrations, &module)
            .map(|reason| format!(" ({reason})"))
            .unwrap_or_default();
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location,
            message: format!(
                "{UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX} {count} call(s) import a known registration through unresolved module `{module}`{cause}; definitions may be missing"
            ),
        });
        explained_modules.insert(module);
    }
    // A specifier can fail resolution without any call being attributed to it — a barrel that is
    // imported but whose registrations are re-exported onward, or a file whose registration calls
    // were themselves unresolvable. Report the cause anyway so a resolution limit is never
    // reduced to a bare "produced 0 definitions".
    for (module, recorded) in registrations::unresolved_module_reasons(&registrations) {
        if explained_modules.contains(module) {
            continue;
        }
        let line = recorded.row + 1;
        let column = unicode_column(source_bytes, recorded.byte_offset, recorded.byte_column);
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location: SourceLocation::new(&file.path, line, column, line, column),
            message: format!(
                "{UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX} module `{module}` could not be resolved statically ({}); definitions may be missing",
                recorded.reason
            ),
        });
    }
    definitions.sort_by(|left, right| {
        left.location
            .line
            .cmp(&right.location.line)
            .then(left.location.column.cmp(&right.location.column))
    });
    Ok(Extraction {
        definitions,
        diagnostics,
    })
}

fn collect_calls<'tree>(
    node: Node<'tree>,
    context: &AdapterContext<'_, 'tree>,
    definitions: &mut Vec<StepDefinition>,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
    unresolved_registration_calls: &mut UnresolvedRegistrationCalls,
) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if node.kind() == "call_expression" {
            if let Some(unresolved) = unresolved_registration_call(node, context) {
                let location = node_location(context.file, node, context.source);
                match unresolved {
                    UnresolvedRegistration::Generic => {
                        unresolved_registration_calls.record(location)
                    }
                    UnresolvedRegistration::Module(module) => {
                        unresolved_registration_calls.record_module(module, location)
                    }
                    UnresolvedRegistration::Passed => {
                        unresolved_registration_calls.record_passed(location)
                    }
                }
            }
            let definition = if node
                .parent()
                .is_some_and(|parent| parent.kind() == "decorator")
            {
                extract_decorator(node, context, diagnostics)
            } else {
                extract_call(node, context, diagnostics)
            };
            if let Some(definition) = definition {
                definitions.push(definition);
            }
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
}

enum UnresolvedRegistration<'a> {
    Generic,
    Module(&'a str),
    /// `helper(Given, 'a step', fn)`: a registration handed to another function, which may call it.
    Passed,
}

/// Whether a registration appears anywhere in a call's arguments as a value rather than being
/// called: directly (`helper(Given)`), inside an object or array (`helper({ register: Given })`,
/// `helper([Given])`), returned from a function (`helper(() => Given)`), or handed to a constructor.
/// A nested call is not entered: it checks its own arguments, and a registration it calls
/// (`helper(() => Given('a', fn))`) is extracted as a definition. Since calls are never entered, a
/// registration reached here is always a value, never a callee, and each node is walked once.
fn arguments_pass_registration(arguments: Node<'_>, context: &AdapterContext<'_, '_>) -> bool {
    let mut stack = vec![arguments];
    while let Some(node) = stack.pop() {
        let passed = match node.kind() {
            // `Given.bind(null)` is still the registration, bound; any other call yields something
            // new and checks its own arguments.
            "call_expression" => {
                let bound = node
                    .child_by_field_name("function")
                    .filter(|function| function.kind() == "member_expression")
                    .filter(|function| {
                        function
                            .child_by_field_name("property")
                            .is_some_and(|property| node_text(property, context.source) == "bind")
                    })
                    .and_then(|function| function.child_by_field_name("object"));
                stack.extend(bound);
                continue;
            }
            "identifier" | "member_expression" | "subscript_expression" => {
                (registration_name(node, context.source, context.registrations).is_some()
                    || decorator_registration_name(node, context.source, context.registrations)
                        .is_some())
                    && !base_is_locally_bound(node, context.source)
            }
            // `{ Given }` passes the binding under its own name.
            "shorthand_property_identifier" => {
                context
                    .registrations
                    .recognizes_alias(node_text(node, context.source))
                    && !base_is_locally_bound(node, context.source)
            }
            _ => false,
        };
        if passed {
            return true;
        }
        push_value_positions(node, context.source, &mut stack);
    }
    false
}

/// Pushes the children of `node` whose value can be `node`'s own value. Only these can carry a
/// registration through unchanged: `[Given]`, `{ step: Given }`, `flag ? Given : When`,
/// `Given || fallback`, `() => Given`. Everything else consumes the registration and yields
/// something new — `Given.name`, `typeof Given`, `Given !== undefined`, `new Given()` — so the walk
/// stops there rather than report a value that is not the registration.
fn push_value_positions<'tree>(node: Node<'tree>, source: &[u8], stack: &mut Vec<Node<'tree>>) {
    let field = |name: &str| node.child_by_field_name(name);
    match node.kind() {
        "arguments"
        | "array"
        | "object"
        | "parenthesized_expression"
        | "as_expression"
        | "satisfies_expression"
        | "non_null_expression"
        | "await_expression" => {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
        "pair" | "assignment_expression" => stack.extend(field("value").or(field("right"))),
        // Spreading a literal carries its elements (`...[Given]`); spreading anything else copies
        // its own properties into a new container (`...Given`), which is not the registration.
        "spread_element" => stack.extend(
            node.named_child(0)
                .filter(|operand| matches!(operand.kind(), "array" | "object")),
        ),
        // `(a, b)` evaluates to its last operand only.
        "sequence_expression" => {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor).last());
        }
        "ternary_expression" => stack.extend(
            [field("consequence"), field("alternative")]
                .into_iter()
                .flatten(),
        ),
        "binary_expression" => {
            let preserves = field("operator")
                .is_some_and(|operator| matches!(node_text(operator, source), "||" | "&&" | "??"));
            if preserves {
                stack.extend([field("left"), field("right")].into_iter().flatten());
            }
        }
        // A function or object method passes whatever it returns or yields; its parameters are not
        // values.
        "arrow_function" | "function_expression" | "generator_function" | "method_definition" => {
            match field("body") {
                Some(body) if body.kind() == "statement_block" => push_returned_values(body, stack),
                body => stack.extend(body),
            }
        }
        // A constructor stores its arguments; the class it constructs is consumed.
        "new_expression" => stack.extend(field("arguments")),
        _ => {}
    }
}

/// Pushes the value of every `return` and `yield` in a function body, through any control flow —
/// `if`, `try`, `switch`, loops — but not into a nested function, whose returns are its own.
fn push_returned_values<'tree>(body: Node<'tree>, stack: &mut Vec<Node<'tree>>) {
    let mut pending = vec![body];
    while let Some(node) = pending.pop() {
        match node.kind() {
            "arrow_function"
            | "function_expression"
            | "function_declaration"
            | "generator_function"
            | "generator_function_declaration"
            | "method_definition"
            | "class"
            | "class_declaration" => continue,
            "return_statement" | "yield_expression" => {
                let mut cursor = node.walk();
                stack.extend(node.named_children(&mut cursor));
            }
            _ => {}
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
}

/// Whether the name an argument starts from (`Given`, or `cucumber` in `cucumber.Given`) is bound
/// inside an enclosing function: a parameter, or a declaration directly in an enclosing block. Such
/// a name is a local value, such as a fixture named `Given`, not the file's registration binding.
fn base_is_locally_bound(argument: Node<'_>, source: &[u8]) -> bool {
    let mut base = argument;
    while let Some(inner) = match base.kind() {
        "member_expression" => base.child_by_field_name("object"),
        "parenthesized_expression" => base.named_child(0),
        _ => None,
    } {
        base = inner;
    }
    if !matches!(base.kind(), "identifier" | "shorthand_property_identifier") {
        return false;
    }
    let name = node_text(base, source);
    let mut bound = BTreeSet::new();
    let mut current = base;
    while let Some(scope) = current.parent() {
        match scope.kind() {
            "arrow_function"
            | "function_expression"
            | "function_declaration"
            | "generator_function"
            | "generator_function_declaration"
            | "method_definition" => {
                for field in ["parameters", "parameter"] {
                    if let Some(parameters) = scope.child_by_field_name(field) {
                        collect_binding_names(parameters, source, &mut bound);
                    }
                }
            }
            // `for (const Given of values)`: a loop declaration binds for the loop body. A bare
            // `for (Given of values)` assigns an outer name and binds nothing.
            "for_in_statement" => {
                if let Some(left) = scope
                    .child_by_field_name("left")
                    .filter(|left| loop_binding_keyword(scope, *left).is_some())
                {
                    collect_binding_names(left, source, &mut bound);
                }
            }
            "for_statement" => {
                if let Some(initializer) = scope.child_by_field_name("initializer").filter(|node| {
                    matches!(node.kind(), "lexical_declaration" | "variable_declaration")
                }) {
                    let mut declarators = initializer.walk();
                    for declarator in initializer.named_children(&mut declarators) {
                        if let Some(pattern) = declarator.child_by_field_name("name") {
                            collect_binding_names(pattern, source, &mut bound);
                        }
                    }
                }
            }
            "catch_clause" => {
                if let Some(parameter) = scope.child_by_field_name("parameter") {
                    collect_binding_names(parameter, source, &mut bound);
                }
            }
            "statement_block" => {
                let mut cursor = scope.walk();
                for statement in scope.named_children(&mut cursor) {
                    match statement.kind() {
                        "lexical_declaration" | "variable_declaration" => {
                            let mut declarators = statement.walk();
                            for declarator in statement.named_children(&mut declarators) {
                                if let Some(pattern) = declarator.child_by_field_name("name") {
                                    collect_binding_names(pattern, source, &mut bound);
                                }
                            }
                        }
                        "function_declaration"
                        | "generator_function_declaration"
                        | "class_declaration"
                        | "abstract_class_declaration"
                        | "enum_declaration" => {
                            // A declaration always carries its name.
                            bound.extend(
                                statement
                                    .child_by_field_name("name")
                                    .map(|name| node_text(name, source).to_owned()),
                            );
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if bound.contains(name) {
            return true;
        }
        current = scope;
    }
    false
}

fn unresolved_registration_call<'a>(
    call: Node<'_>,
    context: &'a AdapterContext<'_, '_>,
) -> Option<UnresolvedRegistration<'a>> {
    let function = call.child_by_field_name("function")?;
    if registration_name(function, context.source, context.registrations).is_some()
        || decorator_registration_name(function, context.source, context.registrations).is_some()
    {
        return None;
    }
    // A registration passed as an argument can be called by the receiving function, whose body the
    // analyzer does not follow. The definition it registers would be missed, so it is reported.
    let passes_registration = call
        .child_by_field_name("arguments")
        .is_some_and(|arguments| arguments_pass_registration(arguments, context));
    if passes_registration {
        return Some(UnresolvedRegistration::Passed);
    }
    if let Some(module) =
        unresolved_registration_module(function, context.source, context.registrations)
    {
        return Some(UnresolvedRegistration::Module(module));
    }
    let looks_like_registration = match registration_callee(function, context.source) {
        Some(RegistrationCallee::Identifier(name)) => matches!(
            name,
            "Given"
                | "When"
                | "Then"
                | "And"
                | "But"
                | "Step"
                | "defineStep"
                | "given"
                | "when"
                | "then"
        ),
        // Lowercase `.then()` is ordinary Promise usage, not registration evidence. Member
        // expressions intentionally use only the distinctive registration-style properties.
        Some(RegistrationCallee::Property { name, .. }) => matches!(
            name.as_ref(),
            "Given" | "When" | "Then" | "And" | "But" | "Step" | "defineStep"
        ),
        _ => false,
    };
    looks_like_registration.then_some(UnresolvedRegistration::Generic)
}

fn extract_call<'tree>(
    call: Node<'tree>,
    context: &AdapterContext<'_, 'tree>,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
) -> Option<StepDefinition> {
    let source = context.source;
    let function = call.child_by_field_name("function")?;
    let (callee, registration, registration_framework) =
        registration_name(function, source, context.registrations)?;
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let arguments: Vec<_> = arguments.named_children(&mut cursor).collect();
    if arguments.len() < 2 {
        return None;
    }
    let matcher_node = arguments[0];
    let (matcher, matcher_kind, matcher_flags) =
        extract_matcher(matcher_node, context, diagnostics)?;
    let handler = arguments.iter().rev().copied().find(|node| {
        matches!(
            node.kind(),
            "arrow_function"
                | "function_expression"
                | "generator_function"
                | "identifier"
                | "member_expression"
                | "call_expression"
        )
    });
    let Some(handler) = handler else {
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location: node_location(context.file, call, source),
            message: "dynamic or unsupported step handler cannot be compared statically".to_owned(),
        });
        return None;
    };
    let bound_arguments = bind_arguments(handler, source);
    let resolved_handler = resolve_handler(handler, source, context.handler_bindings);
    if resolved_handler.is_none() && handler.kind() == "call_expression" {
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location: node_location(context.file, handler, source),
            message: "dynamic or unsupported step handler cannot be compared statically".to_owned(),
        });
    }
    let comparable = resolved_handler.is_some();
    let handler = resolved_handler.unwrap_or(handler);
    Some(step_definition(
        call,
        context,
        diagnostics,
        (callee, registration, registration_framework),
        (matcher, matcher_kind, matcher_flags),
        fingerprint_handler(
            handler,
            bound_arguments,
            source,
            comparable,
            context.assertions,
        ),
    ))
}

fn extract_decorator<'tree>(
    call: Node<'tree>,
    context: &AdapterContext<'_, 'tree>,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
) -> Option<StepDefinition> {
    let source = context.source;
    let function = call.child_by_field_name("function")?;
    let (callee, registration, registration_framework) =
        decorator_registration_name(function, source, context.registrations)?;
    // `arguments` is a required field of a tree-sitter call_expression.
    let arguments = call.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    let Some(matcher_node) = arguments.named_children(&mut cursor).next() else {
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location: node_location(context.file, call, source),
            message: "dynamic or unsupported step matcher cannot be analyzed statically".to_owned(),
        });
        return None;
    };
    let (matcher, matcher_kind, matcher_flags) =
        extract_matcher(matcher_node, context, diagnostics)?;
    let Some(method) = decorated_method(call) else {
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location: node_location(context.file, call, source),
            message: "dynamic or unsupported step handler cannot be compared statically".to_owned(),
        });
        return None;
    };
    let Some(handler) = fingerprint_method_handler(method, source, context.assertions) else {
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location: node_location(context.file, method, source),
            message: "dynamic or unsupported step handler cannot be compared statically".to_owned(),
        });
        return None;
    };
    Some(step_definition(
        call,
        context,
        diagnostics,
        (callee, registration, registration_framework),
        (matcher, matcher_kind, matcher_flags),
        handler,
    ))
}

/// Assembles the definition both registration shapes describe once their differing parts are
/// resolved. A plain call fingerprints its handler argument and a decorator fingerprints the method
/// it precedes, but the matcher, the resolved registration, and everything the `call` node itself
/// carries — the location and the inline suppressions — are built the same way for both.
fn step_definition<'tree>(
    call: Node<'tree>,
    context: &AdapterContext<'_, 'tree>,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
    (callee, registration, registration_framework): (String, String, Framework),
    (matcher, matcher_kind, matcher_flags): (String, MatcherKind, String),
    handler: HandlerFingerprint,
) -> StepDefinition {
    StepDefinition {
        normalized_matcher: normalize_matcher_with_flags(&matcher, matcher_kind, &matcher_flags),
        matcher,
        matcher_kind,
        matcher_flags,
        handler,
        framework: effective_registration_framework(registration_framework, context.framework),
        registration: if registration.is_empty() {
            callee
        } else {
            registration
        },
        location: node_location(context.file, call, context.source),
        inline_suppressions: inline_suppressions(
            call,
            context.source_lines,
            context.file,
            diagnostics,
        ),
    }
}

/// Resolves a decorator to the class method it decorates without crossing another class member.
/// Tree-sitter represents decorators as siblings immediately before their member, so skipping
/// only consecutive decorator siblings keeps the association structural and unambiguous.
fn decorated_method(call: Node<'_>) -> Option<Node<'_>> {
    let decorator = call
        .parent()
        .filter(|parent| parent.kind() == "decorator")?;
    if let Some(method) = decorator
        .parent()
        .filter(|parent| parent.kind() == "method_definition")
    {
        return Some(method);
    }
    let mut sibling = decorator.next_named_sibling()?;
    while sibling.kind() == "decorator" {
        sibling = sibling.next_named_sibling()?;
    }
    (sibling.kind() == "method_definition").then_some(sibling)
}

fn effective_registration_framework(binding: Framework, file: Framework) -> Framework {
    if binding == Framework::Unknown {
        file
    } else {
        binding
    }
}

fn extract_matcher(
    matcher_node: Node<'_>,
    context: &AdapterContext<'_, '_>,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
) -> Option<(String, MatcherKind, String)> {
    let source = context.source;
    let (matcher, matcher_kind, matcher_flags) = match matcher_value(matcher_node, source) {
        Some(value) => value,
        None if matcher_node.kind() == "string" => {
            diagnostics.push(ExtractionDiagnostic {
                level: ExtractionDiagnosticLevel::Error,
                location: node_location(context.file, matcher_node, source),
                message: "invalid JavaScript escape sequence in step matcher".to_owned(),
            });
            return None;
        }
        None => {
            diagnostics.push(ExtractionDiagnostic {
                level: ExtractionDiagnosticLevel::Warning,
                location: node_location(context.file, matcher_node, source),
                message: "dynamic or unsupported step matcher cannot be analyzed statically"
                    .to_owned(),
            });
            return None;
        }
    };
    if matcher_kind == MatcherKind::RegularExpression {
        match rust_regex_support(&matcher, &matcher_flags) {
            RegexSupport::Unsupported => diagnostics.push(ExtractionDiagnostic {
                level: ExtractionDiagnosticLevel::Warning,
                location: node_location(context.file, matcher_node, source),
                message: "regular expression uses syntax unsupported by static usage analysis; unused and ambiguity checks will treat this definition as indeterminate".to_owned(),
            }),
            // The analysis phase emits one run-level operational error after reports are written.
            RegexSupport::ResourceLimit | RegexSupport::Supported => {}
        }
    }
    Some((matcher, matcher_kind, matcher_flags))
}

fn node_location(file: &SourceFile, node: Node<'_>, source: &[u8]) -> SourceLocation {
    let start = node.start_position();
    let end = node.end_position();
    SourceLocation::new(
        &file.path,
        start.row + 1,
        unicode_column(source, node.start_byte(), start.column),
        end.row + 1,
        unicode_column(source, node.end_byte(), end.column),
    )
}

fn unicode_column(source: &[u8], byte_offset: usize, byte_column: usize) -> usize {
    let line_start = byte_offset.saturating_sub(byte_column);
    let end = byte_offset.min(source.len());
    std::str::from_utf8(&source[line_start.min(end)..end])
        .map_or(byte_column, |prefix| prefix.chars().count())
        + 1
}

fn first_syntax_error(root: Node<'_>) -> Option<Node<'_>> {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            return Some(node);
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

fn node_text<'a>(node: Node<'_>, source: &'a [u8]) -> &'a str {
    std::str::from_utf8(&source[node.byte_range()]).unwrap_or("")
}

#[cfg(test)]
mod tests;
