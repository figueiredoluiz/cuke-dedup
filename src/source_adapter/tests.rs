use super::*;
use crate::typescript::{JAVASCRIPT, TSX, TYPESCRIPT};
use std::collections::BTreeSet;
use std::fs;

/// Creates a temporary project with a local Cucumber registration re-export.
fn session_project() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::write(
        directory.path().join("support.ts"),
        "export { Given } from '@cucumber/cucumber';",
    )
    .unwrap();
    directory
}

/// Extracts a synthetic source through its suffix-selected adapter and supplied session, panicking on failure.
fn session_extract(
    session: &mut SourceExtractionSession,
    root: &Path,
    suffix: &str,
    source: &str,
) -> Extraction {
    let path = root.join(format!("steps.{suffix}"));
    let adapter = adapter_for_path(&path).unwrap();
    adapter
        .extract_with_session(
            source,
            &SourceFile {
                path,
                language: adapter.language(),
            },
            session,
        )
        .unwrap()
}

const IMPORTED_STEP: &str =
    "import { Given } from './support'; Given('shared step', () => work());";

/// Checks backend state initializes once per session and invalid or missing state returns an error.
#[test]
fn session_state_initializes_once_per_backend_and_per_run() {
    struct Counter(usize);
    impl AdapterSessionState for Counter {
        /// Creates fresh state for the test backend without using project configuration.
        fn initialize(_: Option<&Path>, _: &[String], _: &[String]) -> Self {
            Self(0)
        }
    }
    struct Other;
    impl AdapterSessionState for Other {
        /// Creates fresh state for the test backend without using project configuration.
        fn initialize(_: Option<&Path>, _: &[String], _: &[String]) -> Self {
            Self
        }
    }
    let mut session = SourceExtractionSession::default();
    session.state::<Counter>().unwrap().0 = 7;
    session.initialize::<Counter>();
    session.state::<Other>().unwrap();
    assert_eq!(session.states.len(), 2);
    assert_eq!(session.state::<Counter>().unwrap().0, 7);
    assert_eq!(
        SourceExtractionSession::default()
            .state::<Counter>()
            .unwrap()
            .0,
        0
    );
    let project = tempfile::tempdir().unwrap();
    let mut explicit = SourceExtractionSession::new(project.path());
    assert!(explicit.state::<Counter>().is_err());
    assert!(!explicit.states.contains_key(&TypeId::of::<Counter>()));
    // Even a violated internal key/value invariant must return an error, not panic.
    session
        .states
        .insert(TypeId::of::<Counter>(), Box::new(Other));
    assert!(session.state::<Counter>().is_err());
}

/// Checks JS, TS, and TSX share a resolver snapshot and retain duplicate findings in either extraction order.
#[test]
fn session_mixed_languages_share_cache_and_preserve_findings_in_either_order() {
    for suffixes in [["js", "ts", "tsx"], ["tsx", "ts", "js"]] {
        let project = session_project();
        let root = project.path();
        let mut session = SourceExtractionSession::new(root);
        assert_eq!(session.states.len(), 2);
        let mut definitions = Vec::new();
        for suffix in suffixes {
            let extracted = session_extract(&mut session, root, suffix, IMPORTED_STEP);
            assert_eq!(extracted.definitions.len(), 1, "{suffix}");
            assert!(extracted.diagnostics.is_empty(), "{suffix}");
            // A second resolver would observe this edit; a shared run keeps cached exports.
            fs::write(
                root.join("support.ts"),
                "export { When } from '@cucumber/cucumber';",
            )
            .unwrap();
            definitions.extend(extracted.definitions);
        }
        fs::write(
            root.join("support.ts"),
            "export { Given } from '@cucumber/cucumber';",
        )
        .unwrap();
        let mut expected = Vec::new();
        for suffix in suffixes {
            let extracted = session_extract(
                &mut SourceExtractionSession::default(),
                root,
                suffix,
                IMPORTED_STEP,
            );
            assert!(extracted.diagnostics.is_empty());
            expected.extend(extracted.definitions);
        }
        assert_eq!(definitions, expected);
        let config = crate::config::Config::load(root, Default::default()).unwrap();
        let result = crate::analysis::analyze(definitions, vec![], &config).unwrap();
        assert_eq!(
            result
                .findings
                .iter()
                .filter(|finding| finding.rule == crate::model::Rule::DuplicateMatcher)
                .count(),
            2
        );
    }
}

/// Checks a session retains its failed-root fallback after that root is created, while a new session observes it.
#[test]
fn session_keeps_failed_root_resolution_snapshot_and_default_fallback() {
    let project = session_project();
    let missing = project.path().join("initially-missing");
    let mut session = SourceExtractionSession::new(&missing);
    fs::create_dir(&missing).unwrap();
    let actual = session_extract(&mut session, project.path(), "ts", IMPORTED_STEP);
    let expected = session_extract(
        &mut SourceExtractionSession::default(),
        project.path(),
        "ts",
        IMPORTED_STEP,
    );
    assert_eq!(actual, expected);
    assert_eq!(actual.definitions.len(), 1);
    let rejected = session_extract(
        &mut SourceExtractionSession::new(&missing),
        project.path(),
        "ts",
        IMPORTED_STEP,
    );
    assert!(rejected.definitions.is_empty());
    assert!(!rejected.diagnostics.is_empty());
}

/// Checks an existing session retains its canonical project root after a symlink is retargeted.
#[cfg(unix)]
#[test]
fn session_keeps_valid_root_snapshot_when_symlink_is_retargeted() {
    let original = session_project();
    let other = session_project();
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("root");
    std::os::unix::fs::symlink(original.path(), &link).unwrap();
    let mut session = SourceExtractionSession::new(&link);
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(other.path(), &link).unwrap();
    let actual = session_extract(&mut session, original.path(), "ts", IMPORTED_STEP);
    assert_eq!(actual.definitions.len(), 1);
    assert!(actual.diagnostics.is_empty());
    let rejected = session_extract(
        &mut SourceExtractionSession::new(&link),
        original.path(),
        "ts",
        IMPORTED_STEP,
    );
    assert!(rejected.definitions.is_empty());
    assert!(!rejected.diagnostics.is_empty());
}

