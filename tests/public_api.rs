use cuke_dedup::analysis::{
    analyze, analyze_with_diagnostics, analyze_with_step_usage, merge_parameter_types,
};
use cuke_dedup::config::{Config, ConfigOverrides, ReporterKind};
use cuke_dedup::discovery::{ExcludeSet, SourceFile, SourceLanguage};
use cuke_dedup::model::SourceLocation;
use cuke_dedup::model::{
    AnalysisResult, EffectStream, FeatureStep, Framework, FrameworkSeed, HandlerFingerprint,
    MatcherKind, RegistrySeed, Rule, Severity, StepDefinition,
};
use cuke_dedup::modes::{BaselineFile, BASELINE_SCHEMA_VERSION};
use cuke_dedup::reporters::{
    render_html, render_json, render_jsonl, render_sarif, write_terminal, ExecutionMetrics,
    ReportContext,
};
use cuke_dedup::ruby::RUBY;
use cuke_dedup::source_adapter::{
    AdapterRegistry, AdapterSessionState, AnalysisPolicy, DependencyGap, DiscoveredFeatures,
    DiscoveryInput, DomainSpec, Extraction, ExtractionDiagnostic, ExtractionDiagnosticLevel,
    FrameworkContract, FrameworkDescriptor, HandlerScope, IdentityFlags, IndirectStepUsage,
    RegexDialect, RegexKey, SourceAdapter, SourceAdapterRegistration, SourceDependency,
    SourceExtractionSession, SourceParameterType, StatefulSourceAdapter, StepMatching,
    StreamEvidence, TranslatedRegex,
};
use cuke_dedup::typescript;
use cuke_dedup::typescript::{JAVASCRIPT, TYPESCRIPT};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The registered adapter for `language`, for tests that drive extraction directly.
fn registered_adapter(language: SourceLanguage) -> &'static dyn SourceAdapter {
    AdapterRegistry::builtin()
        .registrations()
        .iter()
        .map(|registration| registration.adapter)
        .find(|adapter| adapter.language() == language)
        .expect("an adapter is registered for the language")
}

/// Checks external adapters retain session constructors, auto traits, default capabilities, the core parameter-type merge, and legacy extraction delegation.
#[test]
fn extraction_session_preserves_public_constructors_auto_traits_and_adapter_defaults() {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    /// Requires session types to satisfy the public thread-safety, pinning, and unwind-safety trait bounds at compile time.
    fn assert_auto_traits<T: Send + Sync + Unpin + UnwindSafe + RefUnwindSafe>() {}
    assert_auto_traits::<SourceExtractionSession>();

    struct ExternalAdapter;
    impl SourceAdapter for ExternalAdapter {
        /// Returns the external test adapter's stable name.
        fn name(&self) -> &'static str {
            "external"
        }
        /// Identifies the external test adapter as JavaScript.
        fn language(&self) -> SourceLanguage {
            JAVASCRIPT
        }
        /// Returns a sentinel failure to verify default session delegation reaches the external adapter.
        fn extract(&self, _: &str, _: &SourceFile) -> anyhow::Result<Extraction> {
            anyhow::bail!("external adapter reached")
        }
        /// Extraction fails before a comparison.
        fn analysis_policy(&self) -> AnalysisPolicy {
            AnalysisPolicy::unattached()
        }
    }
    let project = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: project.path().join("steps.js"),
        language: JAVASCRIPT,
    };
    let adapter: &dyn SourceAdapter = &ExternalAdapter;
    assert!(adapter.discover_by_default());
    assert!(!adapter.supports_indirect_usage());
    assert_eq!(adapter.inspect_source(&file, &[]), None);
    let configured = BTreeMap::from([("custom".to_owned(), "[a-z]+".to_owned())]);
    assert_eq!(
        merge_parameter_types(Vec::new(), &configured),
        (configured.clone(), Vec::new())
    );
    let declaration = SourceParameterType::new(
        Some("custom".to_owned()),
        Some(".+".to_owned()),
        SourceLocation::new(PathBuf::from("steps.js"), 1, 1, 1, 2),
    );
    let (patterns, diagnostics) = merge_parameter_types(vec![declaration], &configured);
    assert_eq!(patterns["custom"], ".+");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    for mut session in [
        SourceExtractionSession::default(),
        SourceExtractionSession::new(project.path()),
        SourceExtractionSession::with_registrations(project.path(), &["Setup".into()]),
    ] {
        assert_eq!(
            adapter
                .extract_with_session("", &file, &mut session)
                .unwrap_err()
                .to_string(),
            "external adapter reached"
        );
    }
}

/// Maps known reporter variants to names and accommodates future non-exhaustive variants.
fn reporter_name(reporter: ReporterKind) -> &'static str {
    match reporter {
        ReporterKind::Terminal => "terminal",
        ReporterKind::Json => "json",
        ReporterKind::Jsonl => "jsonl",
        ReporterKind::Html => "html",
        ReporterKind::Sarif => "sarif",
        _ => "future reporter",
    }
}

/// Checks diagnostic analysis preserves a definition after a regex limit while strict analysis returns that limit as an error.
#[test]
fn external_callers_can_preserve_partial_results_and_operational_diagnostics() {
    let definitions = typescript::extract(
        "Given(/a{1000000}/, () => work());",
        &SourceFile {
            path: PathBuf::from("steps.ts"),
            language: TYPESCRIPT,
        },
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();

    let outcome = analyze_with_diagnostics(definitions.clone(), Vec::new(), &config).unwrap();
    assert_eq!(outcome.result.definitions.len(), 1);
    assert_eq!(outcome.incomplete.len(), 1);
    assert!(outcome.incomplete[0].contains("regex resource limit"));

    let error = analyze(definitions, Vec::new(), &config).unwrap_err();
    assert!(error.to_string().contains("regex resource limit"));
}

/// Checks sessionless extraction resolves a local registration re-export within the nearest package boundary.
#[test]
fn sessionless_file_extraction_uses_the_nearest_package_boundary() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("support.ts"),
        "export { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();
    let steps = directory.path().join("nested/deeper/steps.ts");
    fs::create_dir_all(steps.parent().unwrap()).unwrap();
    fs::write(
        &steps,
        "import { Given } from '../../support';\nGiven('nested public API step', () => work());\n",
    )
    .unwrap();

    let definitions = typescript::extract_file(&SourceFile {
        path: steps,
        language: TYPESCRIPT,
    })
    .unwrap();

    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].matcher, "nested public API step");
}

/// Checks external callers can construct configuration, metrics, diagnostics, baselines, and adapter registrations through public APIs.
#[test]
fn extensible_public_outputs_use_stable_constructors() {
    let overrides = ConfigOverrides {
        config_file: None,
        definitions: None,
        features: None,
        exclude: None,
        exclude_defaults: None,
        include_hidden: None,
        reporters: Some(vec![ReporterKind::Json]),
        output: None,
        threshold: None,
        require_features: None,
        no_metrics: None,
        rules: BTreeMap::<Rule, Severity>::new(),
    };
    assert_eq!(overrides.reporters, Some(vec![ReporterKind::Json]));

    let metrics = ExecutionMetrics::new(1, 2, 1.0, 2.0, 3.0);
    assert_eq!(metrics.files_discovered, 3);

    let diagnostic = ExtractionDiagnostic::new(
        ExtractionDiagnosticLevel::Warning,
        SourceLocation::new("steps.ts", 1, 1, 1, 2),
        "dynamic matcher",
    );
    let extraction = Extraction::new(Vec::new(), vec![diagnostic]);
    assert_eq!(extraction.diagnostics.len(), 1);

    let baseline = BaselineFile::new(BTreeMap::from([("finding".to_owned(), 1)]));
    assert_eq!(baseline.fingerprints["finding"], 1);

    let default_baseline = BaselineFile::default();
    assert_eq!(default_baseline.schema_version, BASELINE_SCHEMA_VERSION);
    assert!(default_baseline.fingerprints.is_empty());

    let registration = SourceAdapterRegistration::new(
        ".custom",
        AdapterRegistry::builtin().registrations()[0].adapter,
    );
    assert_eq!(registration.suffix, ".custom");

    let all_rules: &[Rule] = Rule::ALL;
    let duplication_rules: &[Rule] = Rule::DUPLICATION_THRESHOLD;
    assert!(all_rules.len() > duplication_rules.len());

    assert_eq!(reporter_name(ReporterKind::Json), "json");
}

