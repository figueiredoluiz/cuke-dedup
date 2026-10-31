//! Tree-sitter based JavaScript and TypeScript step-definition extraction.

pub(crate) mod semantics;

mod assertions;
mod ast;
mod framework_callbacks;
pub(crate) mod framework_config;
pub(crate) mod frameworks;
mod handler;
mod matcher;
mod module_resolver;
mod project_resolution;
mod registrations;
mod source_filter;

use self::assertions::{AssertionBindings, FileScopes};
#[cfg(test)]
use self::handler::MAX_HANDLER_SNIPPET_CHARS;
use self::handler::{
    bind_arguments, collect_handler_bindings, fingerprint_handler, fingerprint_method_handler,
    resolve_handler, HandlerBinding, ModuleScope,
};
use self::matcher::matcher_value;
use self::matcher::{rust_regex_support, RegexSupport};
use self::module_resolver::RegistrationResolver;
use self::registrations::{
    decorator_registration_name, detect_framework, detect_registrations, registration_callee,
    registration_name, unresolved_registration_module, RegistrationCallee, RegistrationNames,
};
#[cfg(test)]
use crate::model::bounded_source_snippet;
use crate::model::{Framework, HandlerFingerprint, MatcherKind, SourceLocation, StepDefinition};
use crate::source_adapter::suppression::inline_suppressions;
use crate::source_adapter::{
    registered_adapter, AdapterSessionState, ExtractionDiagnosticKind, SourceAdapter,
    SourceExtractionSession, StatefulSourceAdapter, INVALID_MATCHER_DIAGNOSTIC,
    UNPARSEABLE_SOURCE_DIAGNOSTIC_PREFIX, UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX,
    UNSUPPORTED_MATCHER_DIAGNOSTIC,
};
pub use crate::source_adapter::{
    Extraction, ExtractionDiagnostic, ExtractionDiagnosticLevel, SourceFile, SourceLanguage,
};
use anyhow::{Context, Result};
use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};
use tree_sitter::{Node, Parser};

/// JavaScript, JSX, or their module variants.
pub const JAVASCRIPT: SourceLanguage = SourceLanguage::new("javascript");
/// TypeScript or its module variants.
pub const TYPESCRIPT: SourceLanguage = SourceLanguage::new("typescript");
/// TypeScript with JSX syntax.
pub const TSX: SourceLanguage = SourceLanguage::new("tsx");

/// The tree-sitter grammar of a JavaScript/TypeScript language, or `None` for any other language.
pub(crate) fn grammar(language: SourceLanguage) -> Option<tree_sitter::Language> {
    match language {
        JAVASCRIPT => Some(tree_sitter_javascript::LANGUAGE.into()),
        TYPESCRIPT => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        TSX => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        _ => None, // fail-closed: another adapter's or an undeclared language has no grammar.
    }
}

pub(crate) struct TreeSitterSourceAdapter {
    name: &'static str,
    language: SourceLanguage,
}

pub(crate) static JAVASCRIPT_ADAPTER: TreeSitterSourceAdapter = TreeSitterSourceAdapter {
    name: "javascript",
    language: JAVASCRIPT,
};
pub(crate) static TYPESCRIPT_ADAPTER: TreeSitterSourceAdapter = TreeSitterSourceAdapter {
    name: "typescript",
    language: TYPESCRIPT,
};
pub(crate) static TSX_ADAPTER: TreeSitterSourceAdapter = TreeSitterSourceAdapter {
    name: "tsx",
    language: TSX,
};

#[derive(Default)]
pub(crate) struct TypeScriptExtractionSession {
    resolver: RegistrationResolver,
    configured_registrations: std::collections::BTreeSet<String>,
    configured_assertion_modules: std::collections::BTreeSet<String>,
}

impl TypeScriptExtractionSession {
    /// Creates reusable project resolution state with configured registration and assertion modules.
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
    /// Initializes project-backed state when a root is available, otherwise uses default state.
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
    /// Returns the adapter's stable diagnostic name.
    fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the source language accepted by this adapter.
    fn language(&self) -> SourceLanguage {
        self.language
    }