/// Checks separate project sessions isolate custom registration names and local export caches.
#[test]
fn session_roots_and_configured_registrations_are_isolated() {
    let project = session_project();
    let other = session_project();
    let mut first = SourceExtractionSession::with_registrations(project.path(), &["SetupA".into()]);
    let mut second = SourceExtractionSession::with_registrations(other.path(), &["SetupB".into()]);
    let source = "SetupA('custom', () => work()); SetupB('custom', () => work());";
    for (session, root, name) in [
        (&mut first, project.path(), "SetupA"),
        (&mut second, other.path(), "SetupB"),
    ] {
        let result = session_extract(session, root, "ts", source);
        assert_eq!(result.definitions.len(), 1);
        assert_eq!(result.definitions[0].registration, name);
        assert!(result.diagnostics.is_empty());
    }
    fs::write(
        other.path().join("support.ts"),
        "export { When } from '@cucumber/cucumber';",
    )
    .unwrap();
    assert_eq!(
        session_extract(&mut first, project.path(), "ts", IMPORTED_STEP)
            .definitions
            .len(),
        1
    );
    assert!(
        session_extract(&mut second, other.path(), "ts", IMPORTED_STEP)
            .definitions
            .is_empty()
    );
}

struct DefaultSessionAdapter;

impl SourceAdapter for DefaultSessionAdapter {
    /// Returns the default-delegation test adapter's diagnostic name.
    fn name(&self) -> &'static str {
        "default-session-test"
    }

    /// Identifies the default-delegation test adapter as JavaScript.
    fn language(&self) -> SourceLanguage {
        JAVASCRIPT
    }

    /// Returns a sentinel error so tests can detect delegation to the legacy extraction method.
    fn extract(&self, _source: &str, _file: &SourceFile) -> Result<Extraction> {
        anyhow::bail!("default extract delegation reached")
    }

    /// Delegation never reaches a comparison.
    fn analysis_policy(&self) -> AnalysisPolicy {
        AnalysisPolicy::unattached()
    }
}

/// Checks every supported suffix routes to the expected language and named adapter.
#[test]
fn registry_routes_supported_suffixes() {
    let cases = [
        ("steps.js", JAVASCRIPT),
        ("steps.mjs", JAVASCRIPT),
        ("steps.cjs", JAVASCRIPT),
        ("steps.jsx", JAVASCRIPT),
        ("steps.ts", TYPESCRIPT),
        ("steps.mts", TYPESCRIPT),
        ("steps.cts", TYPESCRIPT),
        ("steps.tsx", TSX),
        ("steps.rb", crate::ruby::RUBY),
    ];
    for (path, expected) in cases {
        let adapter = adapter_for_path(Path::new(path)).expect(path);
        assert_eq!(adapter.language(), expected, "{path}");
        let by_language = adapter_for_language(expected).unwrap();
        assert_eq!(by_language.language(), expected, "{path}");
        assert_eq!(by_language.name(), adapter.name(), "{path}");
        assert!(!adapter.name().is_empty());
    }
    assert_eq!(registered_suffixes().len(), cases.len());
}

/// Checks registry suffixes are unique, initialize sessions, and extract one definition into shared IR.
#[test]
fn every_registration_has_a_unique_suffix_and_produces_shared_ir() {
    let mut suffixes = BTreeSet::new();
    for registration in SOURCE_ADAPTER_REGISTRY {
        assert!(suffixes.insert(registration.suffix), "duplicate suffix");
        assert!(
            registration
                .hooks
                .is_some_and(|hooks| std::ptr::addr_eq(hooks.adapter, registration.adapter)),
            "{} is missing session hooks for its adapter",
            registration.suffix
        );
        let file = SourceFile {
            path: PathBuf::from(format!("steps{}", registration.suffix)),
            language: registration.adapter.language(),
        };
        let extraction = registration
            .adapter
            .extract(
                if file.language == crate::ruby::RUBY {
                    "Given('a registered step') { run() }"
                } else {
                    "Given('a registered step', () => run())"
                },
                &file,
            )
            .unwrap();
        assert_eq!(extraction.definitions.len(), 1, "{}", registration.suffix);
        assert!(extraction.diagnostics.is_empty(), "{}", registration.suffix);
    }
}

/// Checks declaration files, source maps, and unsupported language suffixes are excluded from adapter routing.
#[test]
fn registry_rejects_declarations_maps_and_unknown_languages() {
    for path in [
        "types.d.ts",
        "types.d.mts",
        "types.d.cts",
        ".d.ts",
        ".d.mts",
        ".d.cts",
        "steps.js.map",
        "steps.py",
    ] {
        assert!(adapter_for_path(Path::new(path)).is_none(), "{path}");
    }
}

/// Checks non-UTF-8 Unix filenames route by ASCII extension while declaration files remain excluded.
#[cfg(unix)]
#[test]
fn registry_classifies_non_utf8_names_by_their_ascii_extension() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let source = PathBuf::from(OsString::from_vec(b"steps-\xff.ts".to_vec()));
    assert_eq!(language_for_path(&source), Some(TYPESCRIPT));

    let declaration = PathBuf::from(OsString::from_vec(b"types-\xff.d.ts".to_vec()));
    assert!(adapter_for_path(&declaration).is_none());
}

/// Checks the default session extraction method delegates to an adapter's existing extract implementation.
#[test]
fn adapter_session_default_preserves_existing_extract_implementations() {
    let directory = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: directory.path().join("steps.js"),
        language: JAVASCRIPT,
    };
    let mut session = SourceExtractionSession::new(directory.path());

    let adapter = DefaultSessionAdapter;
    assert_eq!(adapter.name(), "default-session-test");
    assert_eq!(adapter.language(), JAVASCRIPT);
    // An adapter that declares no framework attributes definitions to none and excludes nothing;
    // Ruby declares one framework, JavaScript one ecosystem exclude.
    assert!(adapter.framework_contracts().is_empty());
    assert!(adapter.default_excludes().is_empty());
    assert!(!adapter_for_language(crate::ruby::RUBY)
        .unwrap()
        .framework_contracts()
        .is_empty());
    assert!(!adapter_for_language(JAVASCRIPT)
        .unwrap()
        .default_excludes()
        .is_empty());

    let error = adapter
        .extract_with_session("", &file, &mut session)
        .unwrap_err();

    assert_eq!(error.to_string(), "default extract delegation reached");
}