/// Checks every public renderer accepts one report context with its configured duplication threshold.
#[test]
fn report_context_is_the_single_public_rendering_entry_point() {
    let root = PathBuf::from("/repo");
    let result = AnalysisResult::new(Vec::new(), Vec::new(), Vec::new());
    let context = ReportContext::new(&result, &root, 5.0);

    assert!(render_json(&context)
        .unwrap()
        .contains("\"threshold\": 5.0"));
    assert!(render_jsonl(&context)
        .unwrap()
        .contains("\"type\":\"summary\""));
    assert!(render_html(&context).unwrap().contains("<!doctype html>"));
    assert!(render_sarif(&context)
        .unwrap()
        .contains("\"version\": \"2.1.0\""));

    let mut terminal = Vec::new();
    write_terminal(&context, &mut terminal).unwrap();
    assert!(String::from_utf8(terminal)
        .unwrap()
        .contains("Analyzed 0 definitions"));
}

/// Checks lowercase global registrations require explicit session configuration before they extract definitions.
#[test]
fn configured_registrations_enable_lowercase_global_step_names() {
    use cuke_dedup::source_adapter::SourceExtractionSession;

    let project = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: project.path().join("steps.ts"),
        language: TYPESCRIPT,
    };
    let adapter = registered_adapter(TYPESCRIPT);
    let source = "given('a lowercase global step', () => work());";

    // Lowercase `given`/`when`/`then` are not ambient globals: a bare call with no import and no
    // configuration registers nothing, so the analyzer never invents a step from an unrelated
    // lowercase helper.
    let mut plain = SourceExtractionSession::new(project.path());
    assert!(adapter
        .extract_with_session(source, &file, &mut plain)
        .unwrap()
        .definitions
        .is_empty());

    // The `registrations` configuration is the documented opt-in: naming `given` there enables it
    // as a bare global registration name.
    let mut configured =
        SourceExtractionSession::with_registrations(project.path(), &["given".into()]);
    assert_eq!(
        adapter
            .extract_with_session(source, &file, &mut configured)
            .unwrap()
            .definitions
            .len(),
        1
    );
}

/// Checks local registration barrels resolve through CommonJS and ESM while missing or malformed bindings remain empty.
#[test]
fn local_commonjs_and_esm_barrel_imports_resolve_registrations_in_process() {
    use cuke_dedup::source_adapter::SourceExtractionSession;

    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("package.json"), "{}").unwrap();
    fs::write(
        project.path().join("barrel.js"),
        "const { Given } = require('@cucumber/cucumber');\nmodule.exports = { Given };\n",
    )
    .unwrap();
    let adapter = registered_adapter(JAVASCRIPT);
    let file = SourceFile {
        path: project.path().join("steps.js"),
        language: JAVASCRIPT,
    };
    let extract = |source: &str| {
        let mut session = SourceExtractionSession::new(project.path());
        adapter
            .extract_with_session(source, &file, &mut session)
            .unwrap()
            .definitions
            .len()
    };

    // A project-local barrel resolves whether imported through CommonJS `require` or an ESM
    // specifier — the two importer paths share one resolution helper.
    assert_eq!(
        extract(
            "const { Given: reg } = require('./barrel');\nreg('cjs a', () => {});\nreg('cjs b', () => {});"
        ),
        2
    );
    assert_eq!(
        extract(
            "import { Given as reg } from './barrel';\nreg('esm a', () => {});\nreg('esm b', () => {});"
        ),
        2
    );
    // A `require` with no specifier, and one of an unresolvable local module, register nothing.
    assert_eq!(extract("const missing = require();\n"), 0);
    assert_eq!(
        extract("const { Given: reg } = require('./nope');\nreg('x', () => {});"),
        0
    );
    // Error-recovered declarations with no name or value register nothing and never panic.
    assert_eq!(extract("const = 5;\nvar ;\nlet x;"), 0);
}

/// Checks a CommonJS local barrel preserves Playwright-BDD factory registration extraction.
#[test]
fn local_barrel_re_exporting_the_bdd_factory_resolves_through_require() {
    use cuke_dedup::source_adapter::SourceExtractionSession;

    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("package.json"), "{}").unwrap();
    // A local barrel re-exporting Playwright-BDD's `createBdd` factory. Destructuring it from a
    // local `require` must enter the factory path, exactly as an ESM import would, or the steps it
    // creates are missed.
    fs::write(
        project.path().join("barrel.js"),
        "const { createBdd } = require('playwright-bdd');\nmodule.exports = { createBdd };\n",
    )
    .unwrap();
    let adapter = registered_adapter(JAVASCRIPT);
    let file = SourceFile {
        path: project.path().join("steps.js"),
        language: JAVASCRIPT,
    };
    let mut session = SourceExtractionSession::new(project.path());
    let definitions = adapter
        .extract_with_session(
            "const { createBdd } = require('./barrel');\nconst { Given } = createBdd();\nGiven('a step', () => {});\nGiven('a step', () => {});",
            &file,
            &mut session,
        )
        .unwrap()
        .definitions;
    assert_eq!(definitions.len(), 2);
}

/// Checks typed incomplete diagnostics retain their kind independently of message wording.
#[test]
fn adapters_can_classify_diagnostics_without_language_specific_messages() {
    use cuke_dedup::source_adapter::ExtractionDiagnosticKind;
    let diagnostic = ExtractionDiagnostic::with_kind(
        ExtractionDiagnosticKind::Incomplete,
        ExtractionDiagnosticLevel::Warning,
        SourceLocation::new("steps.rb", 1, 1, 1, 1),
        "external glue unavailable",
    );
    assert_eq!(diagnostic.kind, ExtractionDiagnosticKind::Incomplete);
    assert_eq!(diagnostic.message, "external glue unavailable");
}

/// Checks Ruby usage metadata enables precise unused findings while legacy analysis preserves conservative incompleteness.
#[test]
fn ruby_usage_metadata_and_legacy_analysis_remain_conservative() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::load(root.path(), ConfigOverrides::default()).unwrap();
    let file = SourceFile {
        path: root.path().join("steps.rb"),
        language: RUBY,
    };
    let extracted = registered_adapter(RUBY).extract(
        "Given('target') { first() }; Then('unused') { second() }; def helper; step 'target'; end", &file).unwrap();
    let features = cuke_dedup::gherkin::extract(
        "Feature: usage\n Scenario: other\n  Given other\n",
        &root.path().join("suite.feature"),
    )
    .unwrap();
    let legacy =
        analyze_with_diagnostics(extracted.definitions.clone(), features.clone(), &config).unwrap();
    assert!(!legacy.incomplete.is_empty());
    assert!(!legacy
        .result
        .findings
        .iter()
        .any(|f| f.rule == Rule::UnusedDefinition));
    let mut usage = extracted.indirect_usage.unwrap();
    let known = cuke_dedup::analysis::analyze_with_step_usage(
        extracted.definitions.clone(),
        features.clone(),
        &config,
        &usage,
    )
    .unwrap();
    let unused: Vec<_> = known
        .result
        .findings
        .iter()
        .filter(|f| f.rule == Rule::UnusedDefinition)
        .collect();
    assert_eq!(unused.len(), 1);
    assert!(unused[0].message.contains("`unused`"));
    assert!(known.incomplete.is_empty());
    usage.extend(None);
    assert!(usage.unknown);
}

