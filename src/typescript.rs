//! Tree-sitter based JavaScript and TypeScript step-definition extraction.

mod ast;
mod handler;
mod matcher;
mod module_resolver;
mod project_resolution;
mod registrations;
mod suppression;

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
use self::matcher::{
    matcher_value, normalize_matcher_with_flags, rust_regex_support, RegexSupport,
};
use self::module_resolver::RegistrationResolver;
use self::registrations::{
    decorator_registration_name, detect_framework, detect_registrations, registration_callee,
    registration_name, unresolved_registration_module, RegistrationCallee, RegistrationNames,
};
use self::suppression::inline_suppressions;
use crate::model::{Framework, MatcherKind, SourceLocation, StepDefinition};
use crate::source_adapter::{
    adapter_for_language, grammar_for_language, SourceAdapter, SourceExtractionSession,
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
}

impl TypeScriptExtractionSession {
    pub(crate) fn for_root(root: &std::path::Path, registrations: &[String]) -> Self {
        Self {
            resolver: RegistrationResolver::for_root(root),
            configured_registrations: registrations.iter().cloned().collect(),
        }
    }
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
        extract_detailed_impl(source, file, &mut session.typescript)
    }
}

struct AdapterContext<'source, 'tree> {
    source: &'source [u8],
    source_lines: &'source [&'source str],
    file: &'source SourceFile,
    framework: Framework,
    registrations: &'source RegistrationNames,
    handler_bindings: &'source BTreeMap<String, Vec<HandlerBinding<'tree>>>,
}

#[derive(Default)]
struct UnresolvedRegistrationCalls {
    count: usize,
    first_location: Option<SourceLocation>,
    modules: BTreeMap<String, (usize, SourceLocation)>,
}

impl UnresolvedRegistrationCalls {
    fn record(&mut self, location: SourceLocation) {
        self.count = self.count.saturating_add(1);
        self.first_location.get_or_insert(location);
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
    let mut handler_bindings = BTreeMap::new();
    collect_handler_bindings(root, source_bytes, &mut handler_bindings);
    let context = AdapterContext {
        source: source_bytes,
        source_lines: &source_lines,
        file,
        framework,
        registrations: &registrations,
        handler_bindings: &handler_bindings,
    };
    let mut definitions = Vec::new();
    let mut diagnostics = Vec::new();
    let mut unresolved_registration_calls = UnresolvedRegistrationCalls::default();
    if root.has_error() {
        let syntax_node = first_syntax_error(root).unwrap_or(root);
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Error,
            location: node_location(file, syntax_node, source_bytes),
            message: "source contains JavaScript/TypeScript syntax errors, so analysis is incomplete; fix the syntax, use a .tsx extension for JSX, or narrow definition discovery".to_owned(),
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
    Some(StepDefinition {
        normalized_matcher: normalize_matcher_with_flags(&matcher, matcher_kind, &matcher_flags),
        matcher,
        matcher_kind,
        matcher_flags,
        handler: fingerprint_handler(handler, bound_arguments, source, comparable),
        framework: effective_registration_framework(registration_framework, context.framework),
        registration: if registration.is_empty() {
            callee
        } else {
            registration
        },
        location: node_location(context.file, call, source),
        inline_suppressions: inline_suppressions(
            call,
            context.source_lines,
            context.file,
            diagnostics,
        ),
    })
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
    let Some(handler) = fingerprint_method_handler(method, source) else {
        diagnostics.push(ExtractionDiagnostic {
            level: ExtractionDiagnosticLevel::Warning,
            location: node_location(context.file, method, source),
            message: "dynamic or unsupported step handler cannot be compared statically".to_owned(),
        });
        return None;
    };
    Some(StepDefinition {
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
        location: node_location(context.file, call, source),
        inline_suppressions: inline_suppressions(
            call,
            context.source_lines,
            context.file,
            diagnostics,
        ),
    })
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