/// Checks language and suffix lookup select the identical registered adapter instances.
#[test]
fn language_lookup_and_suffix_routing_agree_on_one_adapter_table() {
    // Data-address identity (not `ptr::eq`, which compares wide-pointer vtables) is what
    // "one authoritative mapping" means here: both routing surfaces must select the same
    // adapter static, not merely two adapters that agree on name and language.
    for registration in SOURCE_ADAPTER_REGISTRY {
        let expected = registration.adapter.language();
        let name = format!("steps{}", registration.suffix);
        let routed = adapter_for_path(Path::new(&name)).expect(&name);
        assert!(
            std::ptr::addr_eq(
                routed as *const dyn SourceAdapter,
                registration.adapter as *const dyn SourceAdapter
            ),
            "{}",
            registration.suffix
        );
        let by_language = adapter_for_language(expected).unwrap();
        assert!(
            std::ptr::addr_eq(
                by_language as *const dyn SourceAdapter,
                registration.adapter as *const dyn SourceAdapter
            ),
            "{}",
            registration.suffix
        );
        assert_eq!(language_for_path(Path::new(&name)), Some(expected));
    }
    for language in [JAVASCRIPT, TYPESCRIPT, TSX, crate::ruby::RUBY] {
        assert_eq!(adapter_for_language(language).unwrap().language(), language);
    }
}

/// A language is its id: a value built from the id names the adapter's language, every registered
/// adapter declares one language, and an id no adapter declares routes nowhere.
#[test]
fn source_languages_are_open_ids_declared_by_adapters() {
    assert_eq!(SourceLanguage::new("ruby"), crate::ruby::RUBY);
    assert_eq!(crate::ruby::RUBY.id(), "ruby");
    let mut declared = std::collections::BTreeMap::new();
    for registration in SOURCE_ADAPTER_REGISTRY {
        let adapter = registration.adapter;
        let language = *declared
            .entry(adapter.name())
            .or_insert_with(|| adapter.language());
        assert_eq!(adapter.language(), language, "{}", registration.suffix);
    }
    let ids: BTreeSet<_> = declared.values().map(|language| language.id()).collect();
    assert_eq!(
        ids,
        BTreeSet::from(["javascript", "ruby", "tsx", "typescript"])
    );
    let python = SourceLanguage::new("python");
    assert!(adapter_for_language(python).is_none());
    assert_eq!(language_for_path(Path::new("steps.py")), None);
}

/// Extraction entry points reject a language no registered adapter declares, naming it.
#[test]
fn unregistered_languages_fail_at_extraction_entry_points() {
    let project = session_project();
    let file = |language| SourceFile {
        path: project.path().join("steps.ts"),
        language,
    };
    let python = file(SourceLanguage::new("python"));
    let source = "Given('a step', () => work());";
    for error in [
        crate::typescript::extract(source, &python).unwrap_err(),
        crate::typescript::extract_file(&python).unwrap_err(),
        SourceExtractionSession::new(project.path())
            .finalize(std::slice::from_ref(&python), &mut Vec::new())
            .unwrap_err(),
    ] {
        assert_eq!(
            error.to_string(),
            "no source adapter is registered for language `python`"
        );
    }
}

/// A registry of the built-in registrations whose adapter's language satisfies `keep`.
fn builtin_where(keep: impl Fn(SourceLanguage) -> bool) -> AdapterRegistry {
    AdapterRegistry::new(
        AdapterRegistry::builtin()
            .registrations()
            .iter()
            .copied()
            .filter(|registration| keep(registration.adapter.language())),
    )
    .unwrap()
}