/// External callers over Ruby extraction: an over-limit Ruby regex remains in the extracted
/// definitions without a misleading syntax diagnostic. Analysis keeps the neighbouring finding
/// with the regex resource-limit diagnostic, while an unsupported syntax control retains a typed
/// extraction diagnostic. Strict `analyze` rejects the limit, and every public renderer takes the
/// Ruby result through one report context.
#[test]
fn ruby_extraction_flows_through_the_public_diagnostic_analysis_and_report_api() {
    use cuke_dedup::source_adapter::ExtractionDiagnosticKind;
    let root = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: root.path().join("steps.rb"),
        language: RUBY,
    };
    let source = format!(
        "Given('kept') {{ kept }}\nGiven('kept') {{ other }}\n\nGiven(/{}/) {{ work }}\n",
        "a".repeat(1024 * 1024 + 1)
    );
    let extracted = registered_adapter(RUBY).extract(&source, &file).unwrap();
    assert_eq!(extracted.definitions.len(), 3);
    assert!(extracted.diagnostics.is_empty());
    let unsupported = registered_adapter(RUBY)
        .extract("Given(/^a++$/) { work }\n", &file)
        .unwrap();
    let diagnostic = unsupported.diagnostics.first().unwrap();
    assert_eq!(diagnostic.kind, ExtractionDiagnosticKind::Incomplete);
    assert_eq!(diagnostic.level, ExtractionDiagnosticLevel::Warning);
    assert_eq!(
        (diagnostic.location.line, diagnostic.location.column),
        (1, 1)
    );
    assert!(diagnostic
        .message
        .contains("supported static matching subset"));

    let config = Config::load(root.path(), ConfigOverrides::default()).unwrap();
    let outcome =
        analyze_with_diagnostics(extracted.definitions.clone(), Vec::new(), &config).unwrap();
    assert_eq!(outcome.result.definitions.len(), 3);
    assert!(outcome
        .incomplete
        .iter()
        .any(|message| message.contains("steps.rb:4:1 exceeds")
            && message.contains("regex resource limit")));
    assert_eq!(outcome.result.findings.len(), 1);
    assert_eq!(outcome.result.findings[0].rule, Rule::DuplicateMatcher);
    let error = analyze(extracted.definitions, Vec::new(), &config).unwrap_err();
    assert!(error.to_string().contains("regex resource limit"));

    let context = ReportContext::new(&outcome.result, root.path(), 5.0);
    assert!(render_json(&context)
        .unwrap()
        .contains("\"path\": \"steps.rb\""));
    assert!(render_jsonl(&context)
        .unwrap()
        .contains("\"type\":\"summary\""));
    assert!(render_html(&context).unwrap().contains("steps.rb:1:1"));
    assert!(render_sarif(&context)
        .unwrap()
        .contains("\"uri\": \"steps.rb\""));
    let mut terminal = Vec::new();
    write_terminal(&context, &mut terminal).unwrap();
    let terminal = String::from_utf8(terminal).unwrap();
    assert!(terminal.contains("Analyzed 3 definitions"));
    assert!(terminal.contains("steps.rb:1:1"));
}

/// A caller regex dialect whose usage expression, key form and identity flags are fixed per static.
struct CallerDialect {
    expression: Option<&'static str>,
    key: RegexKey,
    identity: IdentityFlags,
}

impl RegexDialect for CallerDialect {
    /// Returns the fixed usage expression as the translation body, ignoring the matcher.
    fn translate(&self, _: &str, _: &str) -> Option<TranslatedRegex> {
        self.expression.map(|body| TranslatedRegex::new("", body))
    }
    /// Returns the fixed key form.
    fn regex_key(&self) -> RegexKey {
        self.key
    }
    /// Returns the fixed identity flags.
    fn identity_flags(&self) -> IdentityFlags {
        self.identity
    }
    /// Requires equal flag strings.
    fn similarity_flags_match(&self, left: &str, right: &str) -> bool {
        left == right
    }
}

static NEVER_MATCHES: CallerDialect = CallerDialect {
    expression: Some("^never$"),
    key: RegexKey::CanonicalSource,
    identity: IdentityFlags::Listed("i"),
};
static UNTRANSLATABLE: CallerDialect = CallerDialect {
    expression: None,
    key: RegexKey::CanonicalSource,
    identity: IdentityFlags::Listed("i"),
};
static UNCOMPILABLE: CallerDialect = CallerDialect {
    expression: Some("(unclosed"),
    key: RegexKey::CanonicalSource,
    identity: IdentityFlags::Listed("i"),
};
static ALL_FLAGS: CallerDialect = CallerDialect {
    expression: None,
    key: RegexKey::CanonicalSource,
    identity: IdentityFlags::All,
};
static TRANSLATED_BODY: CallerDialect = CallerDialect {
    expression: None,
    key: RegexKey::TranslatedBody,
    identity: IdentityFlags::All,
};
static CALLER: DomainSpec = DomainSpec::new("caller", &NEVER_MATCHES).with_display_name("Caller");
// Same label and dialect as `CALLER`, but a different static: a different domain.
static CALLER_TWIN: DomainSpec = DomainSpec::new("caller", &NEVER_MATCHES);
static OPAQUE: DomainSpec = DomainSpec::new("opaque", &UNTRANSLATABLE);
static BROKEN: DomainSpec = DomainSpec::new("broken", &UNCOMPILABLE);
static ALL_FLAGS_DOMAIN: DomainSpec = DomainSpec::new("all-flags", &ALL_FLAGS);
static TRANSLATED_DOMAIN: DomainSpec = DomainSpec::new("translated", &TRANSLATED_BODY);

/// A shared-scope, complete-stream caller policy over `domain`.
const fn caller_policy(domain: &'static DomainSpec, matching: StepMatching) -> AnalysisPolicy {
    AnalysisPolicy::new(
        domain,
        HandlerScope::Shared,
        matching,
        StreamEvidence::Complete,
        false,
    )
}

/// A definition carrying `policy`, keyed by it, with a handler unique to `line`.
fn caller_definition(
    policy: AnalysisPolicy,
    matcher: &str,
    kind: MatcherKind,
    line: usize,
) -> StepDefinition {
    let body = format!("work{line}()");
    StepDefinition {
        matcher: matcher.to_owned(),
        normalized_matcher: policy.normalized_matcher(kind, matcher, "", None),
        matcher_kind: kind,
        matcher_flags: String::new(),
        handler: HandlerFingerprint {
            exact: body.clone(),
            normalized: body.clone(),
            alpha_normalized: body.clone(),
            structural: body.clone(),
            behavior_signature: vec![format!("call:{body}")],
            source_snippet: body,
            comparable: true,
            trivial: false,
            effect_stream: EffectStream::Complete,
            capture_scope: None,
        },
        framework: Framework::new("caller-framework"),
        analysis_policy: policy,
        registration: "Given".to_owned(),
        location: SourceLocation::new(PathBuf::from("steps.caller"), line, 1, line, 2),
        inline_suppressions: Vec::new(),
    }
}

/// The rules of the findings `definitions` and one feature step reading `feature` produce.
fn caller_rules(definitions: Vec<StepDefinition>, feature: &str) -> anyhow::Result<Vec<Rule>> {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let steps = vec![FeatureStep {
        keyword: "Given ".to_owned(),
        text: feature.to_owned(),
        location: SourceLocation::new(PathBuf::from("a.feature"), 3, 5, 3, 20),
    }];
    let result = analyze_with_diagnostics(definitions, steps, &config)?.result;
    Ok(result.findings.iter().map(|finding| finding.rule).collect())
}

/// Checks caller-built policies compare only within their own static domain, drive usage through
/// their dialect and honour their step-matching capability.
#[test]
fn caller_built_policies_separate_domains_and_drive_usage() {
    use MatcherKind::{CucumberExpression, RegularExpression};
    let shared = caller_policy(&CALLER, StepMatching::ByText);
    let pair = |second: AnalysisPolicy| {
        caller_rules(
            vec![
                caller_definition(shared, "I log in", CucumberExpression, 2),
                caller_definition(second, "I log in", CucumberExpression, 4),
            ],
            "I log in",
        )
    };
    assert!(pair(shared).unwrap().contains(&Rule::DuplicateMatcher));
    for (foreign, refused) in [
        (
            registered_adapter(TYPESCRIPT).analysis_policy(),
            "definitions from comparison domains `caller` and `ecmascript`",
        ),
        (
            AnalysisPolicy::unattached(),
            "definitions from comparison domains `caller` and `unattached`",
        ),
        (
            caller_policy(&CALLER_TWIN, StepMatching::ByText),
            "definitions from two comparison domains labelled `caller`",
        ),
    ] {
        let error = pair(foreign).unwrap_err().to_string();
        assert!(
            error.starts_with(refused) && error.contains("separate analysis runs"),
            "{error}"
        );
    }

    // The dialect's expression decides usage: a translation that cannot match leaves the
    // definition unused; no translation makes it non-authoritative.
    let regex = |domain| {
        caller_rules(
            vec![caller_definition(
                caller_policy(domain, StepMatching::ByText),
                "a cat",
                RegularExpression,
                2,
            )],
            "a cat",
        )
        .unwrap()
    };
    assert_eq!(regex(&CALLER), [Rule::UnusedDefinition]);
    assert_eq!(regex(&OPAQUE), []);
    // A translation that does not compile leaves usage indeterminate, as no translation does.
    assert_eq!(regex(&BROKEN), []);

    let unmatched = |matching| {
        caller_rules(
            vec![caller_definition(
                caller_policy(&CALLER, matching),
                "I log out",
                CucumberExpression,
                2,
            )],
            "I log in",
        )
        .unwrap()
    };
    assert_eq!(unmatched(StepMatching::ByText), [Rule::UnusedDefinition]);
    assert_eq!(unmatched(StepMatching::Positional), []);
}

