//! Conservative, opt-in extraction of direct Cucumber-Ruby registrations.

use crate::model::{Framework, MatcherKind, SourceLocation, StepDefinition};
use crate::source_adapter::{
    AdapterSessionState, Extraction, ExtractionDiagnostic, ExtractionDiagnosticKind as Kind,
    ExtractionDiagnosticLevel as Level, SourceAdapter, SourceExtractionSession, SourceFile,
    SourceLanguage, StatefulSourceAdapter,
};
use anyhow::{bail, Context, Result};
use std::path::Path;
use tree_sitter::Node;

pub(crate) mod dependencies;
mod handler;
mod ownership;
pub(crate) mod parameters;

pub(crate) struct RubyAdapter;
pub(crate) static RUBY_ADAPTER: RubyAdapter = RubyAdapter;
pub(crate) struct RubySession {
    effects: ownership::RegistrationEffects,
}
impl AdapterSessionState for RubySession {
    fn initialize(_: Option<&Path>, _: &[String], _: &[String]) -> Self {
        Self {
            effects: Default::default(),
        }
    }
}
impl RubySession {
    pub(crate) fn invalidated(&self) -> bool {
        self.effects.invalidated()
    }
}
impl StatefulSourceAdapter for RubyAdapter {
    type State = RubySession;
}

impl SourceAdapter for RubyAdapter {
    fn name(&self) -> &'static str {
        "cucumber-ruby"
    }
    fn language(&self) -> SourceLanguage {
        SourceLanguage::Ruby
    }
    fn extract(&self, source: &str, file: &SourceFile) -> Result<Extraction> {
        let (mut extraction, _) = extract(source, file)?;
        mark_unresolved_dependency_usage(&mut extraction);
        Ok(extraction)
    }
    fn extract_with_session(
        &self,
        source: &str,
        file: &SourceFile,
        session: &mut SourceExtractionSession,
    ) -> Result<Extraction> {
        let (mut extraction, invalidated) = extract(source, file)?;
        extraction.diagnostics.retain(|diagnostic| {
            diagnostic.kind != Kind::Dependency
                || !session.dependency_resolved(&diagnostic.location)
        });
        mark_unresolved_dependency_usage(&mut extraction);
        session.state::<RubySession>()?.effects.extend(invalidated);
        Ok(extraction)
    }
}

fn mark_unresolved_dependency_usage(extraction: &mut Extraction) {
    if extraction
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == Kind::Dependency)
    {
        // Unknown providers may delegate to any local step. Only resolved source edges can
        // discharge this uncertainty, without clearing independent dynamic usage signals.
        extraction
            .indirect_usage
            .get_or_insert_with(Default::default)
            .unknown = true;
    }
}