/// Checks the built-in registry routes each suffix to its language and rejects unregistered,
/// declaration and source-map paths; configuration, defaults and sessions carry the static table.
#[test]
fn builtin_registry_routes_suffixes_and_is_carried_by_reference() {
    let builtin = AdapterRegistry::builtin();
    assert_eq!(
        builtin.suffixes(),
        [".rb", ".mjs", ".cjs", ".jsx", ".js", ".mts", ".cts", ".ts", ".tsx"]
    );
    for (path, expected) in [
        ("steps.rb", Some(crate::ruby::RUBY)),
        ("steps.cjs", Some(JAVASCRIPT)),
        ("steps.mts", Some(TYPESCRIPT)),
        ("steps.tsx", Some(TSX)),
        ("steps.py", None),
        ("types.d.ts", None),
        ("steps.js.map", None),
    ] {
        assert_eq!(
            builtin.language_for_path(Path::new(path)),
            expected,
            "{path}"
        );
    }
    assert_eq!(
        builtin
            .adapter_for_language(crate::ruby::RUBY)
            .map(|adapter| adapter.name()),
        Some("cucumber-ruby")
    );
    assert!(builtin
        .adapter_for_language(SourceLanguage::new("python"))
        .is_none());
    assert!(format!("{builtin:?}").starts_with(r#"{".rb": "cucumber-ruby""#));
    let project = session_project();
    let config = crate::config::Config::load(project.path(), Default::default()).unwrap();
    for registry in [
        config.adapters(),
        &AdapterRegistry::default(),
        &SourceExtractionSession::default().adapters,
        &SourceExtractionSession::new(project.path()).adapters,
        &SourceExtractionSession::for_config(&config).adapters,
    ] {
        assert!(std::ptr::eq(
            registry.registrations(),
            SOURCE_ADAPTER_REGISTRY
        ));
    }
}

/// Checks session finalization, registry-scoped uncertainty, discovery and framework detection
/// read the registry they carry: removing an adapter from it changes each answer.
#[test]
fn registry_readers_use_the_registry_they_carry() {
    let project = session_project();
    let root = project.path();
    let ruby = SourceFile {
        path: root.join("steps.rb"),
        language: crate::ruby::RUBY,
    };
    let mut session = SourceExtractionSession::new(root);
    session
        .finalize(std::slice::from_ref(&ruby), &mut Vec::new())
        .unwrap();
    session.adapters = builtin_where(|language| language != crate::ruby::RUBY);
    assert_eq!(
        session
            .finalize(std::slice::from_ref(&ruby), &mut Vec::new())
            .unwrap_err()
            .to_string(),
        "no source adapter is registered for language `ruby`"
    );

    let mut definitions = ts_definitions("Given('a step', () => work());");
    definitions.extend(
        registered_adapter(crate::ruby::RUBY)
            .unwrap()
            .extract("Given('a step') { work }", &ruby)
            .unwrap()
            .definitions,
    );
    let mut finalization = SourceFinalization::default();
    finalization.uncertainties.push(SourceUncertainty::new(
        UncertaintyScope::Registry(JAVASCRIPT),
        UncertaintyCause::Registration,
        "test evidence",
    ));
    for (registry, retained) in [
        (AdapterRegistry::builtin().clone(), 1),
        (builtin_where(|language| language != JAVASCRIPT), 0),
    ] {
        let mut actual = definitions.clone();
        finalization.apply_with(&registry, &mut actual);
        assert_eq!(actual.len(), retained, "{registry:?}");
    }

    fs::write(root.join("steps.rb"), "Given('a step') { work }").unwrap();
    fs::write(root.join("steps.ts"), "Given('a step', () => work());").unwrap();
    let overrides = crate::config::ConfigOverrides {
        definitions: Some(vec!["steps.rb".to_owned(), "steps.ts".to_owned()]),
        ..Default::default()
    };
    for (registry, expected, node_modules_excluded) in [
        (
            AdapterRegistry::builtin().clone(),
            vec!["steps.rb", "steps.ts"],
            true,
        ),
        (
            builtin_where(|language| language == crate::ruby::RUBY),
            vec!["steps.rb"],
            false,
        ),
    ] {
        let config =
            crate::config::Config::load_with_adapters(root, overrides.clone(), registry).unwrap();
        assert_eq!(
            config.exclude.contains(&"**/node_modules/**".to_owned()),
            node_modules_excluded
        );
        let discovered = crate::discovery::discover(&config).unwrap();
        let names: Vec<_> = discovered
            .definitions
            .iter()
            .map(|file| file.path.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, expected);
    }

    let package = serde_json::json!({"devDependencies": {"@cucumber/cucumber": "1"}});
    let empty = AdapterRegistry::new([]).unwrap();
    for (registry, detected) in [(AdapterRegistry::builtin(), true), (&empty, false)] {
        let found = crate::framework_registry::detect(registry, root, Some(&package)).unwrap();
        assert_eq!(found.is_some(), detected);
    }

    // The built-in Ruby adapter registered under another suffix through the public constructor
    // keeps its session state and dependency discovery.
    fs::write(
        root.join("steps.rake"),
        "require_relative 'helper'\nGiven('a step') { work }",
    )
    .unwrap();
    fs::write(root.join("helper.rb"), "def work; end").unwrap();
    let rake = crate::config::ConfigOverrides {
        definitions: Some(vec!["steps.rake".to_owned()]),
        ..Default::default()
    };
    let registry = AdapterRegistry::new([SourceAdapterRegistration::new(
        ".rake",
        registered_adapter(crate::ruby::RUBY).unwrap(),
    )])
    .unwrap();
    let config = crate::config::Config::load_with_adapters(root, rake, registry).unwrap();
    let discovered = crate::discovery::discover(&config).unwrap();
    // The followed load joins the definitions as a dependency-only Ruby source.
    assert_eq!(discovered.definitions.len(), 2, "{discovered:?}");
    assert_eq!(discovered.dependencies.len(), 1, "{discovered:?}");
    SourceExtractionSession::for_config(&config)
        .finalize(&discovered.definitions, &mut Vec::new())
        .unwrap();

    // A copied built-in registration whose adapter is replaced takes the replacement's hooks: a
    // caller adapter declaring Ruby follows no Ruby load.
    let mut replaced = *AdapterRegistry::builtin()
        .registrations()
        .iter()
        .find(|registration| registration.suffix == ".rb")
        .unwrap();
    replaced.adapter = &REPLACEMENT_RUBY;
    fs::write(
        root.join("steps.rb"),
        "require_relative 'helper'\nGiven('a step') { work }",
    )
    .unwrap();
    let rb = crate::config::ConfigOverrides {
        definitions: Some(vec!["steps.rb".to_owned()]),
        ..Default::default()
    };
    let registry = AdapterRegistry::new([replaced]).unwrap();
    let config = crate::config::Config::load_with_adapters(root, rb, registry).unwrap();
    let discovered = crate::discovery::discover(&config).unwrap();
    assert_eq!(discovered.definitions.len(), 1, "{discovered:?}");
    assert!(discovered.dependencies.is_empty(), "{discovered:?}");
}

/// A caller adapter declaring Ruby, to replace the built-in one.
struct ReplacementRuby;

impl SourceAdapter for ReplacementRuby {
    /// Returns a name distinct from the built-in Ruby adapter's.
    fn name(&self) -> &'static str {
        "replacement-ruby"
    }
    /// Declares Ruby.
    fn language(&self) -> SourceLanguage {
        crate::ruby::RUBY
    }
    /// Extracts nothing.
    fn extract(&self, _: &str, _: &SourceFile) -> Result<Extraction> {
        Ok(Extraction::new(Vec::new(), Vec::new()))
    }
    /// Declares Ruby's comparison policy.
    fn analysis_policy(&self) -> AnalysisPolicy {
        crate::ruby::semantics::policy()
    }
}

static REPLACEMENT_RUBY: ReplacementRuby = ReplacementRuby;

/// Checks registered languages pass the extraction and finalization entry points that reject an unregistered id.
#[test]
fn registered_languages_pass_the_extraction_entry_points() {
    let project = session_project();
    for (name, language, source) in [
        ("steps.ts", TYPESCRIPT, "Given('a step', () => work());"),
        ("steps.rb", crate::ruby::RUBY, "Given('a step') { work }"),
    ] {
        let file = SourceFile {
            path: project.path().join(name),
            language,
        };
        let extraction = registered_adapter(language)
            .unwrap()
            .extract(source, &file)
            .unwrap();
        assert_eq!(extraction.definitions.len(), 1, "{name}");
        SourceExtractionSession::new(project.path())
            .finalize(std::slice::from_ref(&file), &mut Vec::new())
            .unwrap();
    }
}

/// Checks either routing surface can warm shared session state without changing subsequent extraction.
#[test]
fn language_routed_and_suffix_routed_extraction_share_session_state() {
    // The registry's session initializers must prepare exactly the backend state that
    // `adapter_for_language` routes to: the same resolver-backed import has to extract
    // identically through either surface, whichever warms the shared cache first.
    let project = session_project();
    for suffix in ["js", "ts", "tsx"] {
        for language_route_first in [true, false] {
            let path = project.path().join(format!("steps.{suffix}"));
            let language = language_for_path(&path).expect(suffix);
            let file = SourceFile { path, language };
            let suffix_routed = adapter_for_path(&file.path).expect(suffix);
            let language_routed = adapter_for_language(language).unwrap();
            let mut session = SourceExtractionSession::new(project.path());
            let (first, second) = if language_route_first {
                (language_routed, suffix_routed)
            } else {
                (suffix_routed, language_routed)
            };
            let results = [
                first.extract_with_session(IMPORTED_STEP, &file, &mut session),
                second.extract_with_session(IMPORTED_STEP, &file, &mut session),
            ];
            let [first, second] = results;
            let extraction = first.unwrap();
            assert_eq!(&extraction, second.as_ref().unwrap(), "{suffix}");
            assert_eq!(extraction.definitions.len(), 1, "{suffix}");
            assert!(extraction.diagnostics.is_empty(), "{suffix}");
            // Every registered adapter is TypeScript-backed today, so one shared backend
            // state must serve all of them; a second backend is allowed to change this.
            assert_eq!(session.states.len(), 2, "{suffix}");
        }
    }
}

/// Checks legacy diagnostic classification follows message prefixes and responds to message mutation.
#[test]
fn legacy_diagnostics_preserve_message_classification_and_mutation() {
    for (message, incomplete, unparseable) in [
        ("ordinary warning", false, false),
        (UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX, true, false),
        (UNPARSEABLE_SOURCE_DIAGNOSTIC_PREFIX, true, true),
    ] {
        let mut diagnostic = ExtractionDiagnostic::new(
            ExtractionDiagnosticLevel::Warning,
            SourceLocation::new("steps.ts", 1, 1, 1, 1),
            message,
        );
        assert_eq!(is_completeness_diagnostic(&diagnostic), incomplete);
        assert_eq!(is_unparseable_diagnostic(&diagnostic), unparseable);
        diagnostic.message = "ordinary warning".into();
        assert!(!is_completeness_diagnostic(&diagnostic));
        assert!(!is_unparseable_diagnostic(&diagnostic));
    }
}

/// Checks uncertainty scope and cause selectively remove registrations or handler trust while preserving independent findings.
#[test]
fn finalization_scopes_preserve_independent_final_findings() {
    use crate::model::Rule;
    use UncertaintyCause::{Handler, Registration, Source};
    let project = session_project();
    let root = project.path();
    let mut session = SourceExtractionSession::new(root);
    let mut definitions = session_extract(
        &mut session,
        root,
        "ts",
        "Given('unrelated control', () => check()); Given('unrelated control', () => check());",
    )
    .definitions;
    definitions.extend(session_extract(&mut session, root, "js",
        "Given('opens red panel', () => work()); Given('opens red panel', () => work()); Given('opens blue panel', () => work());").definitions);
    let config = crate::config::Config::load(root, Default::default()).unwrap();
    let definition = UncertaintyScope::Definition(definitions[4].location.clone());
    let file = UncertaintyScope::File(root.join("steps.js"));
    let registry = UncertaintyScope::Registry(JAVASCRIPT);
    for (scope, cause, retained, matchers, handlers) in [
        (definition.clone(), Registration, 4, 2, 0),
        (file.clone(), Registration, 2, 1, 0),
        (definition, Handler, 5, 2, 0),
        (file, Handler, 5, 2, 0),
        (registry, Source, 5, 2, 2),
    ] {
        let mut actual = definitions.clone();
        let mut finalization = SourceFinalization::default();
        finalization
            .uncertainties
            .push(SourceUncertainty::new(scope, cause, "test evidence"));
        finalization.apply(&mut actual);
        assert_eq!(actual.len(), retained, "{finalization:?}");
        let result = crate::analysis::analyze(actual, vec![], &config).unwrap();
        for (rule, expected) in [
            (Rule::DuplicateMatcher, matchers),
            (Rule::DuplicateHandler, handlers),
        ] {
            let actual = result.findings.iter().filter(|f| f.rule == rule).count();
            assert_eq!(actual, expected, "{finalization:?}");
        }
    }
}

/// `definitions` after a registration effect scoped to `language`'s registry.
fn after_registry_effect(
    mut definitions: Vec<StepDefinition>,
    language: SourceLanguage,
) -> Vec<StepDefinition> {
    let mut finalization = SourceFinalization::default();
    finalization.uncertainties.push(SourceUncertainty::new(
        UncertaintyScope::Registry(language),
        UncertaintyCause::Registration,
        "registry effect",
    ));
    finalization.apply(&mut definitions);
    definitions
}

/// Checks registry uncertainty groups ECMAScript variants together while keeping Ruby findings
/// independent, and covers every definition for a language no registered adapter declares.
#[test]
fn registry_uncertainty_isolates_languages_but_groups_ecmascript_variants() {
    use crate::model::Rule;
    let project = session_project();
    let root = project.path();
    let mut session = SourceExtractionSession::new(root);
    let mut definitions = Vec::new();
    for suffix in ["js", "ts", "tsx", "rb"] {
        let source = if suffix == "rb" {
            "Given('shared step') { work() }; Given('shared step') { work() }"
        } else {
            IMPORTED_STEP
        };
        definitions.extend(session_extract(&mut session, root, suffix, source).definitions);
    }
    let config = crate::config::Config::load(root, Default::default()).unwrap();
    for (language, retained, matcher_findings) in [
        (JAVASCRIPT, 2, 1),
        (TYPESCRIPT, 2, 1),
        (TSX, 2, 1),
        (crate::ruby::RUBY, 3, 2),
        (SourceLanguage::new("python"), 0, 0),
    ] {
        let actual = after_registry_effect(definitions.clone(), language);
        assert_eq!(actual.len(), retained, "{language:?}");
        let result = crate::analysis::analyze_with_diagnostics(actual, vec![], &config)
            .unwrap()
            .result;
        let rule = Rule::DuplicateMatcher;
        let actual = result.findings.iter().filter(|f| f.rule == rule).count();
        assert_eq!(actual, matcher_findings, "{language:?}");
    }
}

/// A registry-scoped effect reaches an unattached definition, whose language nothing shows, and
/// leaves an extracted definition of another domain alone.
#[test]
fn registry_uncertainty_covers_unattached_definitions() {
    let extracted = adapter_for_language(JAVASCRIPT)
        .unwrap()
        .extract(
            "import { Given } from '@cucumber/cucumber'; Given('a', () => work());",
            &SourceFile {
                path: "steps.js".into(),
                language: JAVASCRIPT,
            },
        )
        .unwrap()
        .definitions;
    let mut unattached = extracted.clone();
    unattached[0].analysis_policy = AnalysisPolicy::unattached();
    for (definitions, language, retained) in [
        (&extracted, crate::ruby::RUBY, 1),
        (&extracted, JAVASCRIPT, 0),
        (&unattached, crate::ruby::RUBY, 0),
        (&unattached, JAVASCRIPT, 0),
    ] {
        let actual = after_registry_effect(definitions.clone(), language);
        assert_eq!(actual.len(), retained, "{language:?}");
    }
}

/// Checks session finalization applies late Ruby registration mutation in either order and remains idempotent.
#[test]
fn public_session_finalization_applies_late_ruby_effects_in_either_order() {
    for mutation_first in [false, true] {
        let project = session_project();
        let root = project.path();
        let mut session = SourceExtractionSession::new(root);
        let files: Vec<_> = ["registrations.rb", "mutation.rb"]
            .into_iter()
            .map(|name| SourceFile {
                path: root.join(name),
                language: crate::ruby::RUBY,
            })
            .collect();
        let sources = [
            "Given('shared') { work() }; Then('shared') { work() }",
            "def Given(text); :replaced; end",
        ];
        let mut definitions = session_extract(&mut session, root, "ts", IMPORTED_STEP).definitions;
        for index in if mutation_first { [1, 0] } else { [0, 1] } {
            let extracted = adapter_for_language(crate::ruby::RUBY)
                .unwrap()
                .extract_with_session(sources[index], &files[index], &mut session)
                .unwrap();
            definitions.extend(extracted.definitions);
        }
        if !mutation_first {
            assert_eq!(definitions.len(), 3);
        }
        let first = session.finalize(&files, &mut definitions).unwrap();
        assert!(!first.uncertainties.is_empty());
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].location.path, root.join("steps.ts"));
        let once = definitions.clone();
        assert_eq!(session.finalize(&files, &mut definitions).unwrap(), first);
        assert_eq!(definitions, once);
    }
}