/// Two hand-built definitions under `policy` and the rules they raise against `feature`. Their
/// handlers are identical when `same_handler`; otherwise they differ in structure and overlap in
/// nine of ten events, which only a complete stream can show. `stream` is their evidence regime.
fn hand_built_rules(
    policy: AnalysisPolicy,
    pair: [(&str, &str); 2],
    (kind, same_handler, stream): (MatcherKind, bool, EffectStream),
    feature: &str,
) -> Vec<Rule> {
    let definitions = pair
        .into_iter()
        .enumerate()
        .map(|(index, (matcher, flags))| {
            let line = if same_handler { 2 } else { index + 2 };
            let mut definition = caller_definition(policy, matcher, kind, line);
            definition.normalized_matcher = policy.normalized_matcher(kind, matcher, flags, None);
            definition.matcher_flags = flags.to_owned();
            definition.handler.effect_stream = stream;
            definition
                .handler
                .behavior_signature
                .extend((0..9).map(|event| format!("call:shared{event}()")));
            definition.location.line = index + 2;
            definition.location.end_line = index + 2;
            definition
        })
        .collect();
    let mut rules = caller_rules(definitions, feature).unwrap();
    rules.dedup();
    rules
}

/// Checks an unattached definition stops comparing as JavaScript/TypeScript. Each row's built-in
/// policy is the control: it raises the rule, so a bare `[]` cannot mean "nothing analyzed".
#[test]
fn unattached_definitions_do_not_compare_as_a_builtin_language() {
    use EffectStream::{Complete, Partial};
    use MatcherKind::{CucumberExpression, RegularExpression};
    let unattached = AnalysisPolicy::unattached();
    let ecmascript = registered_adapter(TYPESCRIPT).analysis_policy();
    let shared = (RegularExpression, false, Complete);
    for (case, pair, shape, feature, rule) in [
        // JavaScript's inert `g` separates nothing there; an unknown dialect's flag might.
        (
            "inert flag",
            [("a b", ""), ("a b", "g")],
            shared,
            "x",
            Rule::DuplicateMatcher,
        ),
        // JavaScript's whitespace folding is a dialect rule.
        (
            "folded form",
            [("a b", ""), (r"a\s+b", "")],
            shared,
            "x",
            Rule::NormalizedMatcher,
        ),
        // Wording variants need equal flags.
        (
            "flags differ",
            [("I log in now", "i"), ("I log on now", "")],
            (RegularExpression, true, Complete),
            "x",
            Rule::NearDuplicateStep,
        ),
        // A partial stream cannot show that different handlers behave alike.
        (
            "partial stream",
            [("I log in now", ""), ("I log on now", "")],
            (CucumberExpression, false, Partial),
            "I log in now",
            Rule::NearDuplicateStep,
        ),
    ] {
        assert!(
            hand_built_rules(ecmascript, pair, shape, feature).contains(&rule),
            "control: {case}"
        );
        assert!(
            !hand_built_rules(unattached, pair, shape, feature).contains(&rule),
            "{case}"
        );
    }

    // An unclaimed definition beside extracted ones is refused, not compared as theirs.
    let error = caller_rules(
        vec![
            caller_definition(ecmascript, "I log in", CucumberExpression, 2),
            caller_definition(unattached, "I log in", CucumberExpression, 3),
        ],
        "I log in",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.starts_with("definitions from comparison domains `ecmascript` and `unattached`"),
        "{error}"
    );

    // Read back where no adapter claims it, a stored JS key stops comparing; claimed, it does not.
    let read_back = |path: &str| {
        let definitions = ["a b", r"a\s+b"]
            .into_iter()
            .enumerate()
            .map(|(index, matcher)| {
                let mut definition =
                    caller_definition(ecmascript, matcher, RegularExpression, index + 2);
                definition.framework = Framework::UNKNOWN;
                definition.location.path = PathBuf::from(path);
                serde_json::from_value(serde_json::to_value(definition).unwrap()).unwrap()
            })
            .collect();
        caller_rules(definitions, "x").unwrap()
    };
    assert!(read_back("steps.ts").contains(&Rule::NormalizedMatcher));
    assert!(!read_back("steps").contains(&Rule::NormalizedMatcher));

    // Within the domain, matchers written alike still compare.
    let equal = [("a b", "i"), ("a b", "i")];
    assert!(hand_built_rules(unattached, equal, shared, "x").contains(&Rule::DuplicateMatcher));
    // Equal flags restore the wording variant the flags row refused.
    assert!(hand_built_rules(
        unattached,
        [("I log in now", "i"), ("I log on now", "i")],
        (RegularExpression, true, Complete),
        "x",
    )
    .contains(&Rule::NearDuplicateStep));

    // With indirect usage evidence, no regex translates, so an unmatched regex is not unused; an
    // expression still is.
    let run = |policy, kind, usage: Option<&IndirectStepUsage>| {
        let directory = tempfile::tempdir().unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        let definitions = vec![caller_definition(policy, "a cat", kind, 2)];
        let outcome = match usage {
            Some(usage) => analyze_with_step_usage(definitions, Vec::new(), &config, usage),
            None => analyze_with_diagnostics(definitions, Vec::new(), &config),
        }
        .unwrap();
        let rules: Vec<_> = outcome.result.findings.iter().map(|f| f.rule).collect();
        (rules, outcome.incomplete)
    };
    let known = IndirectStepUsage::default();
    let unused = vec![Rule::UnusedDefinition];
    assert_eq!(
        run(ecmascript, RegularExpression, None),
        (unused.clone(), vec![])
    );
    assert_eq!(
        run(unattached, RegularExpression, Some(&known)),
        (vec![], vec![])
    );
    assert_eq!(
        run(unattached, CucumberExpression, Some(&known)),
        (unused, vec![])
    );
    // Without that evidence nothing is proven unused, and the result says so.
    let unresolved =
        "Unattached indirect step usage is unresolved; unused-definition findings are disabled";
    for kind in [RegularExpression, CucumberExpression] {
        assert_eq!(
            run(unattached, kind, None),
            (vec![], vec![unresolved.to_owned()])
        );
    }
}

/// Checks diagnostics name the caller domain that raised them, with no shared-code edit: its
/// display name, else its label; a declaration with no domain reads as unattached.
#[test]
fn diagnostics_word_the_domain_that_raised_them() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    for (domain, name) in [(&CALLER, "Caller"), (&OPAQUE, "opaque")] {
        let policy = AnalysisPolicy::new(
            domain,
            HandlerScope::Shared,
            StepMatching::ByText,
            StreamEvidence::Complete,
            true,
        );
        let definition = caller_definition(policy, "a cat", MatcherKind::CucumberExpression, 2);
        assert_eq!(
            analyze_with_diagnostics(vec![definition], Vec::new(), &config)
                .unwrap()
                .incomplete,
            [format!(
                "{name} indirect step usage is unresolved; unused-definition findings are disabled"
            )]
        );
    }

    let declaration = SourceParameterType::new(
        Some("size".to_owned()),
        None,
        SourceLocation::new(PathBuf::from("steps.caller"), 4, 1, 4, 2),
    );
    for (declaration, name) in [
        (declaration.clone().with_domain(&CALLER), "Caller"),
        (declaration, "Unattached"),
    ] {
        assert_eq!(
            merge_parameter_types(vec![declaration], &BTreeMap::new()).1,
            [format!(
                "{name} parameter type \"size\" has an unresolved or duplicate declaration at steps.caller:4"
            )]
        );
    }
}