fn extract(
    source: &str,
    file: &SourceFile,
) -> Result<(Extraction, ownership::RegistrationEffects)> {
    if file.language != SourceLanguage::Ruby {
        bail!("Ruby adapter requires Ruby source");
    }
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_ruby::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .context("Ruby parser produced no tree")?;
    let root = tree.root_node();
    let nodes = descendants(root);
    let mut result = Extraction {
        indirect_usage: Some(indirect_usage(&nodes, source)),
        ..Extraction::default()
    };
    if root.has_error() {
        result.diagnostics.push(diagnostic(
            file,
            root,
            source,
            Kind::Unparseable,
            "Ruby source contains syntax errors; extraction is incomplete",
        ));
    }
    result.parameter_types = parameters::collect(&nodes, source, file);
    let mut effects = ownership::RegistrationEffects::collect(root, source);
    if result
        .parameter_types
        .iter()
        .any(parameters::invalidates_registry)
    {
        effects.invalidate();
    }
    let replaced = effects.invalidated();
    if replaced {
        result.diagnostics.push(diagnostic(
            file,
            root,
            source,
            Kind::Incomplete,
            "Ruby registration ownership or executable source effects are unresolved",
        ));
    }
    for (node, name) in named_calls(&nodes, source) {
        if matches!(name, "require" | "require_relative" | "load" | "autoload") {
            result.diagnostics.push(diagnostic(
                file,
                node,
                source,
                Kind::Dependency,
                "Ruby source dependency is unresolved",
            ));
        }
        if let Some(receiver) = node.child_by_field_name("receiver") {
            if registration(name) && is_self_receiver(receiver) {
                result.diagnostics.push(diagnostic(
                    file,
                    node,
                    source,
                    Kind::Incomplete,
                    "Ruby self-qualified registration is outside direct-call extraction support",
                ));
            }
            continue;
        }
        if matches!(
            name,
            "eval" | "send" | "public_send" | "define_method" | "alias_method"
        ) {
            result.diagnostics.push(diagnostic(file, node, source, Kind::Incomplete,
                    "Ruby dependency, dynamic dispatch or parameter type is not resolved; definitions may be missing"));
        }
        if name == "ParameterType"
            && result.parameter_types.iter().any(|declaration| {
                declaration.location == location(file, node, source)
                    && (declaration.expression.is_none()
                        || parameters::invalidates_registry(declaration))
            })
        {
            result.diagnostics.push(diagnostic(
                file,
                node,
                source,
                Kind::Incomplete,
                "Ruby parameter type declaration is not statically resolved",
            ));
        }
        if !registration(name) {
            continue;
        }
        if replaced
            || node
                .parent()
                .is_none_or(|parent| parent.kind() != "program")
        {
            result.diagnostics.push(diagnostic(
                file,
                node,
                source,
                Kind::Incomplete,
                "Ruby registration is redefined or not a direct top-level call",
            ));
            continue;
        }
        let Some((matcher, matcher_kind, flags, block)) = registration_parts(node, source) else {
            result.diagnostics.push(diagnostic(
                file,
                node,
                source,
                Kind::Incomplete,
                "Ruby registration requires a static matcher and inline block",
            ));
            continue;
        };
        if matcher_kind == MatcherKind::RegularExpression
            && regex_expression(&matcher, &flags).is_none()
        {
            result.diagnostics.push(diagnostic(
                file,
                node,
                source,
                Kind::Incomplete,
                "Ruby regular expression is outside the supported static matching subset",
            ));
        }
        let handler = handler::fingerprint(block, root, source);
        if !handler.comparable {
            result.diagnostics.push(diagnostic(
                file,
                block,
                source,
                Kind::Incomplete,
                "Ruby handler contains syntax outside exact-handler comparison support",
            ));
        }
        let comparison = if matcher_kind == MatcherKind::CucumberExpression {
            crate::matcher::normalize_cucumber_expression(&matcher)
        } else {
            matcher.clone()
        };
        result.definitions.push(StepDefinition {
            normalized_matcher: format!("ruby:{matcher_kind:?}:{flags}:{comparison}"),
            matcher,
            matcher_kind,
            matcher_flags: flags,
            handler,
            framework: Framework::CucumberRuby,
            registration: name.to_owned(),
            location: location(file, node, source),
            inline_suppressions: vec![],
        });
    }
    Ok((result, effects))
}

fn is_self_receiver(mut node: Node<'_>) -> bool {
    while node.kind() == "parenthesized_statements" {
        let mut cursor = node.walk();
        let Some(value) = node
            .named_children(&mut cursor)
            .filter(|n| n.kind() != "comment")
            .last()
        else {
            return false;
        };
        node = value;
    }
    node.kind() == "self"
}

fn named_calls<'a>(
    nodes: &'a [Node<'a>],
    source: &'a str,
) -> impl Iterator<Item = (Node<'a>, &'a str)> {
    nodes.iter().copied().filter_map(move |node| {
        if node.kind() != "call" {
            return None;
        }
        let method = node.child_by_field_name("method")?;
        Some((node, text(method, source)))
    })
}