use crate::model::{EffectStream, FeatureStep, Framework, Rule, SourceLocation};
use crate::source_adapter::semantics::{DomainSpec, HandlerScope, StepMatching, StreamEvidence};
use crate::typescript::frameworks::{
    CUCUMBER_JS, CYPRESS_CUCUMBER, JEST_CUCUMBER, PLAYWRIGHT_BDD, VITEST_CUCUMBER,
};

/// Extracts TypeScript definitions from `source` in `steps.ts`, panicking on failure.
fn ts_definitions(source: &str) -> Vec<StepDefinition> {
    crate::typescript::extract(
        source,
        &SourceFile {
            path: "steps.ts".into(),
            language: TYPESCRIPT,
        },
    )
    .unwrap()
}

/// One feature step with the given text.
fn feature_step(text: &str) -> Vec<FeatureStep> {
    vec![FeatureStep {
        keyword: "Given".into(),
        text: text.into(),
        location: SourceLocation::new("steps.feature", 1, 1, 1, 10),
    }]
}

/// Analyzes `definitions` against `steps` with default limits.
fn analyze_policy_case(
    definitions: Vec<StepDefinition>,
    steps: Vec<FeatureStep>,
) -> Result<crate::model::AnalysisResult> {
    let root = tempfile::tempdir().unwrap();
    let config = crate::config::Config::load(root.path(), Default::default()).unwrap();
    crate::analysis::analyze_with_step_usage(definitions, steps, &config, &Default::default())
        .map(|outcome| outcome.result)
}