/// Checks caller policies key regex matchers as their dialect declares, translations render with
/// their flag group, contract policies resolve per framework, and policy-carrying types keep their
/// auto traits.
#[test]
fn caller_policies_key_matchers_and_resolve_contract_policies() {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    /// Requires policy-carrying types to satisfy the public thread- and unwind-safety bounds.
    fn assert_auto_traits<T: Send + Sync + UnwindSafe + RefUnwindSafe>() {}
    assert_auto_traits::<AnalysisPolicy>();
    assert_auto_traits::<StepDefinition>();

    let key = |domain, translated: Option<&TranslatedRegex>| {
        caller_policy(domain, StepMatching::ByText).normalized_matcher(
            MatcherKind::RegularExpression,
            "a",
            "gi",
            translated,
        )
    };
    let translation = TranslatedRegex::new("i", "b");
    assert_eq!(key(&CALLER, None), "[regex-flags:i] a");
    assert_eq!(key(&ALL_FLAGS_DOMAIN, None), "[regex-flags:gi] a");
    assert_eq!(key(&CALLER, Some(&translation)), "[regex-flags:i] a");
    assert_eq!(key(&TRANSLATED_DOMAIN, Some(&translation)), "b");
    assert_eq!(key(&TRANSLATED_DOMAIN, None), "a");
    assert_eq!(translation.expression(), "(?i:b)");
    assert_eq!(TranslatedRegex::new("", "b").expression(), "b");

    let local = AnalysisPolicy::new(
        &CALLER,
        HandlerScope::Local,
        StepMatching::Positional,
        StreamEvidence::Declared,
        true,
    );
    let shared = caller_policy(&CALLER, StepMatching::ByText);
    assert_ne!(local, shared);
    let scoped = Framework::new("caller-scoped");
    let plain = Framework::new("caller-plain");
    let contracts = [
        FrameworkContract::new(scoped).with_policy(local),
        FrameworkContract::new(plain),
    ];
    assert_eq!(
        FrameworkContract::policy_of(&contracts, scoped, shared),
        local
    );
    assert_eq!(
        FrameworkContract::policy_of(&contracts, plain, shared),
        shared
    );
    assert_eq!(
        FrameworkContract::policy_of(&contracts, Framework::new("absent"), shared),
        shared
    );
}

/// A caller adapter: each `step: <text>` line of a `.lines` file is a Cucumber Expression in the
/// caller domain.
struct LinesAdapter;

impl SourceAdapter for LinesAdapter {
    /// Returns the adapter name.
    fn name(&self) -> &'static str {
        "caller-lines"
    }
    /// Returns the caller language.
    fn language(&self) -> SourceLanguage {
        SourceLanguage::new("caller-lines")
    }
    /// Extracts one definition per `step:` line.
    fn extract(&self, source: &str, _: &SourceFile) -> anyhow::Result<Extraction> {
        let definitions = source
            .lines()
            .enumerate()
            .filter_map(|(index, line)| Some((index + 1, line.strip_prefix("step: ")?)))
            .map(|(line, text)| {
                caller_definition(
                    self.analysis_policy(),
                    text,
                    MatcherKind::CucumberExpression,
                    line,
                )
            })
            .collect();
        Ok(Extraction::new(definitions, Vec::new()))
    }
    /// Attaches the caller policy.
    fn analysis_policy(&self) -> AnalysisPolicy {
        caller_policy(&CALLER, StepMatching::ByText)
    }
}

static LINES: LinesAdapter = LinesAdapter;

/// Checks a caller registry carries a caller adapter through configuration, discovery, session
/// extraction, finalization and analysis, while the built-in configuration ignores its files.
#[test]
fn caller_registries_route_discovery_extraction_and_analysis() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("steps.lines"),
        "step: I log in\nstep: I log in\n",
    )
    .unwrap();
    let overrides = ConfigOverrides {
        definitions: Some(vec!["*.lines".to_owned()]),
        ..ConfigOverrides::default()
    };
    let builtin = Config::load(root.path(), overrides.clone()).unwrap();
    assert!(cuke_dedup::discovery::discover(&builtin)
        .unwrap()
        .definitions
        .is_empty());

    let registrations = AdapterRegistry::builtin()
        .registrations()
        .iter()
        .copied()
        .chain([SourceAdapterRegistration::new(".lines", &LINES)]);
    let registry = AdapterRegistry::new(registrations).unwrap();
    let config = Config::load_with_adapters(root.path(), overrides, registry).unwrap();
    let files = cuke_dedup::discovery::discover(&config).unwrap();
    assert_eq!(files.definitions.len(), 1);
    let file = &files.definitions[0];
    assert_eq!(file.language, SourceLanguage::new("caller-lines"));
    let mut session = SourceExtractionSession::for_config(&config);
    let adapter = config.adapters().adapter_for_path(&file.path).unwrap();
    let mut definitions = adapter
        .extract_file_with_session(file, &mut session)
        .unwrap()
        .definitions;
    session
        .finalize(&files.definitions, &mut definitions)
        .unwrap();
    assert_eq!(
        SourceExtractionSession::new(root.path())
            .finalize(&files.definitions, &mut Vec::new())
            .unwrap_err()
            .to_string(),
        "no source adapter is registered for language `caller-lines`"
    );
    let outcome = analyze_with_diagnostics(definitions, Vec::new(), &config).unwrap();
    let rules: Vec<_> = outcome.result.findings.iter().map(|f| f.rule).collect();
    assert!(rules.contains(&Rule::DuplicateMatcher), "{rules:?}");
}

/// Session state of [`LoadsAdapter`]: whether the session had a root and how many files it read.
struct LoadsState {
    rooted: bool,
    extracted: usize,
}

impl AdapterSessionState for LoadsState {
    /// Records whether the session has a root.
    fn initialize(root: Option<&Path>, _: &[String], _: &[String]) -> Self {
        Self {
            rooted: root.is_some(),
            extracted: 0,
        }
    }
}

/// A stateful caller adapter: each `load: <path>` line of a `.loads` file loads another source,
/// which its dependency discovery follows unless excluded.
struct LoadsAdapter {
    _id: u8,
}

impl SourceAdapter for LoadsAdapter {
    /// Returns the adapter name.
    fn name(&self) -> &'static str {
        "caller-loads"
    }
    /// Returns the caller language.
    fn language(&self) -> SourceLanguage {
        SourceLanguage::new("caller-loads")
    }
    /// Extracts nothing without a session.
    fn extract(&self, _: &str, _: &SourceFile) -> anyhow::Result<Extraction> {
        Ok(Extraction::new(Vec::new(), Vec::new()))
    }
    /// Counts the file in the session state.
    fn extract_with_session(
        &self,
        _: &str,
        _: &SourceFile,
        session: &mut SourceExtractionSession,
    ) -> anyhow::Result<Extraction> {
        session.state::<LoadsState>()?.extracted += 1;
        Ok(Extraction::new(Vec::new(), Vec::new()))
    }
    /// Compares in the caller domain.
    fn analysis_policy(&self) -> AnalysisPolicy {
        caller_policy(&CALLER, StepMatching::ByText)
    }
}

impl StatefulSourceAdapter for LoadsAdapter {
    type State = LoadsState;
    /// Follows `load:` lines of selected sources, skipping excluded targets and recording a missing
    /// one as a gap.
    fn discover_dependencies(
        config: &Config,
        excludes: &ExcludeSet,
        files: &mut cuke_dedup::discovery::DiscoveredFiles,
    ) {
        let language = SourceLanguage::new("caller-loads");
        let selected: Vec<_> = files
            .definitions
            .iter()
            .filter(|file| file.language == language)
            .map(|file| file.path.clone())
            .collect();
        for path in selected {
            let source = fs::read_to_string(&path).unwrap();
            for (index, line) in source.lines().enumerate() {
                let Some(target) = line.strip_prefix("load: ") else {
                    continue;
                };
                if excludes.excludes(Path::new(target)) {
                    continue;
                }
                let location = SourceLocation::new(path.clone(), index + 1, 1, index + 1, 2);
                let target = config.root.join(target);
                if !target.exists() {
                    files
                        .dependency_gaps
                        .push(DependencyGap::new(location, "target is missing"));
                    continue;
                }
                files
                    .dependencies
                    .push(SourceDependency::new(location, target.clone()));
                files.definitions.push(SourceFile {
                    path: target,
                    language,
                });
            }
        }
    }
}

// Not zero-sized, so the two statics have distinct addresses.
static LOADS: LoadsAdapter = LoadsAdapter { _id: 0 };
// Same behaviour, another object: hooks made for `LOADS` do not belong to it.
static OTHER_LOADS: LoadsAdapter = LoadsAdapter { _id: 1 };

