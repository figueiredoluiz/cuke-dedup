use super::*;
use std::collections::BTreeSet;
use std::fs;

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

#[test]
fn session_state_initializes_once_per_backend_and_per_run() {
    struct Counter(usize);
    impl AdapterSessionState for Counter {
        fn initialize(_: Option<&Path>, _: &[String], _: &[String]) -> Self {
            Self(0)
        }
    }
    struct Other;
    impl AdapterSessionState for Other {
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

#[test]
fn session_mixed_languages_share_cache_and_preserve_findings_in_either_order() {
    for suffixes in [["js", "ts", "tsx"], ["tsx", "ts", "js"]] {
        let project = session_project();
        let root = project.path();
        let mut session = SourceExtractionSession::new(root);
        assert_eq!(session.states.len(), 1);
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
    fn name(&self) -> &'static str {
        "default-session-test"
    }

    fn language(&self) -> SourceLanguage {
        SourceLanguage::JavaScript
    }

    fn extract(&self, _source: &str, _file: &SourceFile) -> Result<Extraction> {
        anyhow::bail!("default extract delegation reached")
    }
}

#[test]
fn registry_routes_supported_suffixes() {
    let cases = [
        ("steps.js", SourceLanguage::JavaScript),
        ("steps.mjs", SourceLanguage::JavaScript),
        ("steps.cjs", SourceLanguage::JavaScript),
        ("steps.jsx", SourceLanguage::JavaScript),
        ("steps.ts", SourceLanguage::TypeScript),
        ("steps.mts", SourceLanguage::TypeScript),
        ("steps.cts", SourceLanguage::TypeScript),
        ("steps.tsx", SourceLanguage::Tsx),
    ];
    for (path, expected) in cases {
        let adapter = adapter_for_path(Path::new(path)).expect(path);
        assert_eq!(adapter.language(), expected, "{path}");
        let by_language = adapter_for_language(expected);
        assert_eq!(by_language.language(), expected, "{path}");
        assert_eq!(by_language.name(), adapter.name(), "{path}");
        assert!(!adapter.name().is_empty());
    }
    assert_eq!(registered_suffixes().len(), cases.len());
}

#[test]
fn source_languages_select_their_registered_tree_sitter_grammar() {
    let cases = [
        (SourceLanguage::JavaScript, "const value = 1;"),
        (SourceLanguage::TypeScript, "const value: number = 1;"),
        (SourceLanguage::Tsx, "const value = <div />;"),
    ];

    for (language, source) in cases {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&grammar_for_language(language))
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(!tree.root_node().has_error(), "{language:?}");
    }
}

#[test]
fn every_registration_has_a_unique_suffix_and_produces_shared_ir() {
    let mut suffixes = BTreeSet::new();
    for registration in SOURCE_ADAPTER_REGISTRY {
        assert!(suffixes.insert(registration.suffix), "duplicate suffix");
        assert!(
            registration.initialize_session.is_some(),
            "{} is missing session initialization",
            registration.suffix
        );
        let file = SourceFile {
            path: PathBuf::from(format!("steps{}", registration.suffix)),
            language: registration.adapter.language(),
        };
        let extraction = registration
            .adapter
            .extract("Given('a registered step', () => run())", &file)
            .unwrap();
        assert_eq!(extraction.definitions.len(), 1, "{}", registration.suffix);
        assert!(extraction.diagnostics.is_empty(), "{}", registration.suffix);
    }
}

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

#[cfg(unix)]
#[test]
fn registry_classifies_non_utf8_names_by_their_ascii_extension() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let source = PathBuf::from(OsString::from_vec(b"steps-\xff.ts".to_vec()));
    assert_eq!(language_for_path(&source), Some(SourceLanguage::TypeScript));

    let declaration = PathBuf::from(OsString::from_vec(b"types-\xff.d.ts".to_vec()));
    assert!(adapter_for_path(&declaration).is_none());
}

#[test]
fn adapter_session_default_preserves_existing_extract_implementations() {
    let directory = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: directory.path().join("steps.js"),
        language: SourceLanguage::JavaScript,
    };
    let mut session = SourceExtractionSession::new(directory.path());

    let adapter = DefaultSessionAdapter;
    assert_eq!(adapter.name(), "default-session-test");
    assert_eq!(adapter.language(), SourceLanguage::JavaScript);

    let error = adapter
        .extract_with_session("", &file, &mut session)
        .unwrap_err();

    assert_eq!(error.to_string(), "default extract delegation reached");
}

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
        let by_language = adapter_for_language(expected);
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
    for language in [
        SourceLanguage::JavaScript,
        SourceLanguage::TypeScript,
        SourceLanguage::Tsx,
    ] {
        assert_eq!(adapter_for_language(language).language(), language);
    }
}

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
            let language_routed = adapter_for_language(language);
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
            assert_eq!(session.states.len(), 1, "{suffix}");
        }
    }
}