/// Whether analyzing `definitions` against `steps` reports `rule`; panics when analysis fails.
fn reports(definitions: Vec<StepDefinition>, steps: Vec<FeatureStep>, rule: Rule) -> bool {
    analyze_policy_case(definitions, steps)
        .unwrap()
        .findings
        .iter()
        .any(|finding| finding.rule == rule)
}

/// Every adapter-declared policy resolves to the capabilities and stream flags its adapter owns;
/// no two policies that differ today collapse into one.
#[test]
fn adapter_policies_resolve_to_their_declared_capabilities() {
    use crate::typescript::semantics::policy;
    const UNKNOWN: Framework = Framework::UNKNOWN;
    use HandlerScope::{Isolated, Local, Shared};
    use StepMatching::{ByText, Positional};
    let ruby = adapter_for_language(crate::ruby::RUBY)
        .unwrap()
        .analysis_policy();
    let ecmascript = [
        (CUCUMBER_JS, Shared, ByText),
        (PLAYWRIGHT_BDD, Shared, ByText),
        (CYPRESS_CUCUMBER, Shared, ByText),
        (UNKNOWN, Shared, ByText),
        // Ids no JavaScript/TypeScript contract declares keep the adapter's default policy.
        (crate::ruby::CUCUMBER_RUBY, Shared, ByText),
        (Framework::new("undeclared"), Shared, ByText),
        (JEST_CUCUMBER, Local, Positional),
        (VITEST_CUCUMBER, Isolated, ByText),
    ];
    let flags = |profile: &crate::source_adapter::semantics::AnalysisProfile| {
        (
            profile.requires_identical_handler,
            profile.scores_event_overlap,
            profile.excludes_identical_event_streams,
        )
    };
    let mut regimes = BTreeSet::new();
    for (stream, expected) in [
        (EffectStream::Partial, (true, false, false)),
        (EffectStream::Complete, (false, true, false)),
        (EffectStream::ActionOnly, (false, true, true)),
    ] {
        let profile = ruby.profile(stream);
        assert_eq!(profile.domain.label, "ruby");
        assert_eq!(
            (
                profile.handler_scope,
                profile.step_matching,
                profile.step_invocation
            ),
            (Shared, ByText, true)
        );
        assert_eq!(flags(&profile), expected, "{stream:?}");
        regimes.insert(expected);
        // A complete-stream policy ignores what the handler declares.
        for (framework, scope, matching) in ecmascript {
            let profile = policy(framework).profile(stream);
            assert_eq!(profile.domain.label, "ecmascript");
            assert_eq!(
                (
                    profile.handler_scope,
                    profile.step_matching,
                    profile.step_invocation
                ),
                (scope, matching, false),
                "{framework:?}/{stream:?}"
            );
            assert_eq!(
                flags(&profile),
                (false, true, false),
                "{framework:?}/{stream:?}"
            );
        }
    }
    assert_eq!(regimes.len(), 3);
    let shapes = ecmascript.map(|(_, scope, matching)| format!("{scope:?}/{matching:?}"));
    assert_eq!(shapes.iter().collect::<BTreeSet<_>>().len(), 3);
    // `Complete` evidence overrides what a handler declares; `Declared` follows it.
    for (evidence, identical) in [
        (StreamEvidence::Complete, false),
        (StreamEvidence::Declared, true),
    ] {
        let policy = AnalysisPolicy::new(ruby.domain(), Shared, ByText, evidence, true);
        let profile = policy.profile(EffectStream::Partial);
        assert_eq!(profile.requires_identical_handler, identical);
    }
}

