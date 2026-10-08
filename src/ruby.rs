//! Conservative, opt-in extraction of direct Cucumber-Ruby registrations.

pub(crate) mod semantics;

use crate::model::{Framework, MatcherKind, SourceLocation, StepDefinition};
use crate::source_adapter::{
    AdapterSessionState, Extraction, ExtractionDiagnostic, ExtractionDiagnosticKind as Kind,
    ExtractionDiagnosticLevel as Level, SourceAdapter, SourceExtractionSession, SourceFile,
    SourceFinalization, SourceLanguage, SourceUncertainty, StatefulSourceAdapter, UncertaintyCause,
    UncertaintyScope,
};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use tree_sitter::Node;

mod assertions;
mod bindings;
pub(crate) mod config;
mod dependencies;
mod handler;
mod load_phase;
mod ownership;
mod parameters;
mod providers;
mod registration_aliases;
mod registration_wrappers;
mod step_keywords;

pub(crate) struct RubyAdapter;
pub(crate) static RUBY_ADAPTER: RubyAdapter = RubyAdapter;
pub(crate) struct RubySession {
    effects: ownership::RegistrationEffects,
    providers: providers::Providers,
    assertions: assertions::AssertionProviders,
    assertion_modules: Vec<String>,
    /// Set when provider proofs are unavailable because the selection exceeds the proof budget.
    proof_sources_exceeded: bool,
    /// Ruby files prepared for the session and those actually extracted; a prepared file that was
    /// never extracted (unreadable, or skipped by a caller) still loads, so the suite is open.
    prepared: std::collections::BTreeSet<PathBuf>,
    extracted: std::collections::BTreeSet<PathBuf>,
}
impl AdapterSessionState for RubySession {
    /// Resolves configured Ruby provider paths with the same rule discovery applies to loads.
    fn initialize(root: Option<&Path>, _: &[String], assertion_modules: &[String]) -> Self {
        let mut configured = assertion_modules.to_vec();
        if let Some(root) = root {
            // A configured module names a load request, resolved exactly as discovery resolves one.
            for module in assertion_modules {
                if let Some(path) = dependencies::request_target(&[root.to_owned()], module) {
                    configured.push(path.to_string_lossy().into_owned());
                }
            }
        }
        Self {
            effects: Default::default(),
            providers: Default::default(),
            assertions: Default::default(),
            assertion_modules: configured,
            proof_sources_exceeded: false,
            prepared: Default::default(),
            extracted: Default::default(),
        }
    }
}
impl StatefulSourceAdapter for RubyAdapter {
    type State = RubySession;
    fn discover_dependencies(
        config: &crate::config::Config,
        excludes: &globset::GlobSet,
        files: &mut crate::discovery::DiscoveredFiles,
    ) {
        dependencies::resolve(config, excludes, files);
    }
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
    fn discover_by_default(&self) -> bool {
        false
    }
    fn supports_indirect_usage(&self) -> bool {
        true
    }
    fn resolve_parameter_types(
        &self,
        declarations: Vec<crate::source_adapter::SourceParameterType>,
        configured: &std::collections::BTreeMap<String, String>,
    ) -> (std::collections::BTreeMap<String, String>, Vec<String>) {
        parameters::merge(declarations, configured)
    }
    /// Loads provider and assertion proofs, recording when the selection exceeds their budget.
    fn prepare_session(
        &self,
        files: &[SourceFile],
        session: &mut SourceExtractionSession,
    ) -> Result<()> {
        let source_budget = (
            crate::resource_limits::MAX_REGISTRATION_MODULES,
            crate::resource_limits::MAX_REGISTRATION_MODULE_BYTES,
        );
        let (units, exceeded) = match providers::load_proof_sources(files, source_budget)? {
            providers::ProofSources::Loaded(units) => (Some(units), false),
            providers::ProofSources::OverBudget => (None, true),
            providers::ProofSources::Unreadable => (None, false),
        };
        let edges = session.dependency_edges();
        let state = session.state::<RubySession>()?;
        state.proof_sources_exceeded = exceeded;
        state.prepared = files
            .iter()
            .filter(|file| file.language == SourceLanguage::Ruby)
            .map(|file| file.path.clone())
            .collect();
        if let Some(units) = units {
            let (providers, assertions) = providers::Providers::from_units_with_assertions(
                &units,
                &edges,
                &state.assertion_modules,
            );
            state.providers = providers;
            state.assertions = assertions;
        } else {
            state.providers = Default::default();
            state.assertions = if state.assertion_modules.is_empty() {
                Default::default()
            } else {
                assertions::AssertionProviders::unavailable()
            };
        }
        Ok(())
    }
    /// Reports suite-wide Ruby uncertainties, naming the proof limit when it caused them.
    fn finalize_session(
        &self,
        session: &mut SourceExtractionSession,
    ) -> Result<SourceFinalization> {
        let mut result = SourceFinalization::default();
        let state = session.state::<RubySession>()?;
        result
            .advisories
            .extend_from_slice(state.assertions.advisories());
        let limit = state
            .proof_sources_exceeded
            .then(proof_limit_reason)
            .unwrap_or_default();
        if state.assertions.incomplete() {
            result.uncertainties.push(SourceUncertainty::new(
                UncertaintyScope::Registry(SourceLanguage::Ruby),
                UncertaintyCause::Source,
                format!("Ruby assertion-provider provenance is incomplete{limit}"),
            ));
        }
        if !state.prepared.is_subset(&state.extracted) {
            state.effects.open_load_phase();
        }
        // A call contained in an instance method runs during load once the suite's load phase is
        // open, exactly like any other unresolved effect.
        let escaped = state.effects.escaped_call().map(|site| {
            format!(
                "Ruby call at {}:{}:{} that may redefine the DSL or dispatch dynamically can run during load because the suite's load-time code is not limited to registrations, literal loads and plain class definitions",
                site.path.display(),
                site.line,
                site.column
            )
        });
        let cause = escaped
            .as_ref()
            .map(|reason| format!("; {reason}"))
            .unwrap_or_default();
        let invalidated = state.effects.invalidated() || escaped.is_some();
        if state.providers.work_exhausted() {
            result.uncertainties.push(SourceUncertainty::new(
                UncertaintyScope::Registry(SourceLanguage::Ruby),
                if invalidated {
                    UncertaintyCause::Registration
                } else {
                    UncertaintyCause::Source
                },
                if invalidated {
                    format!("Ruby registration source proof exceeded its work limit; DSL redefinition or metaprogramming also prevents trusted registration extraction{cause}")
                } else {
                    "Ruby registration source proof exceeded its work limit".to_owned()
                },
            ));
        } else if invalidated {
            result.uncertainties.push(SourceUncertainty::new(
                UncertaintyScope::Registry(SourceLanguage::Ruby),
                UncertaintyCause::Registration,
                format!("Ruby DSL redefinition or metaprogramming prevents trusted registration extraction across the selected suite{limit}{cause}"),
            ));
        }
        Ok(result)
    }
    /// Extracts one file, keeping dependency diagnostics for unfollowed loads and their reasons.
    fn extract_with_session(
        &self,
        source: &str,
        file: &SourceFile,
        session: &mut SourceExtractionSession,
    ) -> Result<Extraction> {
        let assertions = session
            .state::<RubySession>()?
            .assertions
            .get(&file.path, source);
        let proof = session
            .state::<RubySession>()?
            .providers
            .get(&file.path, source);
        let (mut extraction, mut invalidated) =
            extract_with_proof(source, file, proof, assertions.as_ref(), true)?;
        extraction.diagnostics.retain(|diagnostic| {
            diagnostic.kind != Kind::Dependency
                || !session.dependency_resolved(&diagnostic.location)
        });
        for diagnostic in &mut extraction.diagnostics {
            if let Some(reason) = (diagnostic.kind == Kind::Dependency)
                .then(|| session.dependency_gap(&diagnostic.location))
                .flatten()
            {
                diagnostic.message = format!("Ruby source dependency was not followed: {reason}");
            }
        }
        mark_unresolved_dependency_usage(&mut extraction);
        // An unresolved top-level load runs unanalyzed code during load, so the suite's load phase
        // is not closed. Loads inside handlers or methods run only when those do.
        if extraction.diagnostics.iter().any(|diagnostic| {
            diagnostic.kind == Kind::Dependency && invalidated.load_require(&diagnostic.location)
        }) {
            invalidated.open_load_phase();
        }
        let state = session.state::<RubySession>()?;
        state.extracted.insert(file.path.clone());
        state.effects.extend(invalidated);
        Ok(extraction)
    }
}