    /// Declares the JavaScript/TypeScript frameworks definitions may be attributed to.
    fn framework_contracts(&self) -> &'static [crate::source_adapter::FrameworkContract] {
        &frameworks::FRAMEWORK_CONTRACTS
    }

    /// Excludes installed packages, which every JavaScript/TypeScript framework shares.
    fn default_excludes(&self) -> &'static [&'static str] {
        &["**/node_modules/**"]
    }

    /// Declares the baseline JavaScript/TypeScript comparison policy; each definition carries the
    /// policy of its own framework.
    fn analysis_policy(&self) -> crate::source_adapter::AnalysisPolicy {
        semantics::default_policy()
    }

    /// Checks whether a file can be excluded before parsing using registration evidence.
    fn inspect_source(
        &self,
        file: &SourceFile,
        registrations: &[String],
    ) -> Option<crate::source_adapter::SourceExclusion> {
        source_filter::inspect(&file.path, registrations)
    }

    /// Extracts one source with a fresh session, returning definitions and diagnostics.
    fn extract(&self, source: &str, file: &SourceFile) -> Result<Extraction> {
        let mut session = SourceExtractionSession::default();
        self.extract_with_session(source, file, &mut session)
    }

    /// Extracts source using shared resolver state, rejecting a mismatched source language.
    fn extract_with_session(
        &self,
        source: &str,
        file: &SourceFile,
        session: &mut SourceExtractionSession,
    ) -> Result<Extraction> {
        if file.language != self.language {
            anyhow::bail!(
                "{} adapter cannot parse `{}` source {}",
                self.name,
                file.language.id(),
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
    framework_calls: &'source framework_callbacks::FrameworkCalls,
    root: Node<'tree>,
    file_scopes: &'source FileScopes,
    configured_assertion_modules: &'source BTreeSet<String>,
    handler_evidence: OnceCell<HandlerEvidence<'tree>>,
}

/// What only a handler reads. Built on a file's first handler: most files have none, and building
/// this walks the whole tree several times.
struct HandlerEvidence<'tree> {
    assertions: AssertionBindings,
    bindings: BTreeMap<String, Vec<HandlerBinding<'tree>>>,
    module_scope: ModuleScope<'tree>,
}

impl<'tree> AdapterContext<'_, 'tree> {
    /// Returns the file's handler evidence, building it on first use.
    fn handler_evidence(&self) -> &HandlerEvidence<'tree> {
        self.handler_evidence.get_or_init(|| {
            let mut bindings = BTreeMap::new();
            collect_handler_bindings(self.root, self.source, &mut bindings);
            HandlerEvidence {
                assertions: AssertionBindings::discover(
                    self.root,
                    self.source,
                    self.registrations,
                    self.configured_assertion_modules,
                    self.file_scopes,
                ),
                bindings,
                module_scope: ModuleScope::new(self.root),
            }
        })
    }
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
    /// Counts an unresolved registration and retains its first location for reporting.
    fn record(&mut self, location: SourceLocation) {
        self.count = self.count.saturating_add(1);
        self.first_location.get_or_insert(location);
    }

    /// Counts registrations passed to unmodeled code and retains the first location.
    fn record_passed(&mut self, location: SourceLocation) {
        self.passed = self.passed.saturating_add(1);
        self.first_passed.get_or_insert(location);
    }

    /// Counts unresolved calls per module while retaining each module's first location.
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
    Ok(registered_adapter(file.language)?
        .extract_file(file)?
        .definitions)
}

/// Extracts step definitions from in-memory source using `file` for syntax and locations.
pub fn extract(source: &str, file: &SourceFile) -> Result<Vec<StepDefinition>> {
    Ok(extract_detailed(source, file)?.definitions)
}

/// Extracts definitions and localized diagnostics from in-memory source.
pub fn extract_detailed(source: &str, file: &SourceFile) -> Result<Extraction> {
    registered_adapter(file.language)?.extract(source, file)
}