/// Extraction attaches the policy of the framework that registered each definition, and Ruby its
/// own. A serialized definition reads back with the policy extraction attached.
#[test]
fn extraction_attaches_the_policy_of_the_registering_framework() {
    let baseline = crate::typescript::semantics::default_policy();
    assert_eq!(
        baseline,
        crate::typescript::semantics::policy(Framework::UNKNOWN)
    );
    // The debug form names the domain label, so a failing assertion shows which domain differed.
    assert!(format!("{baseline:?}").contains("DomainSpec(ecmascript)"));
    // Every definition carries one policy; keep it to a pointer plus packed flags.
    assert!(std::mem::size_of::<AnalysisPolicy>() <= 16);
    let mut extracted = Vec::new();
    for (source, framework) in [
        (
            "import { Given } from '@cucumber/cucumber'; Given('a', () => work());",
            CUCUMBER_JS,
        ),
        (
            "import { Given } from 'playwright-bdd'; Given('a', () => work());",
            PLAYWRIGHT_BDD,
        ),
        (
            "import { Given } from '@badeball/cypress-cucumber-preprocessor'; Given('a', () => work());",
            CYPRESS_CUCUMBER,
        ),
        (
            "import { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => test('t', ({ given }) => { given('a', () => work()); }));",
            JEST_CUCUMBER,
        ),
        (
            "import { Given } from 'vitest-cucumber-plugin'; Given('a', () => work());",
            VITEST_CUCUMBER,
        ),
        ("Given('a', () => work());", Framework::UNKNOWN),
    ] {
        let definitions = ts_definitions(source);
        assert_eq!(definitions.len(), 1, "{framework:?}");
        assert_eq!(definitions[0].framework, framework);
        assert_eq!(
            definitions[0].analysis_policy,
            crate::typescript::semantics::policy(framework),
            "{framework:?}"
        );
        extracted.push(definitions[0].clone());
    }
    let ruby = adapter_for_language(crate::ruby::RUBY)
        .unwrap()
        .extract(
            "Given('a') { work() }",
            &SourceFile {
                path: "steps.rb".into(),
                language: crate::ruby::RUBY,
            },
        )
        .unwrap();
    assert_eq!(
        ruby.definitions[0].analysis_policy,
        crate::ruby::semantics::policy()
    );
    assert_ne!(ruby.definitions[0].analysis_policy, baseline);
    extracted.push(ruby.definitions[0].clone());
    // The policy is not serialized; reading back attaches the one extraction did, which differs
    // from the baseline for Jest-Cucumber, vitest-cucumber-plugin and Ruby.
    let baselines = extracted
        .iter()
        .filter(|definition| definition.analysis_policy == baseline)
        .count();
    assert_eq!(baselines, extracted.len() - 3);
    for definition in extracted {
        let restored: StepDefinition =
            serde_json::from_str(&serde_json::to_string(&definition).unwrap()).unwrap();
        assert_eq!(restored, definition, "{:?}", definition.framework);
    }
}

/// A language's comparison domain is the one its registered adapter declares, and the definitions
/// that adapter extracts carry it.
#[test]
fn language_domains_follow_the_registered_adapters() {
    let ruby = "Given('a') { work() }";
    let ecmascript = "import { Given } from '@cucumber/cucumber'; Given('a', () => work());";
    for (language, label, suffix, source) in [
        (crate::ruby::RUBY, "ruby", "rb", ruby),
        (JAVASCRIPT, "ecmascript", "js", ecmascript),
        (TYPESCRIPT, "ecmascript", "ts", ecmascript),
        (TSX, "ecmascript", "tsx", ecmascript),
    ] {
        let domain = AdapterRegistry::builtin()
            .comparison_domain(language)
            .unwrap();
        assert_eq!(domain.label, label, "{language:?}");
        let extracted = adapter_for_language(language)
            .unwrap()
            .extract(
                source,
                &SourceFile {
                    path: format!("steps.{suffix}").into(),
                    language,
                },
            )
            .unwrap();
        assert_eq!(extracted.definitions.len(), 1, "{language:?}");
        assert_eq!(
            extracted.definitions[0].analysis_policy.domain(),
            domain,
            "{language:?}"
        );
    }
    assert_ne!(
        AdapterRegistry::builtin().comparison_domain(crate::ruby::RUBY),
        AdapterRegistry::builtin().comparison_domain(TSX)
    );
    // A language no registered adapter declares has no comparison domain.
    assert_eq!(
        AdapterRegistry::builtin().comparison_domain(SourceLanguage::new("python")),
        None
    );
    // Domains are identified by their static spec: a reused label does not merge them.
    let ruby = AdapterRegistry::builtin()
        .comparison_domain(crate::ruby::RUBY)
        .unwrap();
    let alias: &'static DomainSpec = Box::leak(Box::new(DomainSpec::new(ruby.label, ruby.dialect)));
    assert_ne!(alias, ruby);
    // A domain reads as its label in diagnostics until it is named.
    let unnamed = DomainSpec::new("unnamed", ruby.dialect);
    assert_eq!(unnamed.display_name, "unnamed");
    assert_eq!(unnamed.with_display_name("Named").display_name, "Named");
}

/// Domain names come in registry order, each once, for the languages present only.
#[test]
fn domain_names_follow_the_registry_for_the_languages_present() {
    let names = |languages: &[SourceLanguage]| {
        AdapterRegistry::builtin().domain_names(languages.iter().copied())
    };
    assert_eq!(names(&[]), [""; 0]);
    assert_eq!(names(&[SourceLanguage::new("python")]), [""; 0]);
    assert_eq!(names(&[TSX, JAVASCRIPT]), ["JS/TS"]);
    assert_eq!(names(&[crate::ruby::RUBY]), ["Ruby"]);
    assert_eq!(
        names(&[TYPESCRIPT, crate::ruby::RUBY, JAVASCRIPT]),
        ["Ruby", "JS/TS"]
    );
}