/// The clause naming the provider-proof budget, appended to a suite-wide uncertainty.
fn proof_limit_reason() -> String {
    format!(
        ": provider proofs are unavailable because the selected and loaded Ruby files exceed the {}-file / {} proof limit",
        dependencies::grouped(crate::resource_limits::MAX_REGISTRATION_MODULES),
        dependencies::byte_limit(crate::resource_limits::MAX_REGISTRATION_MODULE_BYTES)
    )
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

/// Extracts one source without a session; no call is contained without suite finalization.
fn extract(
    source: &str,
    file: &SourceFile,
) -> Result<(Extraction, ownership::RegistrationEffects)> {
    // Without a session nothing finalizes the suite, so no call can be contained.
    extract_with_proof(source, file, None, None, false)
}

/// Extracts registrations using source-owned provider and assertion evidence.
fn extract_with_proof(
    source: &str,
    file: &SourceFile,
    proof: Option<&providers::Proof>,
    assertions: Option<&assertions::AssertionBindings>,
    finalized: bool,
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
    let proof = proof.filter(|_| !root.has_error());
    let nodes = descendants(root);
    let lines: Vec<_> = source.lines().collect();
    let mut aliases = registration_aliases::RegistrationAliases::collect(root, source);
    if let Some(proof) = proof {
        aliases.extend_provider(root, proof);
    }
    let wrappers = registration_wrappers::RegistrationWrappers::collect(root, source, &aliases);
    let mut result = Extraction {
        indirect_usage: Some(indirect_usage(&nodes, source, &aliases, proof)),
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
    let mut effects = ownership::RegistrationEffects::collect(
        file, finalized, root, source, &aliases, &wrappers, proof,
    );
    if let Some(declaration) = result
        .parameter_types
        .iter()
        .find(|declaration| parameters::invalidates_registry(declaration))
    {
        effects.invalidate(nodes.iter().copied().find(|node| {
            node.kind() == "call" && location(file, *node, source) == declaration.location
        }));
    }
    let replaced = effects.invalidated();
    if replaced {
        let cause = effects
            .invalidation_site()
            .and_then(|(start, end)| root.descendant_for_byte_range(start, end))
            .unwrap_or(root);
        result.diagnostics.push(diagnostic(
            file,
            cause,
            source,
            Kind::Incomplete,
            "Ruby registration ownership or executable source effects are unresolved",
        ));
    }
    for (node, name) in named_calls(&nodes, source) {
        if proof.is_some_and(|p| p.unresolved_calls.contains(&node.start_byte())) {
            result
                .indirect_usage
                .get_or_insert_with(Default::default)
                .unknown = true;
            result.diagnostics.push(diagnostic(
                file,
                node,
                source,
                Kind::Incomplete,
                "Ruby constant provider callable has unresolved registration provenance",
            ));
        }
        if wrappers.forwarding(node)
            || proof.is_some_and(|p| p.forwarding.contains(&node.start_byte()))
        {
            continue;
        }
        let alias = aliases.registration(node);
        let name = alias
            .or_else(|| wrappers.registration(node))
            .unwrap_or(name);
        // A loader call without operands loads nothing (Ruby raises ArgumentError), so it can
        // neither resolve nor leave a dependency unresolved.
        if matches!(name, "require" | "require_relative" | "load" | "autoload")
            && has_operands(node)
        {
            result.diagnostics.push(diagnostic(
                file,
                node,
                source,
                Kind::Dependency,
                "Ruby source dependency is unresolved",
            ));
        }
        if let Some(receiver) = node
            .child_by_field_name("receiver")
            .filter(|_| alias.is_none())
        {
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
            || (node
                .parent()
                .is_none_or(|parent| parent.kind() != "program")
                && !proof.is_some_and(|p| p.executed.contains(&node.start_byte())))
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
        let Some((matcher, matcher_kind, flags, block)) = registration_parts(
            node,
            source,
            proof.is_some_and(|p| p.handlers.contains_key(&node.start_byte())),
        ) else {
            result.diagnostics.push(diagnostic(
                file,
                node,
                source,
                Kind::Incomplete,
                "Ruby registration requires a static matcher and a statically resolved handler",
            ));
            continue;
        };
        let translated_regex = (matcher_kind == MatcherKind::RegularExpression)
            .then(|| regex_expression(&matcher, &flags))
            .flatten();
        if matcher_kind == MatcherKind::RegularExpression && translated_regex.is_none() {
            result.diagnostics.push(diagnostic(
                file,
                node,
                source,
                Kind::Incomplete,
                "Ruby regular expression is outside the supported static matching subset",
            ));
        }
        let handler = proof
            .and_then(|p| p.handlers.get(&node.start_byte()))
            .cloned()
            .unwrap_or_else(|| handler::fingerprint(block, root, source, assertions));
        if !handler.comparable
            && !proof.is_some_and(|proof| proof.comparison_rejections.contains(&node.start_byte()))
            && !assertions
                .is_some_and(|assertions| assertions.rejected_comparison(block, root, source))
        {
            result.diagnostics.push(diagnostic(
                file,
                block,
                source,
                Kind::Incomplete,
                "Ruby handler comparison has unresolved syntax, captures or receiver identity",
            ));
        }
        let comparison = if matcher_kind == MatcherKind::CucumberExpression {
            crate::matcher::normalize_cucumber_expression(&matcher)
        } else {
            // Translation preserves Ruby capture arity. Flags have their own comparison
            // partition, so strip only the generated outer flag group from this key.
            translated_regex
                .map(|expression| {
                    expression[expression.find(':').unwrap() + 1..expression.len() - 1].to_owned()
                })
                .unwrap_or_else(|| matcher.clone())
        };
        result.definitions.push(StepDefinition {
            normalized_matcher: comparison,
            matcher,
            matcher_kind,
            matcher_flags: flags,
            handler,
            framework: Framework::CucumberRuby,
            registration: name.to_owned(),
            location: location(file, node, source),
            inline_suppressions: crate::source_adapter::suppression::inline_suppressions(
                node.start_position().row,
                "#",
                &lines,
                file,
                &mut result.diagnostics,
            ),
        });
    }
    result
        .definitions
        .sort_by_key(|item| (item.location.line, item.location.column));
    result
        .diagnostics
        .sort_by_key(|item| (item.location.line, item.location.column));
    Ok((result, effects))
}

/// Whether a call passes any argument, ignoring comments inside the parentheses.
fn has_operands(call: Node<'_>) -> bool {
    call.child_by_field_name("arguments")
        .is_some_and(|arguments| {
            // A block argument passes no operand either: `load(&blk)` still raises.
            bindings::semantic_children(arguments)
                .iter()
                .any(|argument| argument.kind() != "block_argument")
        })
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

fn indirect_usage(
    nodes: &[Node<'_>],
    source: &str,
    aliases: &registration_aliases::RegistrationAliases,
    proof: Option<&providers::Proof>,
) -> crate::source_adapter::IndirectStepUsage {
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
        if aliases.capture(node)
            || proof.is_some_and(|p| p.isolated_calls.contains(&node.start_byte()))
        {
            continue;
        }
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

fn method_table_mutator(name: &str) -> bool {
    matches!(
        name,
        "define_method"
            | "define_singleton_method"
            | "alias_method"
            | "undef_method"
            | "remove_method"
    )
}

/// Method names whose redefinition or table mutation can redirect registration or deferral.
fn protected_method(name: &str) -> bool {
    deferring_dsl_method(name)
        || method_table_mutator(name)
        || matches!(
            name,
            "ParameterType"
                | "World"
                | "lambda"
                | "proc"
                | "method"
                | "public_method"
                | "call"
                | "to_proc"
                | "method_missing"
                | "require"
                | "require_relative"
                | "autoload"
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
    if method_table_mutator(name.as_str()) {
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

/// Whether `node` sits in a block that runs only during scenarios: a step handler, a scenario hook
/// block, or a `ParameterType` transformer.
fn inside_deferred_body(
    node: Node<'_>,
    source: &str,
    aliases: &registration_aliases::RegistrationAliases,
    wrappers: &registration_wrappers::RegistrationWrappers,
) -> bool {
    let mut parent = node.parent();
    while let Some(block) = parent {
        parent = block.parent();
        if !matches!(block.kind(), "block" | "do_block") {
            continue;
        }
        let Some(call) = block.parent() else { continue };
        if call.kind() == "call" && call.child_by_field_name("block") == Some(block) {
            let method = call.child_by_field_name("method").map(|n| text(n, source));
            // Step handlers and scenario hook blocks run only during scenarios.
            if call.parent().is_some_and(|p| p.kind() == "program")
                && (aliases.registration(call).is_some()
                    || wrappers.registration(call).is_some()
                    || (call.child_by_field_name("receiver").is_none()
                        && method.is_some_and(deferring_dsl_method)))
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

/// Cucumber-Ruby aliases every dialect's step keywords onto `register_rb_step_definition`
/// regardless of the feature-file language, so each one is a registrar in every step file.
/// DSL methods whose block runs only during scenarios: step keywords and scenario hooks. The same
/// names are protected against redefinition, so a block is deferred only while its method is the
/// runner's own.
fn deferring_dsl_method(name: &str) -> bool {
    registration(name) || load_phase::scenario_hook(name)
}

fn registration(name: &str) -> bool {
    step_keywords::STEP_KEYWORDS.binary_search(&name).is_ok()
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
    resolved_handler: bool,
) -> Option<(String, MatcherKind, String, Node<'a>)> {
    let args = node.child_by_field_name("arguments")?;
    if args.named_child_count() != 1
        && !(resolved_handler
            && args.named_child_count() == 2
            && node.child_by_field_name("block").is_none()
            && args
                .named_child(1)
                .is_some_and(|n| n.kind() == "block_argument"))
    {
        return None;
    }
    let matcher = args.named_child(0)?;
    let block = node
        .child_by_field_name("block")
        .or_else(|| resolved_handler.then(|| args.named_child(1)).flatten())?;
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
    (end >= start).then(|| {
        let flags = raw[end + 1..]
            .chars()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        (raw[start..end].to_owned(), flags)
    })
}

fn literal_string(raw: &str) -> Option<String> {
    let quote = raw.chars().next()?;
    if raw.len() < 2 || !matches!(quote, '\'' | '"') || !raw.ends_with(quote) {
        return None;
    }
    let mut chars = raw[1..raw.len() - 1].chars().peekable();
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
            'u' if quote == '"' => value.push_str(&unicode_escape(&mut chars)?),
            c if quote == '\'' => {
                value.push('\\');
                value.push(c);
            }
            _ => return None,
        }
    }
    Some(value)
}

/// Decodes Unicode scalar escapes; Ruby does not use UTF-16 surrogate pairs.
fn unicode_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    let braced = chars.peek() == Some(&'{');
    let mut digits = String::new();
    if braced {
        chars.next();
        loop {
            let ch = chars.next()?;
            if ch == '}' {
                break;
            }
            if ch == '\n' {
                return None;
            }
            digits.push(ch);
        }
    } else {
        for _ in 0..4 {
            digits.push(chars.next()?);
        }
        if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
    }
    let mut decoded = String::new();
    for scalar in digits.split_ascii_whitespace() {
        if scalar.len() > 6 || !scalar.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        decoded.push(char::from_u32(u32::from_str_radix(scalar, 16).ok()?)?);
    }
    Some(decoded)
}

/// Full Unicode folds can consume multiple literal ASCII letters in Ruby, unlike Rust regex.
fn compatible_ignorecase_literal(pattern: &str) -> bool {
    let folded = pattern.to_ascii_lowercase();
    pattern.is_ascii()
        && !pattern.chars().any(|c| r"\.[](){}*+?|".contains(c))
        && !["ss", "ff", "fi", "fl", "st"]
            .iter()
            .any(|fold| folded.contains(fold))
}

/// Ruby line anchors are multiline by default; unsupported constructs never reach JS conversion.
pub(crate) fn regex_expression(pattern: &str, flags: &str) -> Option<String> {
    if pattern.len() > crate::resource_limits::MAX_REGEX_PATTERN_BYTES
        || flags
            .chars()
            .any(|flag| !matches!(flag, 'm' | 'i' | 'u' | 'n'))
        || (flags.contains('n') && (!pattern.is_ascii() || flags.contains('u')))
        || (flags.contains('i') && !compatible_ignorecase_literal(pattern))
        || pattern.contains("[[:")
        || pattern.contains("&&")
        || ["++", "*+", "?+", "}+"]
            .iter()
            .any(|operator| pattern.contains(operator))
    {
        return None;
    }
    let mut result = String::from(if flags.contains('m') { "(?ms:" } else { "(?m:" });
    if flags.contains('i') {
        result.insert(2, 'i');
    }
    let mut chars = pattern.chars().peekable();
    let mut class_depth = 0;
    let mut first_class_member = false;
    let mut class_negated = false;
    let mut capture_names = std::collections::BTreeSet::new();
    let mut ordinary_captures = Vec::new();
    while let Some(ch) = chars.next() {
        let in_class = class_depth > 0;
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
        if ch == '[' {
            class_depth += 1;
            first_class_member = true;
            class_negated = false;
        } else if in_class {
            if ch == ']' && !first_class_member {
                class_depth -= 1;
            } else if ch == '^' && first_class_member && !class_negated {
                class_negated = true;
            } else {
                first_class_member = false;
            }
        }

        if ch == '(' && !in_class {
            if chars.peek() != Some(&'?') {
                ordinary_captures.push(result.len() + 1);
            } else {
                chars.next();
                match chars.next()? {
                    ':' => result.push_str("(?:"),
                    delimiter @ ('<' | '\'') => {
                        let end = if delimiter == '<' { '>' } else { '\'' };
                        let mut name = String::new();
                        loop {
                            let member = chars.next()?;
                            if member == end {
                                break;
                            }
                            if !(member.is_ascii_alphabetic()
                                || member == '_'
                                || (!name.is_empty() && member.is_ascii_digit()))
                            {
                                return None;
                            }
                            name.push(member);
                        }
                        if name.is_empty() || !capture_names.insert(name) {
                            return None;
                        }
                        result.push('(');
                    }
                    _ => return None,
                }
                continue;
            }
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
            c if c.is_ascii_punctuation() || c == ' ' => {
                result.push_str(&regex::escape(&c.to_string()));
            }
            _ => return None,
        }
    }
    // Ruby suppresses ordinary captures whenever the expression has a named capture.
    // Copy each segment once: inserting at every offset is quadratic for large patterns.
    if !capture_names.is_empty() {
        let mut rewritten = String::with_capacity(result.len() + ordinary_captures.len() * 2);
        let mut start = 0;
        for position in ordinary_captures {
            rewritten.push_str(&result[start..position]);
            rewritten.push_str("?:");
            start = position;
        }
        rewritten.push_str(&result[start..]);
        result = rewritten;
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