/// Checks a stateful caller adapter gets its session state at construction, its dependency
/// discovery once with the configured excludes, and the discovered edges and gaps as session
/// evidence, whichever of its registrations carries the hooks; a registration without hooks for
/// its adapter gets neither.
#[test]
fn stateful_caller_adapters_keep_session_state_and_dependency_discovery() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("steps.loads"),
        // The self-load adds `steps.loads` again; discovery keeps it once.
        "load: helper.loads\nload: vendor/lib.loads\nload: missing.loads\nload: steps.loads\n",
    )
    .unwrap();
    fs::write(root.path().join("helper.loads"), "").unwrap();
    let overrides = ConfigOverrides {
        definitions: Some(vec!["steps.loads".to_owned()]),
        // A directory glob: only the ancestor check excludes `vendor/lib.loads`.
        exclude: Some(vec!["vendor".to_owned()]),
        ..ConfigOverrides::default()
    };
    let stateful = SourceAdapterRegistration::with_session(".loads", &LOADS);
    let mut replaced = stateful;
    replaced.adapter = &OTHER_LOADS;
    let hookless = SourceAdapterRegistration::new(".loads", &LOADS);
    for (registrations, hooked) in [
        (vec![stateful], true),
        // The hooks sit on a later registration of the same adapter.
        (
            vec![
                hookless,
                SourceAdapterRegistration::with_session(".load2", &LOADS),
            ],
            true,
        ),
        (vec![hookless], false),
        (vec![replaced], false),
    ] {
        let registry = AdapterRegistry::new(registrations).unwrap();
        let config = Config::load_with_adapters(root.path(), overrides.clone(), registry).unwrap();
        let files = cuke_dedup::discovery::discover(&config).unwrap();
        let names: Vec<_> = files
            .definitions
            .iter()
            .map(|file| file.path.file_name().unwrap().to_str().unwrap())
            .collect();
        let mut session = SourceExtractionSession::for_config(&config)
            .with_dependencies(&files.dependencies)
            .with_dependency_gaps(&files.dependency_gaps);
        let state = session.state::<LoadsState>();
        assert_eq!(state.is_ok(), hooked);
        if !hooked {
            assert_eq!(names, ["steps.loads"]);
            assert!(files.dependencies.is_empty() && files.dependency_gaps.is_empty());
            continue;
        }
        assert!(state.unwrap().rooted);
        assert_eq!(files.dependencies.len(), 2);
        // Discovery re-sorts and de-duplicates the definitions the hook appended.
        assert_eq!(names, ["helper.loads", "steps.loads"]);
        for file in &files.definitions {
            LOADS.extract_file_with_session(file, &mut session).unwrap();
        }
        assert_eq!(session.state::<LoadsState>().unwrap().extracted, 2);
        let (edge, gap) = (&files.dependencies[0], &files.dependency_gaps[0]);
        assert_eq!((edge.location.line, gap.location.line), (1, 3));
        assert!(session.dependency_resolved(&edge.location));
        assert!(!session.dependency_resolved(&gap.location));
        assert_eq!(session.dependency_edges(), files.dependencies);
        assert_eq!(session.dependency_gap(&edge.location), None);
        assert_eq!(
            session.dependency_gap(&gap.location),
            Some("target is missing")
        );
    }
}

/// Discovers `caller.bdd`: one feature pattern per line, `?` lines dropped with a warning, an
/// `error` line failing discovery; else the default glob when `package.json` depends on
/// `caller-bdd`.
fn caller_bdd_discovery(input: &DiscoveryInput<'_>) -> anyhow::Result<Option<DiscoveredFeatures>> {
    let config = input.root().join("caller.bdd");
    if config.is_file() {
        let text = input.read(&config)?;
        anyhow::ensure!(
            !text.lines().any(|line| line == "error"),
            "bad caller config"
        );
        let (dynamic, literal): (Vec<_>, Vec<_>) = text.lines().partition(|line| *line == "?");
        let patterns = literal.into_iter().map(str::to_owned).collect();
        return Ok(Some(
            DiscoveredFeatures::new(patterns, config)
                .with_warnings(vec![format!("{} dynamic path(s) skipped", dynamic.len())])
                .with_warnings(vec!["caller config read".to_owned()]),
        ));
    }
    Ok(input.has_dependency("caller-bdd").then(|| {
        DiscoveredFeatures::new(
            vec!["specs/**/*.feature".to_owned()],
            input.root().join("package.json"),
        )
    }))
}

static CALLER_BDD_CONTRACTS: [FrameworkContract; 1] =
    [FrameworkContract::new(Framework::new("caller-bdd"))
        .with_discovery(FrameworkDescriptor::new("Caller BDD", caller_bdd_discovery))];

/// A caller adapter that extracts nothing, declares `contracts` and compares with `policy`. With
/// `nested_seed`, reading its policy starts a seeded read, as an adapter loading its policy lazily
/// might.
struct ContractAdapter {
    name: &'static str,
    contracts: &'static [FrameworkContract],
    policy: AnalysisPolicy,
    nested_seed: bool,
}

impl SourceAdapter for ContractAdapter {
    /// Returns the adapter name.
    fn name(&self) -> &'static str {
        self.name
    }
    /// Returns the adapter name as its language.
    fn language(&self) -> SourceLanguage {
        SourceLanguage::new(self.name)
    }
    /// Extracts nothing.
    fn extract(&self, _: &str, _: &SourceFile) -> anyhow::Result<Extraction> {
        Ok(Extraction::new(Vec::new(), Vec::new()))
    }
    /// Returns the configured policy, after a nested seeded read when asked.
    fn analysis_policy(&self) -> AnalysisPolicy {
        if self.nested_seed {
            use serde::de::DeserializeSeed;
            FrameworkSeed::new(AdapterRegistry::builtin())
                .deserialize(serde_json::json!("unknown"))
                .unwrap();
        }
        self.policy
    }
    /// Declares the configured contracts.
    fn framework_contracts(&self) -> &'static [FrameworkContract] {
        self.contracts
    }
}

/// A caller adapter whose one framework contract discovers feature files.
static BDD: ContractAdapter = ContractAdapter {
    name: "caller-bdd-steps",
    contracts: &CALLER_BDD_CONTRACTS,
    policy: caller_policy(&CALLER, StepMatching::ByText),
    nested_seed: false,
};