/// The attached policy, not the framework label, selects the comparison behaviour: flipping only
/// the policy of an otherwise identical definition flips the outcome, and a framework label that
/// disagrees with the policy changes nothing.
#[test]
fn attached_policy_decides_outcomes_not_the_framework_label() {
    let ruby = crate::ruby::semantics::policy();
    let ecmascript = crate::typescript::semantics::policy(CUCUMBER_JS);
    let positional = crate::typescript::semantics::policy(JEST_CUCUMBER);
    let isolated = crate::typescript::semantics::policy(VITEST_CUCUMBER);

    let unused = |policy: AnalysisPolicy, label| {
        let mut definitions = ts_definitions("Given(/^placeholder$/, () => { load(); save(); });");
        definitions[0].matcher = "^alice$".into();
        definitions[0].framework = label;
        definitions[0].analysis_policy = policy;
        reports(
            definitions,
            feature_step("before\nalice\nafter"),
            Rule::UnusedDefinition,
        )
    };
    for label in [CUCUMBER_JS, crate::ruby::CUCUMBER_RUBY] {
        // Ruby anchors match per line, so the step selects the definition; ECMAScript anchors do
        // not. Positional steps are never reported unused: only their own position selects them.
        assert!(!unused(ruby, label), "{label:?}");
        assert!(unused(ecmascript, label), "{label:?}");
        assert!(!unused(positional, label), "{label:?}");
    }

    let duplicate_handlers = |left: AnalysisPolicy, right: AnalysisPolicy| {
        let mut definitions = ts_definitions(
            "Given('a user exists', () => { load(); save(); }); Given('the basket is empty', () => { load(); save(); });",
        );
        definitions[0].analysis_policy = left;
        definitions[1].analysis_policy = right;
        definitions[1].location.path = "other.ts".into();
        reports(definitions, vec![], Rule::DuplicateHandler)
    };
    assert!(duplicate_handlers(ecmascript, ecmascript));
    assert!(duplicate_handlers(isolated, isolated));
    assert!(duplicate_handlers(positional, positional));
    assert!(!duplicate_handlers(positional, ecmascript));
    assert!(!duplicate_handlers(isolated, ecmascript));
    assert!(!duplicate_handlers(positional, isolated));

    // Domains come from the policy: a Ruby label on an ECMAScript policy mixes nothing, and an
    // ECMAScript label on a Ruby policy does.
    let mixed = |left_label, left: AnalysisPolicy, right: AnalysisPolicy| {
        let mut definitions = ts_definitions(
            "Given('a user exists', () => { load(); save(); }); Given('a cart exists', () => { load(); save(); });",
        );
        definitions[0].framework = left_label;
        definitions[0].analysis_policy = left;
        definitions[1].analysis_policy = right;
        analyze_policy_case(definitions, vec![])
    };
    assert!(mixed(crate::ruby::CUCUMBER_RUBY, ecmascript, ecmascript).is_ok());
    assert!(mixed(CUCUMBER_JS, ruby, ecmascript)
        .unwrap_err()
        .to_string()
        .contains("separate analysis runs"));
}

/// A positional step pairs with a text-matched step of the same matcher by near wording, never as
/// an exact matcher or a parameterization of it, even when their handler scopes agree; two
/// text-matched steps of that matcher are an exact duplicate.
#[test]
fn positional_steps_pair_by_wording_with_text_matched_steps() {
    let text = crate::typescript::semantics::policy(CUCUMBER_JS);
    let positional = AnalysisPolicy::new(
        text.domain(),
        HandlerScope::Shared,
        StepMatching::Positional,
        StreamEvidence::Complete,
        false,
    );
    let rules = |left: AnalysisPolicy, right: AnalysisPolicy| {
        let mut definitions = ts_definitions(
            "Given('the parcel is ready', () => { work(1); }); Given('the parcel is ready', () => { work(2); });",
        );
        definitions[0].analysis_policy = left;
        definitions[1].analysis_policy = right;
        analyze_policy_case(definitions, feature_step("the parcel is ready"))
            .unwrap()
            .findings
            .iter()
            .map(|finding| finding.rule)
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        rules(text, text),
        BTreeSet::from([Rule::DuplicateMatcher, Rule::AmbiguousStep])
    );
    for pair in [rules(positional, text), rules(text, positional)] {
        assert_eq!(pair, BTreeSet::from([Rule::NearDuplicateStep]));
    }
}

/// The JavaScript, TypeScript and TSX registrations share one adapter type, so the registered
/// contracts list each framework once, Ruby's first, then the JavaScript/TypeScript contracts in
/// their precedence order.
#[test]
fn registered_framework_contracts_list_each_framework_once_in_registry_order() {
    let ids: Vec<&str> = AdapterRegistry::builtin()
        .framework_contracts()
        .map(|contract| contract.framework.id())
        .collect();
    assert_eq!(
        ids,
        [
            "cucumber-ruby",
            "playwright-bdd",
            "cypress-cucumber",
            "cucumber-js",
            "jest-cucumber",
            "vitest-cucumber",
        ]
    );
}

/// An adapter outside the crate builds a contract from its id alone: no discovery, no excludes
/// until it supplies them.
#[test]
fn public_framework_contracts_start_without_discovery_or_excludes() {
    let framework = Framework::new("external-framework");
    let contract = FrameworkContract::new(framework);
    assert_eq!(contract.framework, framework);
    assert!(contract.discovery.is_none() && contract.excludes.is_empty());
    let contract = contract.with_excludes(&["**/reports/**"]);
    assert_eq!(
        (contract.framework, contract.excludes),
        (framework, &["**/reports/**"][..])
    );
}

/// The contract builders attach discovery and a policy, and `policy_of` falls back to the
/// adapter default for a contract without one and for an id no contract declares.
#[test]
fn contract_builders_attach_discovery_and_policy() {
    let framework = Framework::new("external-framework");
    let discovery = FrameworkDescriptor::new("External", |_| Ok(None));
    let baseline = crate::typescript::semantics::default_policy();
    let local = AnalysisPolicy::new(
        baseline.domain(),
        HandlerScope::Local,
        StepMatching::ByText,
        StreamEvidence::Complete,
        false,
    );
    assert_ne!(local, baseline);
    let plain = FrameworkContract::new(framework).with_discovery(discovery);
    assert!(plain.discovery.is_some() && plain.policy.is_none());
    let scoped = FrameworkContract::new(framework).with_policy(local);
    assert_eq!(scoped.policy, Some(local));
    for (contracts, expected) in [([plain], baseline), ([scoped], local)] {
        assert_eq!(
            FrameworkContract::policy_of(&contracts, framework, baseline),
            expected
        );
        assert_eq!(
            FrameworkContract::policy_of(&contracts, Framework::UNKNOWN, baseline),
            baseline
        );
    }
}
