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

/// Direct Cucumber-Ruby block registrations, the framework Ruby definitions are attributed to.
pub(crate) const CUCUMBER_RUBY: Framework = Framework::new("cucumber-ruby");
/// Ruby Cucumber sources, selected explicitly by definition globs.
pub const RUBY: SourceLanguage = SourceLanguage::new("ruby");
/// Every framework this adapter attributes definitions to; a named static so its address is unique.
static FRAMEWORK_CONTRACTS: [crate::source_adapter::FrameworkContract; 1] =
    [crate::source_adapter::FrameworkContract::new(CUCUMBER_RUBY)];

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
    /// Expand selected Ruby files through the bounded local dependency resolver.
    fn discover_dependencies(
        config: &crate::config::Config,
        excludes: &globset::GlobSet,
        files: &mut crate::discovery::DiscoveredFiles,
    ) {
        dependencies::resolve(config, excludes, files);
    }
}

impl SourceAdapter for RubyAdapter {
    /// Return the adapter identifier for Cucumber Ruby.
    fn name(&self) -> &'static str {
        "cucumber-ruby"
    }
    /// Identify extracted definitions as Ruby source.
    fn language(&self) -> SourceLanguage {
        RUBY
    }
    /// Extract a standalone file and retain unknown usage when dependency diagnostics remain.
    fn extract(&self, source: &str, file: &SourceFile) -> Result<Extraction> {
        let (mut extraction, _) = extract(source, file)?;
        mark_unresolved_dependency_usage(&mut extraction);
        Ok(extraction)
    }
    /// Require explicit opt-in for experimental Ruby discovery.
    fn discover_by_default(&self) -> bool {
        false
    }
    /// Declares Cucumber-Ruby as the one framework Ruby definitions are attributed to.
    fn framework_contracts(&self) -> &'static [crate::source_adapter::FrameworkContract] {
        &FRAMEWORK_CONTRACTS
    }
    /// Declares the Ruby comparison policy.
    fn analysis_policy(&self) -> crate::source_adapter::AnalysisPolicy {
        semantics::policy()
    }
    /// Enable accounting for Ruby step delegation and unresolved dynamic usage.
    fn supports_indirect_usage(&self) -> bool {
        true
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
            providers::ProofSources::Untrusted => (None, false),
        };
        let edges = session.dependency_edges();
        let state = session.state::<RubySession>()?;
        state.proof_sources_exceeded = exceeded;
        state.prepared = files
            .iter()
            .filter(|file| file.language == RUBY)
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
                UncertaintyScope::Registry(RUBY),
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
                UncertaintyScope::Registry(RUBY),
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
                UncertaintyScope::Registry(RUBY),
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
        let prepared = prepare_nuls(source);
        let source = prepared.as_ref();
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

/// Mark indirect usage unknown when unresolved dependencies may delegate to local steps.
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
    let prepared = prepare_nuls(source);
    extract_with_proof(prepared.as_ref(), file, None, None, false)
}

/// Match Ruby's lexical NUL boundary without executing the source. NUL inside a Ruby comment is
/// ignored; NUL inside literal data is retained; executable interpolation stops the source.
fn prepare_nuls(source: &str) -> std::borrow::Cow<'_, str> {
    if !source.contains('\0') {
        return std::borrow::Cow::Borrowed(source);
    }
    // A space has the same byte width and lets tree-sitter classify comments and quoted data
    // without its special NUL error recovery changing the enclosing lexical node.
    let probe = source.replace('\0', " ");
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .expect("statically linked Ruby grammar is compatible with tree-sitter");
    let tree = parser
        .parse(&probe, None)
        .expect("Ruby parser with no timeout or cancellation returns a tree");
    let mut prepared = source.to_owned();
    for (byte, _) in source.match_indices('\0') {
        let mut node = tree.root_node().descendant_for_byte_range(byte, byte + 1);
        let mut kind = None;
        while let Some(current) = node {
            if matches!(
                current.kind(),
                "comment"
                    | "string"
                    | "delimited_symbol"
                    | "bare_string"
                    | "bare_symbol"
                    | "string_array"
                    | "symbol_array"
                    | "regex"
                    | "heredoc_body"
                    | "interpolation"
            ) {
                kind = Some(current.kind());
                break;
            }
            node = current.parent();
        }
        match kind {
            Some("comment") => prepared.replace_range(byte..byte + 1, " "),
            // tree-sitter rejects NUL inside bare array elements. Keep their one-byte extent and
            // token boundary; array values are opaque to registration extraction.
            Some("bare_string" | "bare_symbol" | "string_array" | "symbol_array") => {
                prepared.replace_range(byte..byte + 1, "x");
            }
            Some("string" | "delimited_symbol" | "regex" | "heredoc_body") => {}
            _ => {
                prepared.truncate(byte);
                break;
            }
        }
    }
    std::borrow::Cow::Owned(prepared)
}