fn indirect_usage(nodes: &[Node<'_>], source: &str) -> crate::source_adapter::IndirectStepUsage {
    let mut usage = crate::source_adapter::IndirectStepUsage {
        unknown: nodes.iter().any(|n| {
            n.has_error()
                || matches!(n.kind(), "method" | "singleton_method")
                    && n.child_by_field_name("name")
                        .is_some_and(|name| matches!(text(name, source), "step" | "steps"))
        }),
        ..Default::default()
    };
    for (node, name) in named_calls(nodes, source) {
        if matches!(
            name,
            "send"
                | "public_send"
                | "__send__"
                | "method"
                | "public_method"
                | "eval"
                | "instance_eval"
                | "class_eval"
                | "module_eval"
        ) {
            usage.unknown = true;
        }
        if !matches!(name, "step" | "steps") {
            continue;
        }
        let argument = node
            .child_by_field_name("arguments")
            .and_then(|args| args.named_child(0));
        let literal = argument
            .filter(|arg| {
                arg.kind() == "string"
                    && !descendants(*arg)
                        .iter()
                        .any(|n| n.kind() == "interpolation")
            })
            .and_then(|arg| literal_string(text(arg, source)));
        if name == "step"
            && node
                .child_by_field_name("receiver")
                .is_none_or(|receiver| receiver.kind() == "self")
        {
            if let Some(value) = literal {
                usage.texts.insert(value);
                continue;
            }
        }
        usage.unknown = true;
    }
    usage
}

fn static_method_name(node: Node<'_>, source: &str) -> Option<String> {
    match node.kind() {
        "simple_symbol" => text(node, source).strip_prefix(':').map(str::to_owned),
        "string"
            if !descendants(node)
                .iter()
                .any(|n| n.kind() == "interpolation") =>
        {
            literal_string(text(node, source))
        }
        _ => None,
    }
}

fn protected_method(name: &str) -> bool {
    registration(name)
        || matches!(
            name,
            "ParameterType"
                | "World"
                | "lambda"
                | "proc"
                | "method_missing"
                | "require"
                | "require_relative"
                | "autoload"
                | "define_method"
                | "define_singleton_method"
                | "alias_method"
                | "undef_method"
                | "remove_method"
                | "send"
                | "public_send"
                | "__send__"
                | "step"
                | "steps"
        )
}

fn may_replace_dsl(node: Node<'_>, source: &str) -> bool {
    let Some(method) = node.child_by_field_name("method") else {
        return false;
    };
    let mut name = text(method, source).to_owned();
    let mut cursor = node.walk();
    let args: Vec<_> = node
        .child_by_field_name("arguments")
        .map(|args| args.named_children(&mut cursor).collect())
        .unwrap_or_default();
    let mut args = args.into_iter();
    let mut dispatched = false;
    while matches!(name.as_str(), "send" | "public_send" | "__send__") {
        dispatched = true;
        let Some(target) = args.next().and_then(|arg| static_method_name(arg, source)) else {
            return true;
        };
        name = target;
    }
    if matches!(name.as_str(), "undef_method" | "remove_method") {
        return args.any(|arg| {
            static_method_name(arg, source).is_none_or(|target| protected_method(&target))
        });
    }
    if matches!(
        name.as_str(),
        "define_method"
            | "define_singleton_method"
            | "alias_method"
            | "undef_method"
            | "remove_method"
    ) {
        return args
            .next()
            .and_then(|arg| static_method_name(arg, source))
            .is_none_or(|target| protected_method(&target));
    }
    (dispatched && protected_method(&name))
        || matches!(
            name.as_str(),
            "autoload"
                | "eval"
                | "class_eval"
                | "module_eval"
                | "instance_eval"
                | "method"
                | "public_method"
                | "include"
                | "extend"
                | "prepend"
                | "const_get"
                | "const_set"
                | "remove_const"
        )
}

fn inside_deferred_body(node: Node<'_>, source: &str) -> bool {
    let mut parent = node.parent();
    while let Some(block) = parent {
        parent = block.parent();
        if !matches!(block.kind(), "block" | "do_block") {
            continue;
        }
        let Some(call) = block.parent() else { continue };
        if call.kind() == "call" && call.child_by_field_name("block") == Some(block) {
            let method = call.child_by_field_name("method").map(|n| text(n, source));
            if call.parent().is_some_and(|p| p.kind() == "program")
                && call.child_by_field_name("receiver").is_none()
                && method.is_some_and(registration)
            {
                return true;
            }
            if call.child_by_field_name("receiver").is_some()
                || call
                    .child_by_field_name("arguments")
                    .is_some_and(|a| a.named_child_count() != 0)
                || !matches!(method, Some("lambda" | "proc"))
            {
                continue;
            }
        } else if call.kind() != "lambda" {
            continue;
        }
        let Some(pair) = call.parent().filter(|p| p.kind() == "pair") else {
            continue;
        };
        if pair.child_by_field_name("value") != Some(call)
            || !pair.child_by_field_name("key").is_some_and(|key| {
                key.kind() == "hash_key_symbol" && text(key, source) == "transformer"
            })
        {
            continue;
        }
        let registration = pair
            .parent()
            .filter(|p| p.kind() == "argument_list")
            .and_then(|p| p.parent());
        if registration.is_some_and(|r| {
            r.kind() == "call"
                && r.parent().is_some_and(|p| p.kind() == "program")
                && r.child_by_field_name("receiver").is_none()
                && r.child_by_field_name("method")
                    .is_some_and(|m| text(m, source) == "ParameterType")
        }) {
            return true;
        }
    }
    false
}

fn registration(name: &str) -> bool {
    matches!(name, "Given" | "When" | "Then" | "And" | "But")
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn descendants(root: Node<'_>) -> Vec<Node<'_>> {
    let mut result = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        result.push(node);
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    result
}

fn registration_parts<'a>(
    node: Node<'a>,
    source: &str,
) -> Option<(String, MatcherKind, String, Node<'a>)> {
    let args = node.child_by_field_name("arguments")?;
    if args.named_child_count() != 1 {
        return None;
    }
    let matcher = args.named_child(0)?;
    let block = node.child_by_field_name("block")?;
    if matcher.has_error() || block.has_error() {
        return None;
    }
    if descendants(matcher)
        .iter()
        .any(|node| node.kind() == "interpolation")
    {
        return None;
    }
    let raw = text(matcher, source);
    match matcher.kind() {
        "string" => Some((
            literal_string(raw)?,
            MatcherKind::CucumberExpression,
            String::new(),
            block,
        )),
        "regex" => {
            let (pattern, flags) = regex_literal(raw)?;
            Some((pattern, MatcherKind::RegularExpression, flags, block))
        }
        _ => None,
    }
}

fn regex_literal(raw: &str) -> Option<(String, String)> {
    let (start, delimiter) = if raw.starts_with('/') {
        (1, '/')
    } else if raw.starts_with("%r{") {
        (3, '}')
    } else {
        return None;
    };
    let end = raw.rfind(delimiter)?;
    (end >= start).then(|| (raw[start..end].to_owned(), raw[end + 1..].to_owned()))
}

fn literal_string(raw: &str) -> Option<String> {
    let quote = raw.chars().next()?;
    if !matches!(quote, '\'' | '"') || !raw.ends_with(quote) {
        return None;
    }
    let mut chars = raw[1..raw.len() - 1].chars();
    let mut value = String::new();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            value.push(ch);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            '\\' => value.push('\\'),
            c if c == quote => value.push(c),
            'n' if quote == '"' => value.push('\n'),
            'r' if quote == '"' => value.push('\r'),
            't' if quote == '"' => value.push('\t'),
            c if quote == '\'' => {
                value.push('\\');
                value.push(c);
            }
            _ => return None,
        }
    }
    Some(value)
}