/// Checks a caller framework contract's discovery sets normalized feature patterns, their origin
/// and warnings from its configuration file or `package.json`, runs after the built-in contracts,
/// reports its errors as warnings except input-limit errors, and is skipped when the configuration
/// names features or no caller registry is attached.
#[test]
fn caller_framework_contracts_discover_feature_patterns() {
    let registry = AdapterRegistry::new(
        AdapterRegistry::builtin()
            .registrations()
            .iter()
            .copied()
            .chain([SourceAdapterRegistration::new(".bdd", &BDD)]),
    )
    .unwrap();
    let load = |files: &[(&str, &str)], overrides: ConfigOverrides, caller: bool| {
        let root = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = root.path().join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let config = if caller {
            Config::load_with_adapters(root.path(), overrides, registry.clone())
        } else {
            Config::load(root.path(), overrides)
        };
        config.map(|config| {
            (
                config.features,
                config.feature_pattern_origin,
                config.config_warnings,
            )
        })
    };
    let caller_config = [(
        "caller.bdd",
        "./specs/**/*.feature\nspecs\\more\n!specs/skip\n?\n",
    )];
    let (features, origin, warnings) =
        load(&caller_config, ConfigOverrides::default(), true).unwrap();
    assert_eq!(
        features,
        ["specs/**/*.feature", "specs/more/**/*.{feature,feature.md}"]
    );
    assert_eq!(origin, "Caller BDD configuration caller.bdd");
    assert_eq!(
        warnings,
        [
            "1 dynamic path(s) skipped",
            "caller config read",
            "negated framework path `!specs/skip` is not a discovery input; use `exclude` in .cuke-dedup.json",
        ]
    );

    let package = [(
        "package.json",
        r#"{"peerDependencies": {"caller-bdd": "1"}}"#,
    )];
    let (features, origin, _) = load(&package, ConfigOverrides::default(), true).unwrap();
    assert_eq!(features, ["specs/**/*.feature"]);
    assert_eq!(origin, "Caller BDD configuration package.json");

    // A caller path with an extension expands only when it names an existing directory.
    let dotted = [
        ("caller.bdd", "specs.v2\n./absent.v2\n"),
        ("specs.v2/login.feature", ""),
    ];
    let (features, _, _) = load(&dotted, ConfigOverrides::default(), true).unwrap();
    assert_eq!(
        features,
        ["specs.v2/**/*.{feature,feature.md}", "absent.v2"]
    );

    // No caller evidence, a built-in registry and explicit features each leave the caller
    // descriptor out: no caller origin and no caller warning.
    let explicit = ConfigOverrides {
        features: Some(vec!["docs/**/*.feature".to_owned()]),
        ..ConfigOverrides::default()
    };
    for (files, overrides, caller, expected_origin) in [
        (
            &[][..],
            ConfigOverrides::default(),
            true,
            "built-in defaults",
        ),
        (
            &caller_config[..],
            ConfigOverrides::default(),
            false,
            "built-in defaults",
        ),
        (&caller_config[..], explicit, true, "--features"),
    ] {
        let (_, origin, warnings) = load(files, overrides, caller).unwrap();
        assert_eq!(origin, expected_origin, "{caller}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    let (_, origin, warnings) = load(
        &[("caller.bdd", "error\n")],
        ConfigOverrides::default(),
        true,
    )
    .unwrap();
    assert_eq!(origin, "built-in defaults");
    assert_eq!(warnings.len(), 1);
    assert!(
        warnings[0].starts_with("could not inspect framework configuration (bad caller config)"),
        "{warnings:?}"
    );
    let oversized = "x".repeat(1024 * 1024 + 1);
    let error = load(
        &[("caller.bdd", &oversized)],
        ConfigOverrides::default(),
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("caller.bdd"), "{error}");

    // A built-in contract earlier in registry order matches first.
    let both = [
        caller_config[0],
        (
            "package.json",
            r#"{"devDependencies": {"@cucumber/cucumber": "1"}}"#,
        ),
    ];
    let (_, origin, _) = load(&both, ConfigOverrides::default(), true).unwrap();
    assert_eq!(origin, "Cucumber.js configuration package.json");

    // A discovery function is testable on its own: build its input and read its result.
    let root = tempfile::tempdir().unwrap();
    let package = serde_json::json!({"dependencies": {"caller-bdd": "1"}});
    let input = DiscoveryInput::new(root.path(), Some(&package));
    assert_eq!(input.package(), Some(&package));
    let found = caller_bdd_discovery(&input).unwrap().unwrap();
    assert_eq!(found.patterns(), ["specs/**/*.feature"]);
    assert_eq!(found.source(), root.path().join("package.json"));
    assert!(found.warnings().is_empty());
    assert!(
        caller_bdd_discovery(&DiscoveryInput::new(root.path(), None))
            .unwrap()
            .is_none()
    );
}

/// A pair whose first framework reads through a nested built-in seed and whose second reads in
/// the enclosing scope.
struct NestedFrameworks(Framework, Framework);

impl<'de> serde::Deserialize<'de> for NestedFrameworks {
    /// Reads `[inner, outer]`, the inner id through a built-in `RegistrySeed`.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::{DeserializeSeed, Error};
        let [inner, outer]: [serde_json::Value; 2] = serde::Deserialize::deserialize(deserializer)?;
        let inner = RegistrySeed::<Framework>::new(AdapterRegistry::builtin())
            .deserialize(inner)
            .map_err(D::Error::custom)?;
        let outer = serde::Deserialize::deserialize(outer).map_err(D::Error::custom)?;
        Ok(Self(inner, outer))
    }
}

/// A value read on a spawned thread, with the outcome of that read.
struct ReadOnAnotherThread<T>(Result<T, String>);

impl<'de, T: serde::de::DeserializeOwned + Send> serde::Deserialize<'de>
    for ReadOnAnotherThread<T>
{
    /// Reads the input as a JSON value, then deserializes `T` from it on a scoped thread.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value: serde_json::Value = serde::Deserialize::deserialize(deserializer)?;
        let read = std::thread::scope(|scope| {
            scope
                .spawn(|| serde_json::from_value::<T>(value).map_err(|e| e.to_string()))
                .join()
                .unwrap()
        });
        Ok(Self(read))
    }
}

/// A value whose deserialization panics.
struct PanicsOnRead;

impl<'de> serde::Deserialize<'de> for PanicsOnRead {
    /// Panics.
    fn deserialize<D: serde::Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        panic!("deserialization panicked")
    }
}

/// A clone made through the `Clone` bound alone, so the seed's own `Clone` impl runs.
fn cloned<T: Clone>(value: &T) -> T {
    value.clone()
}

/// Checks a caller framework id reads back only through a registry that declares it: the
/// registry lookup, `FrameworkSeed`, and `RegistrySeed` over a definition, a list, an analysis
/// result and a nested seed. A caller-only registry replaces the built-in ids, and plain
/// `Deserialize` reads built-in ids again after a seeded read returns, fails or panics. A framework
/// read on another thread inside a seeded read uses the built-in ids.
#[test]
fn caller_framework_ids_read_back_through_their_registry() {
    use serde::de::DeserializeSeed;
    let builtin = AdapterRegistry::builtin();
    let extended = AdapterRegistry::new(
        builtin
            .registrations()
            .iter()
            .copied()
            .chain([SourceAdapterRegistration::new(".bdd", &BDD)]),
    )
    .unwrap();
    let caller_only = AdapterRegistry::new([SourceAdapterRegistration::new(".bdd", &BDD)]).unwrap();
    let caller = Framework::new("caller-bdd");
    let cucumber = builtin.framework("cucumber-js").expect("built-in id");
    for (registry, id, expected) in [
        (&extended, "caller-bdd", Some(caller)),
        (&extended, "cucumber-js", Some(cucumber)),
        (&extended, "unknown", Some(Framework::UNKNOWN)),
        (&extended, "Caller-BDD", None),
        (builtin, "caller-bdd", None),
        (&caller_only, "cucumber-js", None),
        (&caller_only, "unknown", Some(Framework::UNKNOWN)),
    ] {
        assert_eq!(registry.framework(id), expected, "{id}");
    }

    let framework = |registry, id: &str| {
        FrameworkSeed::new(registry)
            .deserialize(serde_json::json!(id))
            .map_err(|error| error.to_string())
    };
    assert_eq!(framework(&extended, "caller-bdd"), Ok(caller));
    assert_eq!(framework(&caller_only, "unknown"), Ok(Framework::UNKNOWN));
    assert_eq!(
        framework(&caller_only, "cucumber-js"),
        Err("unknown framework `cucumber-js`, expected one of caller-bdd, unknown".to_owned())
    );
    assert!(framework(builtin, "caller-bdd")
        .unwrap_err()
        .starts_with("unknown framework `caller-bdd`, expected one of "));

    let definition = |framework| StepDefinition {
        framework,
        ..caller_definition(
            if framework == cucumber {
                registered_adapter(TYPESCRIPT).analysis_policy()
            } else {
                caller_policy(&CALLER, StepMatching::ByText)
            },
            "I log in",
            MatcherKind::CucumberExpression,
            3,
        )
    };
    let [caller_json, cucumber_json] =
        [caller, cucumber].map(|framework| serde_json::to_value(definition(framework)).unwrap());
    let plain = |json: &serde_json::Value| {
        serde_json::from_value::<StepDefinition>(json.clone()).map_err(|error| error.to_string())
    };
    let rejects_caller = |when: &str| {
        let error = plain(&caller_json).unwrap_err();
        assert!(
            error.contains("unknown framework `caller-bdd`"),
            "{when}: {error}"
        );
        assert_eq!(plain(&cucumber_json), Ok(definition(cucumber)), "{when}");
    };
    rejects_caller("before any seed");

    // One seed reads several values: it is `Copy` and `Clone` whatever `T` is.
    let seed = RegistrySeed::<StepDefinition>::new(&extended);
    let reused = cloned(&seed);
    for (seed, json, framework) in [
        (seed, &caller_json, caller),
        (reused, &cucumber_json, cucumber),
    ] {
        assert_eq!(
            seed.deserialize(json.clone()).unwrap(),
            definition(framework)
        );
    }
    assert_eq!(format!("{seed:?}"), "RegistrySeed { .. }");
    rejects_caller("after a seeded read");
    let list = serde_json::json!([caller_json, cucumber_json]);
    assert_eq!(
        RegistrySeed::<Vec<StepDefinition>>::new(&extended)
            .deserialize(list.clone())
            .unwrap(),
        [definition(caller), definition(cucumber)]
    );
    let result = AnalysisResult::new(vec![definition(caller)], Vec::new(), Vec::new());
    let read = RegistrySeed::<AnalysisResult>::new(&extended)
        .deserialize(serde_json::to_value(&result).unwrap())
        .unwrap();
    assert_eq!(read.definitions, [definition(caller)]);
    assert!(
        serde_json::from_value::<AnalysisResult>(serde_json::to_value(&result).unwrap()).is_err()
    );

    // A caller-only seed admits its own ids and `unknown`, not the built-in ones.
    let error = RegistrySeed::<Vec<StepDefinition>>::new(&caller_only)
        .deserialize(list)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unknown framework `cucumber-js`"), "{error}");
    rejects_caller("after a failed seeded read");

    // The nested built-in seed rejects a caller id; the enclosing scope resumes after it returns.
    let nested = |ids: [&str; 2]| {
        RegistrySeed::<NestedFrameworks>::new(&extended)
            .deserialize(serde_json::json!(ids))
            .map(|NestedFrameworks(inner, outer)| [inner, outer])
            .map_err(|error| error.to_string())
    };
    assert_eq!(
        nested(["cucumber-js", "caller-bdd"]),
        Ok([cucumber, caller])
    );
    assert!(nested(["caller-bdd", "caller-bdd"])
        .unwrap_err()
        .contains("unknown framework `caller-bdd`"));
    rejects_caller("after a nested seeded read");

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RegistrySeed::<PanicsOnRead>::new(&extended).deserialize(serde_json::Value::Null)
    }));
    assert!(panicked.is_err());
    rejects_caller("after a panicking seeded read");

    // The scope belongs to the reading thread: inside a seeded read, a framework read on another
    // thread resolves against the built-in registry.
    let on_another_thread = |id: &str| {
        RegistrySeed::<ReadOnAnotherThread<Framework>>::new(&extended)
            .deserialize(serde_json::json!(id))
            .unwrap()
            .0
    };
    assert_eq!(on_another_thread("cucumber-js"), Ok(cucumber));
    assert!(on_another_thread("caller-bdd")
        .unwrap_err()
        .contains("unknown framework `caller-bdd`"));
}