/// Extracts one file with the session's resolver state, returning definitions and diagnostics.
fn extract_detailed_impl(
    source: &str,
    file: &SourceFile,
    session: &mut TypeScriptExtractionSession,
) -> Result<Extraction> {
    let grammar = grammar(file.language)
        .expect("the adapter accepts only its own JavaScript/TypeScript language");
    let mut parser = Parser::new();
    parser
        .set_language(&grammar)
        .context("failed to initialize the JavaScript/TypeScript parser")?;
    let tree = parser
        .parse(source, None)
        .context("Tree-sitter did not return a syntax tree")?;

    let root = tree.root_node();
    let source_bytes = source.as_bytes();
    let source_lines: Vec<_> = source.lines().collect();
    let file_scopes = FileScopes::new(root, source_bytes);
    let framework = detect_framework(root, source_bytes, &file_scopes.loader);
    let registrations = detect_registrations(
        root,
        source_bytes,
        framework,
        &file.path,
        &mut session.resolver,
        &session.configured_registrations,
        &file_scopes,
    )?;
    let framework = registrations.framework;
    let framework_calls = framework_callbacks::discover(
        root,
        source_bytes,
        &file.path,
        &mut session.resolver,
        &registrations,
        &file_scopes,
    )?;
    let context = AdapterContext {
        source: source_bytes,
        source_lines: &source_lines,
        file,
        framework,
        registrations: &registrations,
        framework_calls: &framework_calls,
        root,
        file_scopes: &file_scopes,
        configured_assertion_modules: &session.configured_assertion_modules,
        handler_evidence: OnceCell::new(),
    };
    let mut definitions = Vec::new();
    let mut diagnostics = Vec::new();
    let mut unresolved_registration_calls = UnresolvedRegistrationCalls::default();
    if root.has_error() {
        let syntax_node = first_syntax_error(root).unwrap_or(root);
        // A parse failure still leaves error-recovered definitions worth analyzing, so it does not
        // discard the file. It is a typed completeness signal, which marks the
        // corpus incomplete; the CLI keeps the run non-fatal unless `--fail-on-unparseable` or
        // `--fail-on-incomplete` is set, so the level here stays a warning.
        diagnostics.push(ExtractionDiagnostic {
            kind: ExtractionDiagnosticKind::Unparseable,
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
            kind: ExtractionDiagnosticKind::Incomplete,
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
            kind: ExtractionDiagnosticKind::Incomplete,
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
            kind: ExtractionDiagnosticKind::Incomplete,
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
            kind: ExtractionDiagnosticKind::Incomplete,
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
    Ok(Extraction::new(definitions, diagnostics))
}

/// Walks calls in source order, collecting definitions and unresolved-registration evidence.
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
                registration_name(node, context.source, context.registrations).is_some()
                    || decorator_registration_name(node, context.source, context.registrations)
                        .is_some()
            }
            // `{ Given }` passes the binding under its own name.
            "shorthand_property_identifier" => context
                .registrations
                .recognizes_alias_at(node, node_text(node, context.source)),
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
            | "class_declaration"
            | "ambient_declaration" => continue,
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

/// Classifies registration-like calls whose provenance or execution cannot be established.
fn unresolved_registration_call<'a>(
    call: Node<'_>,
    context: &'a AdapterContext<'_, '_>,
) -> Option<UnresolvedRegistration<'a>> {
    let function = call.child_by_field_name("function")?;
    if context
        .framework_calls
        .incomplete
        .contains(&function.start_byte())
    {
        return Some(UnresolvedRegistration::Generic);
    }
    if context
        .framework_calls
        .handled
        .contains(&function.start_byte())
    {
        return None;
    }
    if registration_name(function, context.source, context.registrations)
        .is_some_and(|(_, _, framework)| framework_callbacks::uses_callbacks(framework))
    {
        return Some(UnresolvedRegistration::Generic);
    }
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
    if context
        .framework_calls
        .inert
        .contains(&function.start_byte())
    {
        return None;
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

/// Extracts the step definition registered by `call`, or records why it cannot.
fn extract_call<'tree>(
    call: Node<'tree>,
    context: &AdapterContext<'_, 'tree>,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
) -> Option<StepDefinition> {
    let source = context.source;
    let function = call.child_by_field_name("function")?;
    if context
        .registrations
        .forwarded_callees
        .contains(&function.start_byte())
    {
        return None;
    }
    let (callee, registration, registration_framework) =
        context.framework_calls.registration(function).or_else(|| {
            registration_name(function, source, context.registrations)
                .filter(|(_, _, framework)| !framework_callbacks::uses_callbacks(*framework))
        })?;
    let arguments = registrations::registration_arguments(call)?;
    let Some(&matcher_node) = arguments.first() else {
        warn_unsupported_matcher(call, context, diagnostics);
        return None;
    };
    let (matcher, mut matcher_kind, matcher_flags) =
        extract_matcher(matcher_node, context, diagnostics)?;
    if matcher_kind == MatcherKind::CucumberExpression
        && frameworks::callback_model(registration_framework)
            .is_some_and(|model| model.literal_strings)
    {
        matcher_kind = MatcherKind::Literal;
    }
    let handler = registrations::registration_handler(&arguments).filter(|node| {
        matches!(
            node.kind(),
            "arrow_function"
                | "function_expression"
                | "generator_function"
                | "identifier"
                | "member_expression"
                | "subscript_expression"
                | "call_expression"
        )
    });
    let Some(handler) = handler else {
        diagnostics.push(dropped_registration_diagnostic(context, call));
        return None;
    };
    let evidence = context.handler_evidence();
    // Frameworks disagree when a function precedes another candidate: Cucumber runs the function,
    // Playwright-BDD the last argument. Neither reading can be assumed.
    let contested = arguments
        .get(1)
        .copied()
        .and_then(registrations::unwrap_registration_callee)
        .is_some_and(|second| {
            second.id() != handler.id()
                && resolve_handler(second, source, &evidence.bindings).is_some()
        });
    if contested {
        diagnostics.push(dropped_registration_diagnostic(context, call));
        return None;
    }
    let bound_arguments = bind_arguments(handler, source);
    let resolved_handler = resolve_handler(handler, source, &evidence.bindings);
    if resolved_handler.is_none() && handler.kind() == "call_expression" {
        diagnostics.push(unsupported_handler_diagnostic(context, handler));
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
            &evidence.assertions,
            &evidence.module_scope,
        ),
    ))
}