/// Ruby line anchors are multiline by default; unsupported constructs never reach JS conversion.
pub(crate) fn regex_expression(pattern: &str, flags: &str) -> Option<String> {
    if pattern.len() > crate::resource_limits::MAX_REGEX_PATTERN_BYTES
        || flags.chars().any(|flag| flag != 'm')
        || pattern.contains("[[:")
        || pattern.contains("&&")
        || ["++", "*+", "?+", "}+"]
            .iter()
            .any(|operator| pattern.contains(operator))
    {
        return None;
    }
    let mut result = String::from(if flags.contains('m') { "(?ms:" } else { "(?m:" });
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    let mut first_class_member = false;
    let mut class_negated = false;
    while let Some(ch) = chars.next() {
        if ch == '{' && !in_class {
            if let Some(bound) = repetition_bound(&mut chars)? {
                result.push('{');
                result.push_str(&bound);
                result.push('}');
            } else {
                result.push_str(r"\{");
            }
            continue;
        }
        if ch == '}' && !in_class {
            result.push_str(r"\}");
            continue;
        }
        if in_class {
            if ch == ']' && !first_class_member {
                in_class = false;
            } else if ch == '^' && first_class_member && !class_negated {
                class_negated = true;
            } else {
                first_class_member = false;
            }
        } else if ch == '[' {
            in_class = true;
            first_class_member = true;
            class_negated = false;
        }

        if ch == '(' && chars.peek() == Some(&'?') {
            chars.next();
            if chars.next() != Some(':') {
                return None;
            }
            result.push_str("(?:");
            continue;
        }
        if ch != '\\' {
            result.push(ch);
            continue;
        }
        let escaped = chars.next()?;
        match escaped {
            'd' => result.push_str("[0-9]"),
            'D' => result.push_str("[^0-9]"),
            'w' => result.push_str("[A-Za-z0-9_]"),
            'W' => result.push_str("[^A-Za-z0-9_]"),
            's' => result.push_str(r"[ \t\r\n\x0c\x0b]"),
            'S' => result.push_str(r"[^ \t\r\n\x0c\x0b]"),
            'A' | 'z' | 'n' | 'r' | 't' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']'
            | '{' | '}' | '^' | '$' | '|' => {
                result.push('\\');
                result.push(escaped);
            }
            c if c.is_ascii_punctuation() => result.push_str(&regex::escape(&c.to_string())),
            _ => return None,
        }
    }
    result.push(')');
    regex::Regex::new(&result).ok()?;
    Some(result)
}