static POLICY_CONTRACTS: [FrameworkContract; 2] = [
    FrameworkContract::new(Framework::new("policy-scoped"))
        .with_policy(caller_policy(&CALLER, StepMatching::Positional)),
    FrameworkContract::new(Framework::new("policy-plain")),
];

/// A caller adapter with its own policy and one contract that overrides it.
static POLICY: ContractAdapter = ContractAdapter {
    name: "caller-policy-steps",
    contracts: &POLICY_CONTRACTS,
    policy: caller_policy(&CALLER, StepMatching::ByText),
    nested_seed: false,
};

/// Checks a read-back definition gets the policy extraction attaches: its contract's, else the
/// adapter's, where the adapter is the one declaring its framework, else, for `unknown`, the one
/// its path routes to, else none (the default). `RegistrySeed` resolves against its registry, plain
/// `Deserialize` and a read on another thread against the built-in adapters.
#[test]
fn read_back_definitions_get_the_policy_extraction_attaches() {
    use serde::de::DeserializeSeed;
    let registry = AdapterRegistry::new(
        AdapterRegistry::builtin()
            .registrations()
            .iter()
            .copied()
            .chain([SourceAdapterRegistration::new(".pol", &POLICY)]),
    )
    .unwrap();
    let framework = |id: &str| registry.framework(id).expect("declared id");
    let contract = caller_policy(&CALLER, StepMatching::Positional);
    let adapter = POLICY.analysis_policy();
    let ts = registered_adapter(TYPESCRIPT);
    let jest = FrameworkContract::policy_of(
        ts.framework_contracts(),
        framework("jest-cucumber"),
        ts.analysis_policy(),
    );
    let definition = |id: &str, path: &str| {
        let mut definition = caller_definition(
            AnalysisPolicy::unattached(),
            "I log in",
            MatcherKind::CucumberExpression,
            3,
        );
        definition.framework = framework(id);
        definition.location.path = PathBuf::from(path);
        serde_json::to_value(definition).unwrap()
    };
    let seeded = |json: serde_json::Value| {
        RegistrySeed::<StepDefinition>::new(&registry)
            .deserialize(json)
            .unwrap()
            .analysis_policy
    };
    for (id, path, expected) in [
        ("policy-scoped", "steps.pol", contract),
        ("policy-plain", "steps.pol", adapter),
        // The declaring adapter wins over the path's.
        ("policy-plain", "steps.ts", adapter),
        ("cucumber-js", "steps.pol", ts.analysis_policy()),
        ("jest-cucumber", "steps.pol", jest),
        // `unknown` takes the adapter its path routes to, and is unattached when none does.
        ("unknown", "steps.pol", adapter),
        ("unknown", "steps.txt", AnalysisPolicy::unattached()),
    ] {
        assert_eq!(seeded(definition(id, path)), expected, "{id} at {path}");
    }
    assert_ne!(contract, adapter);
    assert_ne!(adapter, AnalysisPolicy::unattached());
    assert_ne!(jest, AnalysisPolicy::unattached());

    // Without the seed, the built-in adapters decide: `.pol` routes nowhere.
    let plain = |json: serde_json::Value| {
        serde_json::from_value::<StepDefinition>(json)
            .unwrap()
            .analysis_policy
    };
    assert_eq!(
        plain(definition("unknown", "steps.pol")),
        AnalysisPolicy::unattached()
    );
    assert_eq!(plain(definition("jest-cucumber", "steps.ts")), jest);
    let elsewhere = RegistrySeed::<ReadOnAnotherThread<StepDefinition>>::new(&registry)
        .deserialize(definition("unknown", "steps.pol"))
        .unwrap()
        .0
        .unwrap();
    assert_eq!(elsewhere.analysis_policy, AnalysisPolicy::unattached());
}

static SHARED_CONTRACTS: [FrameworkContract; 1] =
    [FrameworkContract::new(Framework::new("shared-plain"))];

/// Two caller adapters sharing one contract slice with different policies; reading the first's
/// policy starts a nested seeded read.
static SHARED_BY_TEXT: ContractAdapter = ContractAdapter {
    name: "shared-text",
    contracts: &SHARED_CONTRACTS,
    policy: caller_policy(&CALLER, StepMatching::ByText),
    nested_seed: true,
};

static SHARED_POSITIONAL: ContractAdapter = ContractAdapter {
    name: "shared-positional",
    contracts: &SHARED_CONTRACTS,
    policy: caller_policy(&CALLER, StepMatching::Positional),
    nested_seed: false,
};

/// Checks adapters sharing a contract slice keep their own policies on read-back: the adapter a
/// definition's path routes to wins when it declares the framework, the first declaring adapter
/// otherwise; and an adapter that reads through a seed while the policy is resolved does not
/// conflict with the enclosing seed.
#[test]
fn read_back_policy_prefers_the_routed_adapter_of_a_shared_contract() {
    use serde::de::DeserializeSeed;
    let registry = AdapterRegistry::new([
        SourceAdapterRegistration::new(".text", &SHARED_BY_TEXT),
        SourceAdapterRegistration::new(".positional", &SHARED_POSITIONAL),
    ])
    .unwrap();
    let by_text = caller_policy(&CALLER, StepMatching::ByText);
    let positional = caller_policy(&CALLER, StepMatching::Positional);
    for (path, expected) in [
        ("steps.positional", positional),
        ("steps.text", by_text),
        ("steps.txt", by_text),
    ] {
        let mut definition = caller_definition(
            AnalysisPolicy::unattached(),
            "I log in",
            MatcherKind::CucumberExpression,
            3,
        );
        definition.framework = registry.framework("shared-plain").unwrap();
        definition.location.path = PathBuf::from(path);
        let read = RegistrySeed::<StepDefinition>::new(&registry)
            .deserialize(serde_json::to_value(definition).unwrap())
            .unwrap();
        assert_eq!(read.analysis_policy, expected, "{path}");
    }
}