/// Extracts registrations using source-owned provider and assertion evidence.
fn extract_with_proof(
    source: &str,
    file: &SourceFile,
    proof: Option<&providers::Proof>,
    assertions: Option<&assertions::AssertionBindings>,
    finalized: bool,
) -> Result<(Extraction, ownership::RegistrationEffects)> {
    if file.language != RUBY {
        bail!(
            "cucumber-ruby adapter cannot parse `{}` source {}",
            file.language.id(),
            file.path.display()
        );
    }
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_ruby::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .context("Ruby parser produced no tree")?;
    let root = tree.root_node();
    // One scope per file: every handler reads the same file-level bindings.
    let file_scope = bindings::RootScope::new(root, source);
    let syntax_error = syntax_error(root, source);
    let syntax_valid = syntax_error.is_none();
    let proof = proof.filter(|_| syntax_valid);
    let assertions = assertions.filter(|_| syntax_valid);
    let nodes = descendants(root);
    let lines: Vec<_> = source.lines().collect();
    let mut aliases = if syntax_valid {
        registration_aliases::RegistrationAliases::collect(root, source)
    } else {
        Default::default()
    };
    if let Some(proof) = proof {
        aliases.extend_provider(root, proof);
    }
    let wrappers = if syntax_valid {
        registration_wrappers::RegistrationWrappers::collect(root, source, &aliases)
    } else {
        Default::default()
    };
    let mut result = Extraction {
        indirect_usage: Some(indirect_usage(&nodes, source, &aliases, proof)),
        ..Extraction::default()
    };
    if let Some(cause) = syntax_error {
        result.diagnostics.push(diagnostic(
            file,
            cause,
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
            // Any receiver may evaluate to main (`def me = self`, a local rebound through `binding`
            // or `binding_of_caller`), whose DSL registers.
            if registration(name) && may_register(node, source) {
                let message = if is_self_receiver(receiver) {
                    "Ruby self-qualified registration is outside direct-call extraction support"
                } else {
                    "Ruby registration through a receiver is not resolved; definitions may be missing"
                };
                result
                    .diagnostics
                    .push(diagnostic(file, node, source, Kind::Incomplete, message));
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
        if node
            .parent()
            .is_some_and(|parent| parent.kind() != "program")
            && !may_register(node, source)
            && node.child_by_field_name("block").is_none()
        {
            // A call without a possible handler cannot add a step. An attached block still
            // indicates an attempted registration with potentially unsupported arguments.
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
        let translated_regex = if matcher_kind == MatcherKind::RegularExpression {
            match regex_translation(&matcher, &flags) {
                Ok(translated) => Some(translated),
                Err(RegexConversionError::Unsupported) => {
                    result.diagnostics.push(diagnostic(
                        file,
                        node,
                        source,
                        Kind::Incomplete,
                        "Ruby regular expression is outside the supported static matching subset",
                    ));
                    None
                }
                Err(RegexConversionError::CompiledTooBig) => {
                    result.diagnostics.push(diagnostic(
                        file,
                        node,
                        source,
                        Kind::Incomplete,
                        "Ruby regular expression exceeds the regex resource limit",
                    ));
                    None
                }
                // Shared matcher compilation reports the raw-pattern limit at its location.
                Err(RegexConversionError::PatternTooLong) => None,
            }
        } else {
            None
        };
        let handler = proof
            .and_then(|p| p.handlers.get(&node.start_byte()))
            .cloned()
            .unwrap_or_else(|| handler::fingerprint(block, &file_scope, source, assertions));
        if !handler.comparable
            && !proof.is_some_and(|proof| proof.comparison_rejections.contains(&node.start_byte()))
            && !assertions.is_some_and(|assertions| {
                assertions.rejected_comparison(block, &file_scope, source)
            })
        {
            result.diagnostics.push(diagnostic(
                file,
                block,
                source,
                Kind::Incomplete,
                "Ruby handler comparison has unresolved syntax, captures or receiver identity",
            ));
        }
        // Translation preserves Ruby capture arity. Flags have their own comparison partition,
        // so the key carries the translated body without its flag group.
        let analysis_policy = crate::source_adapter::FrameworkContract::policy_of(
            &FRAMEWORK_CONTRACTS,
            CUCUMBER_RUBY,
            semantics::policy(),
        );
        let comparison = analysis_policy.normalized_matcher(
            matcher_kind,
            &matcher,
            &flags,
            translated_regex.as_ref(),
        );
        result.definitions.push(StepDefinition {
            normalized_matcher: comparison,
            matcher,
            matcher_kind,
            matcher_flags: flags,
            handler,
            framework: CUCUMBER_RUBY,
            analysis_policy,
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

/// What an argument evaluates to, as far as Cucumber-Ruby's registrar can tell: a step pattern
/// (String or Regexp), a handler (Symbol or Proc), `nil`/`false`, another known type, or a value
/// only runtime decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArgumentValue {
    Pattern,
    Handler,
    Falsy,
    Other,
    Unknown,
}

/// The value class of one argument node. Every `argument_list` expression kind of tree-sitter-ruby
/// not listed here is `Unknown`; the listed classes were checked with `ruby -e`.
fn argument_value(node: Node<'_>, source: &str) -> ArgumentValue {
    match node.kind() {
        // `(a; b)` is the last statement, `()` is nil.
        "parenthesized_statements" => bindings::semantic_children(node)
            .last()
            .map_or(ArgumentValue::Falsy, |last| argument_value(*last, source)),
        "assignment" => node
            .child_by_field_name("right")
            .map_or(ArgumentValue::Unknown, |right| {
                argument_value(right, source)
            }),
        "string" | "chained_string" | "character" | "regex" => ArgumentValue::Pattern,
        // A backtick heredoc calls the redefinable backtick method.
        "heredoc_beginning" if !text(node, source).contains('`') => ArgumentValue::Pattern,
        // `def` returns its name as a Symbol.
        "simple_symbol" | "delimited_symbol" | "lambda" | "method" | "singleton_method" => {
            ArgumentValue::Handler
        }
        "nil" | "false" | "test_pattern" => ArgumentValue::Falsy,
        "true" | "integer" | "float" | "rational" | "complex" | "array" | "string_array"
        | "symbol_array" | "hash" | "range" => ArgumentValue::Other,
        // A sign written directly before a number is part of the literal; `- 1` is a call.
        "unary"
            if node.child_by_field_name("operand").is_some_and(|operand| {
                matches!(operand.kind(), "integer" | "float" | "rational" | "complex")
                    && node.child(0).is_some_and(|sign| {
                        matches!(sign.kind(), "-" | "+") && sign.end_byte() == operand.start_byte()
                    })
            }) =>
        {
            ArgumentValue::Other
        }
        // tree-sitter parses these reserved keywords as identifiers; Ruby never binds them.
        "identifier" => match text(node, source) {
            "__FILE__" => ArgumentValue::Pattern,
            "__LINE__" | "__ENCODING__" => ArgumentValue::Other,
            _ => ArgumentValue::Unknown, // fail-closed: a local or method may hold anything.
        },
        _ => ArgumentValue::Unknown, // fail-closed: runtime decides the value.
    }
}

/// Whether a `**` operand is a literal empty Hash (`Some(true)`), a literal non-empty one
/// (`Some(false)`), or unknown. Parentheses are transparent, and a literal Hash is non-empty when it
/// has a pair or a non-empty nested `**` literal. `**nil` passes nothing, which "unknown" already
/// allows.
fn literal_hash_empty(node: Node<'_>) -> Option<bool> {
    let node = bindings::unparenthesized(node);
    match node.kind() {
        "hash" => {
            let mut empty = Some(true);
            for entry in bindings::semantic_children(node) {
                empty = if entry.kind() == "hash_splat_argument" {
                    either_non_empty(empty, entry.named_child(0).and_then(literal_hash_empty))
                } else {
                    Some(false) // a pair
                };
            }
            empty
        }
        _ => None, // fail-closed: runtime decides whether the Hash is empty.
    }
}

/// Whether an expression is `nil` by its syntax: `nil`, `()`, or parentheses whose last statement
/// is one of those (`(1; nil)`).
fn evaluates_to_nil(node: Node<'_>) -> bool {
    match node.kind() {
        "nil" => true,
        "parenthesized_statements" => bindings::semantic_children(node)
            .last()
            .is_none_or(|last| evaluates_to_nil(*last)),
        _ => false, // fail-closed: any other operand may be non-nil, so it may pass a block.
    }
}

/// Combines two emptiness answers: non-empty if either is, unknown if either is, else empty.
fn either_non_empty(left: Option<bool>, right: Option<bool>) -> Option<bool> {
    match (left, right) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (Some(true), Some(true)) => Some(true),
        _ => None, // fail-closed: either side may be non-empty at runtime.
    }
}

/// One positional argument: a known value, zero or one known value, or a splat or `...`
/// supplying any number of values.
#[derive(Clone, Copy)]
enum Positional {
    Fixed(ArgumentValue),
    /// Zero or one value of a known class, as an unknown `**hash` entry adds nothing or a Hash.
    Optional(ArgumentValue),
    Variadic,
}

/// Whether a step-keyword call can register. Cucumber-Ruby keywords take
/// `(pattern, symbol = nil, options = {}, &proc)`: the pattern must be a String or Regexp, and
/// `symbol || proc` must be a Symbol or a Proc. Keyword pairs and a non-empty `**hash` are one
/// trailing Hash argument; each splat or `...` supplies any number of arguments, so every
/// argument list within three arguments is tried, and the call can register when one of them
/// has a possible pattern and either a possible handler second or, with that argument absent,
/// `nil` or `false`, a possible block.
fn may_register(call: Node<'_>, source: &str) -> bool {
    let arguments = call
        .child_by_field_name("arguments")
        .map(bindings::semantic_children)
        .unwrap_or_default();
    let mut positional = Vec::new();
    // `None`: no keyword Hash; `Some(false)`: maybe one; `Some(true)`: always one.
    let mut keywords: Option<bool> = None;
    let mut block = call.child_by_field_name("block").is_some();
    for argument in &arguments {
        match argument.kind() {
            "pair" => keywords = Some(true),
            "hash_splat_argument" => match argument.named_child(0).and_then(literal_hash_empty) {
                Some(true) => {}
                Some(false) => keywords = Some(true),
                None => keywords = Some(keywords.unwrap_or(false)),
            },
            "block_argument" => {
                block |= argument
                    .named_child(0)
                    .is_none_or(|value| !evaluates_to_nil(value));
            }
            "forward_argument" => {
                block = true;
                keywords = Some(keywords.unwrap_or(false));
                positional.push(Positional::Variadic);
            }
            "splat_argument" => match argument.named_child(0).map(bindings::unparenthesized) {
                Some(value) if evaluates_to_nil(value) => {}
                // A literal array supplies its elements; its keyword pairs are one trailing Hash.
                Some(value) if value.kind() == "array" => {
                    let mut hash = Some(true);
                    for element in bindings::semantic_children(value) {
                        match element.kind() {
                            "pair" => hash = Some(false),
                            "hash_splat_argument" => {
                                hash = either_non_empty(
                                    hash,
                                    element.named_child(0).and_then(literal_hash_empty),
                                );
                            }
                            "splat_argument" => positional.push(Positional::Variadic),
                            _ => {
                                positional.push(Positional::Fixed(argument_value(element, source)))
                            }
                        }
                    }
                    match hash {
                        Some(true) => {}
                        Some(false) => positional.push(Positional::Fixed(ArgumentValue::Other)),
                        // A Hash that may be empty is zero or one more argument, never another value.
                        None => positional.push(Positional::Optional(ArgumentValue::Other)),
                    }
                }
                _ => positional.push(Positional::Variadic),
            },
            _ => positional.push(Positional::Fixed(argument_value(*argument, source))),
        }
    }
    // Distinct argument lists within the three-argument cap; `None` is a value a splat supplied.
    let mut expansions: Vec<Vec<Option<ArgumentValue>>> = vec![Vec::new()];
    for item in &positional {
        let mut next: Vec<Vec<Option<ArgumentValue>>> = Vec::new();
        for prefix in &expansions {
            let choices: Vec<Vec<Option<ArgumentValue>>> = match item {
                Positional::Fixed(value) => vec![vec![Some(*value)]],
                Positional::Optional(value) => vec![Vec::new(), vec![Some(*value)]],
                Positional::Variadic => (0..=3).map(|width| vec![None; width]).collect(),
            };
            for choice in choices {
                let mut slots = prefix.clone();
                slots.extend(choice);
                if slots.len() <= 3 && !next.contains(&slots) {
                    next.push(slots);
                }
            }
        }
        expansions = next;
    }
    let hashes: &[bool] = match keywords {
        None => &[false],
        Some(false) => &[false, true],
        Some(true) => &[true],
    };
    expansions.iter().any(|slots| {
        hashes.iter().any(|&hash| {
            let mut slots = slots.clone();
            if hash {
                slots.push(Some(ArgumentValue::Other));
            }
            let pattern = slots.first().is_some_and(|first| {
                matches!(
                    first,
                    None | Some(ArgumentValue::Pattern | ArgumentValue::Unknown)
                )
            });
            let proc = match slots.get(1) {
                Some(None | Some(ArgumentValue::Handler | ArgumentValue::Unknown)) => true,
                Some(Some(ArgumentValue::Pattern | ArgumentValue::Other)) => false,
                // `nil`, `false` or absent: the block is the handler.
                Some(Some(ArgumentValue::Falsy)) | None => block,
            };
            slots.len() <= 3 && pattern && proc
        })
    })
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

/// Recognize self after unwrapping the final value of parenthesized statements.
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

/// Yield call nodes paired with their source-spelled method names.
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

/// Collect literal self step calls and mark unresolved delegation or reflection as unknown.
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

/// Decode a simple symbol or non-interpolated string into a method name.
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

/// Recognize calls that define, alias, or remove methods.
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

/// Detect calls that may replace protected DSL methods, treating unknown dispatch conservatively.
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

/// Recognize the internal registrar and supported localized step keywords.
fn registration(name: &str) -> bool {
    name == "register_rb_step_definition"
        || step_keywords::STEP_KEYWORDS.binary_search(&name).is_ok()
}

/// Borrow the source slice covered by a syntax node.
fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

/// Collect the root and its named descendants with an iterative traversal.
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

/// Returns the first syntax boundary Ruby rejects but tree-sitter may recover through.
fn syntax_error<'a>(root: Node<'a>, source: &str) -> Option<Node<'a>> {
    let mut nodes = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        nodes.push(node);
        let mut cursor = node.walk();
        pending.extend(node.children(&mut cursor));
    }
    let supplemental: Vec<_> = nodes
        .iter()
        .copied()
        .filter(|node| {
            (node.kind() == "identifier"
                && node.parent() == Some(root)
                && text(*node, source) == "end")
                || (node.kind() == "escape_sequence" && invalid_unicode_escape(text(*node, source)))
        })
        .collect();
    nodes
        .into_iter()
        .filter(|node| node.is_error() || node.is_missing())
        .chain(supplemental.iter().copied())
        .min_by_key(|node| (node.start_byte(), node.end_byte()))
}

/// Rejects Unicode escapes whose contents are not Ruby Unicode scalar values.
fn invalid_unicode_escape(raw: &str) -> bool {
    let Some(raw) = raw.strip_prefix("\\u") else {
        return false;
    };
    let mut chars = raw.chars().peekable();
    unicode_escape(&mut chars).is_none() || chars.next().is_some()
}

/// Extract a literal matcher and inline or resolved block handler from a supported registration.
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

/// Split a `/…/` or `%r<delimiter>…` Ruby regex literal into a pattern and sorted unique flags.
///
/// Bracket delimiters `() [] {} <>` nest by depth, any other ASCII punctuation delimiter ends at
/// its next occurrence, and a backslash escapes the following character. As in Ruby, the backslash
/// of an escaped non-bracket delimiter is dropped unless that character is regex syntax
/// (`$ * + . ? ^ | ) ] } >`), so `%r-[a\-z]-` is the range `[a-z]`. A `\` delimiter never closes
/// because the escape wins, so it stays unrecognized.
fn regex_literal(raw: &str) -> Option<(String, String)> {
    let (open, body) = raw.strip_prefix('/').map(|body| ('/', body)).or_else(|| {
        let rest = raw.strip_prefix("%r")?;
        let open = rest.chars().next().filter(char::is_ascii_punctuation)?;
        Some((open, &rest[open.len_utf8()..]))
    })?;
    let close = match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '<' => '>',
        other => other,
    };
    let nests = open != close;
    let drops_escape = !nests && !"$*+.?^|)]}>".contains(close);
    let (mut pattern, mut depth, mut chars) = (String::new(), 0usize, body.char_indices());
    while let Some((index, ch)) = chars.next() {
        if ch == '\\' {
            let (_, escaped) = chars.next()?;
            if !(drops_escape && escaped == close) {
                pattern.push(ch);
            }
            pattern.push(escaped);
        } else if ch == close && depth == 0 {
            let flags = &body[index + close.len_utf8()..];
            let flags = flags.chars().collect::<std::collections::BTreeSet<_>>();
            return Some((pattern, flags.into_iter().collect()));
        } else {
            if nests && ch == close {
                depth -= 1;
            } else if nests && ch == open {
                depth += 1;
            }
            pattern.push(ch);
        }
    }
    None
}

/// Decode a plain `'…'` or `"…"` literal to the bytes Ruby builds; None for other literal forms, malformed or unmodeled escapes, and values that are not valid UTF-8.
fn literal_string(raw: &str) -> Option<String> {
    let quote = raw.chars().next()?;
    if raw.len() < 2 || !matches!(quote, '\'' | '"') || !raw.ends_with(quote) {
        return None;
    }
    // Ruby's lexer reads a source CRLF as LF, including after a backslash.
    let mut body = std::borrow::Cow::Borrowed(&raw[1..raw.len() - 1]);
    if body.contains("\r\n") {
        body = body.replace("\r\n", "\n").into();
    }
    let mut chars = body.chars().peekable();
    let mut value = Vec::new();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            push_char(&mut value, ch);
            continue;
        }
        let escaped = chars.next()?;
        if quote == '"' {
            double_quoted_escape(escaped, &mut chars, &mut value)?;
        } else {
            if !matches!(escaped, '\\' | '\'') {
                value.push(b'\\');
            }
            push_char(&mut value, escaped);
        }
    }
    String::from_utf8(value).ok()
}

/// Append the UTF-8 bytes of one character.
fn push_char(value: &mut Vec<u8>, ch: char) {
    value.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
}

/// Append the bytes of one double-quoted escape, whose backslash is consumed; None when unmodeled or malformed.
fn double_quoted_escape(
    escaped: char,
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    value: &mut Vec<u8>,
) -> Option<()> {
    let byte = match escaped {
        // Backslash-newline continues the line and adds nothing.
        '\n' => return Some(()),
        'n' => b'\n',
        'r' => b'\r',
        't' => b'\t',
        'a' => 0x07,
        'b' => 0x08,
        'e' => 0x1b,
        'f' => 0x0c,
        's' => b' ',
        'v' => 0x0b,
        'u' => {
            value.extend_from_slice(unicode_escape(chars)?.as_bytes());
            return Some(());
        }
        // Up to three octal digits; Ruby keeps the low eight bits (`"\400"` is NUL).
        '0'..='7' => {
            let mut code = escaped as u32 - '0' as u32;
            for digit in std::iter::from_fn(|| chars.next_if(|c| matches!(c, '0'..='7'))).take(2) {
                code = code * 8 + (digit as u32 - '0' as u32);
            }
            (code & 0xff) as u8
        }
        // One or two hex digits; none is a Ruby syntax error.
        'x' => {
            let digits: String = std::iter::from_fn(|| chars.next_if(char::is_ascii_hexdigit))
                .take(2)
                .collect();
            u8::from_str_radix(&digits, 16).ok()?
        }
        // fail-closed: control and meta escapes (`\cX`, `\C-X`, `\M-X`, `\M-\C-X`) build bytes by
        // bit arithmetic over a following escape; they stay undecoded and the matcher indeterminate.
        'c' | 'C' | 'M' => return None,
        other => {
            // Any other escape yields the escaped character itself.
            push_char(value, other);
            return Some(());
        }
    };
    value.push(byte);
    Some(())
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
    regex_translation(pattern, flags)
        .ok()
        .map(|translated| translated.expression())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegexConversionError {
    PatternTooLong,
    CompiledTooBig,
    Unsupported,
}

/// Translate the supported Ruby regex subset and compile-check it under shared resource limits.
fn regex_translation(
    pattern: &str,
    flags: &str,
) -> Result<crate::matcher::TranslatedRegex, RegexConversionError> {
    if pattern.len() > crate::resource_limits::MAX_REGEX_PATTERN_BYTES {
        return Err(RegexConversionError::PatternTooLong);
    }
    if flags
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
        return Err(RegexConversionError::Unsupported);
    }
    let mut result = String::from(if flags.contains('m') { "(?ms:" } else { "(?m:" });
    if flags.contains('i') {
        result.insert(2, 'i');
    }
    let group_prefix_len = result.len();
    let mut chars = pattern.chars().peekable();
    let mut class_depth = 0;
    let mut first_class_member = false;
    let mut class_negated = false;
    let mut capture_names = std::collections::BTreeSet::new();
    let mut ordinary_captures = Vec::new();
    while let Some(ch) = chars.next() {
        let in_class = class_depth > 0;
        if ch == '{' && !in_class {
            if let Some(bound) =
                repetition_bound(&mut chars).ok_or(RegexConversionError::Unsupported)?
            {
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
                match chars.next().ok_or(RegexConversionError::Unsupported)? {
                    ':' => result.push_str("(?:"),
                    'm' if chars.next() == Some(':') => result.push_str("(?s:"),
                    delimiter @ ('<' | '\'') => {
                        let end = if delimiter == '<' { '>' } else { '\'' };
                        let mut name = String::new();
                        loop {
                            let member = chars.next().ok_or(RegexConversionError::Unsupported)?;
                            if member == end {
                                break;
                            }
                            if !(member.is_ascii_alphabetic()
                                || member == '_'
                                || (!name.is_empty() && member.is_ascii_digit()))
                            {
                                return Err(RegexConversionError::Unsupported);
                            }
                            name.push(member);
                        }
                        if name.is_empty() || !capture_names.insert(name) {
                            return Err(RegexConversionError::Unsupported);
                        }
                        result.push('(');
                    }
                    _ => return Err(RegexConversionError::Unsupported),
                }
                continue;
            }
        }
        if ch != '\\' {
            result.push(ch);
            continue;
        }
        let escaped = chars.next().ok_or(RegexConversionError::Unsupported)?;
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
            _ => return Err(RegexConversionError::Unsupported),
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
    match crate::resource_limits::compile_regex(&result) {
        // `(?` + flags + `:` is the group prefix and the final `)` closes it.
        Ok(_) => Ok(crate::matcher::TranslatedRegex::new(
            &result[2..group_prefix_len - 1],
            &result[group_prefix_len..result.len() - 1],
        )),
        Err(regex::Error::CompiledTooBig(_)) => Err(RegexConversionError::CompiledTooBig),
        Err(_) => Err(RegexConversionError::Unsupported),
    }
}

// Consume only Ruby repetition syntax. Other braces are literal; excessively long numeric
// bounds stay unsupported rather than spending unbounded work or treating a bound as text.
/// Parse a bounded repetition after an opening brace; distinguish literal braces from overlong bounds.
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

/// Convert node byte offsets to one-based lines and Unicode character columns.
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

/// Create an extraction warning at the syntax node location.
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