// Consume only Ruby repetition syntax. Other braces are literal; excessively long numeric
// bounds stay unsupported rather than spending unbounded work or treating a bound as text.
fn repetition_bound(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Option<Option<String>> {
    let mut probe = chars.clone();
    let mut bound = String::new();
    while let Some(ch) = probe.peek().copied() {
        if ch == '}' {
            let parts: Vec<_> = bound.split(',').collect();
            let valid = match parts.as_slice() {
                [count] => !count.is_empty(),
                [minimum, maximum] => !minimum.is_empty() || !maximum.is_empty(),
                _ => false,
            };
            if !valid {
                return Some(None);
            }
            probe.next();
            *chars = probe;
            if bound.starts_with(',') {
                bound.insert(0, '0');
            }
            return Some(Some(bound));
        }
        if !ch.is_ascii_digit() && ch != ',' {
            return Some(None);
        }
        if bound.len() >= 32 {
            return None;
        }
        bound.push(ch);
        probe.next();
    }
    Some(None)
}

fn location(file: &SourceFile, node: Node<'_>, source: &str) -> SourceLocation {
    let column = |byte: usize| {
        source[..byte]
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .chars()
            .count()
            + 1
    };
    SourceLocation::new(
        &file.path,
        node.start_position().row + 1,
        column(node.start_byte()),
        node.end_position().row + 1,
        column(node.end_byte()),
    )
}

fn diagnostic(
    file: &SourceFile,
    node: Node<'_>,
    source: &str,
    kind: Kind,
    message: &str,
) -> ExtractionDiagnostic {
    ExtractionDiagnostic::with_kind(kind, Level::Warning, location(file, node, source), message)
}

#[cfg(test)]
mod tests;