/// Extracts the step definition a decorator `call` registers on its class method.
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
        warn_unsupported_matcher(call, context, diagnostics);
        return None;
    };
    let (matcher, matcher_kind, matcher_flags) =
        extract_matcher(matcher_node, context, diagnostics)?;
    let handler = decorated_method(call).and_then(|method| {
        let evidence = context.handler_evidence();
        fingerprint_method_handler(method, source, &evidence.assertions, &evidence.module_scope)
    });
    let Some(handler) = handler else {
        diagnostics.push(dropped_registration_diagnostic(context, call));
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
    let framework = effective_registration_framework(registration_framework, context.framework);
    let analysis_policy = semantics::policy(framework);
    StepDefinition {
        // The JavaScript dialect keys on the source pattern, so no translation is passed.
        normalized_matcher: analysis_policy.normalized_matcher(
            matcher_kind,
            &matcher,
            &matcher_flags,
            None,
        ),
        matcher,
        matcher_kind,
        matcher_flags,
        handler,
        framework,
        analysis_policy,
        registration: if registration.is_empty() {
            callee
        } else {
            registration
        },
        location: node_location(context.file, call, context.source),
        inline_suppressions: inline_suppressions(
            call.start_position().row,
            "//",
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

/// Uses file-level attribution only when the registration binding has no known framework.
fn effective_registration_framework(binding: Framework, file: Framework) -> Framework {
    if binding == Framework::UNKNOWN {
        file
    } else {
        binding
    }
}

/// Records an incompleteness warning at a matcher that cannot be extracted statically.
fn warn_unsupported_matcher(
    node: Node<'_>,
    context: &AdapterContext<'_, '_>,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
) {
    diagnostics.push(ExtractionDiagnostic::with_kind(
        ExtractionDiagnosticKind::Incomplete,
        ExtractionDiagnosticLevel::Warning,
        node_location(context.file, node, context.source),
        UNSUPPORTED_MATCHER_DIAGNOSTIC,
    ));
}

/// Extracts a static matcher and reports invalid literals or unsupported regex syntax.
fn extract_matcher(
    matcher_node: Node<'_>,
    context: &AdapterContext<'_, '_>,
    diagnostics: &mut Vec<ExtractionDiagnostic>,
) -> Option<(String, MatcherKind, String)> {
    let source = context.source;
    let matcher_node =
        registrations::unwrap_registration_callee(matcher_node).unwrap_or(matcher_node);
    let (matcher, matcher_kind, matcher_flags) = match matcher_value(matcher_node, source) {
        Some(value) => value,
        None if matcher_node.kind() == "string" => {
            diagnostics.push(ExtractionDiagnostic {
                kind: ExtractionDiagnosticKind::Incomplete,
                level: ExtractionDiagnosticLevel::Error,
                location: node_location(context.file, matcher_node, source),
                message: INVALID_MATCHER_DIAGNOSTIC.to_owned(),
            });
            return None;
        }
        None => {
            warn_unsupported_matcher(matcher_node, context, diagnostics);
            return None;
        }
    };
    if matcher_kind == MatcherKind::RegularExpression {
        match rust_regex_support(&matcher, &matcher_flags) {
            RegexSupport::Unsupported => diagnostics.push(ExtractionDiagnostic {
                kind: ExtractionDiagnosticKind::Other,
                level: ExtractionDiagnosticLevel::Warning,
                location: node_location(context.file, matcher_node, source),
                message: "regular expression uses syntax unsupported by static usage analysis; unused and ambiguity checks will treat this definition as indeterminate".to_owned(),
            }),
            // Analysis compiles the matcher in its matcher set, which has no capture groups, and
            // emits one run-level operational error only when that program exceeds the limit too.
            RegexSupport::ResourceLimit | RegexSupport::Supported => {}
        }
    }
    Some((matcher, matcher_kind, matcher_flags))
}

/// Builds the warning for a handler that cannot be compared statically.
fn unsupported_handler_diagnostic(
    context: &AdapterContext<'_, '_>,
    node: Node<'_>,
) -> ExtractionDiagnostic {
    ExtractionDiagnostic::with_kind(
        ExtractionDiagnosticKind::Other,
        ExtractionDiagnosticLevel::Warning,
        node_location(context.file, node, context.source),
        "dynamic or unsupported step handler cannot be compared statically",
    )
}

/// Reports a resolved registration that yields no definition. Analysis cannot see a step it
/// dropped, so the corpus is incomplete.
fn dropped_registration_diagnostic(
    context: &AdapterContext<'_, '_>,
    node: Node<'_>,
) -> ExtractionDiagnostic {
    ExtractionDiagnostic::with_kind(
        ExtractionDiagnosticKind::Incomplete,
        ExtractionDiagnosticLevel::Warning,
        node_location(context.file, node, context.source),
        "step registration has no static handler, so its definition is not analyzed",
    )
}

/// Maps a syntax node's byte span to one-based lines and Unicode character columns.
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

/// Converts a byte column to a one-based character column, falling back on invalid UTF-8.
fn unicode_column(source: &[u8], byte_offset: usize, byte_column: usize) -> usize {
    let line_start = byte_offset.saturating_sub(byte_column);
    let end = byte_offset.min(source.len());
    std::str::from_utf8(&source[line_start.min(end)..end])
        .map_or(byte_column, |prefix| prefix.chars().count())
        + 1
}

/// Finds the first error or missing node in a source-order named-node traversal.
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

/// Borrows a node's source text, returning an empty string when the slice is not UTF-8.
fn node_text<'a>(node: Node<'_>, source: &'a [u8]) -> &'a str {
    std::str::from_utf8(&source[node.byte_range()]).unwrap_or("")
}

#[cfg(test)]
mod tests;
