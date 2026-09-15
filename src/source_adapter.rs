//! Definition-source adapter contracts and extension-based routing.

use crate::model::{SourceLocation, StepDefinition};
use crate::resource_limits::{read_utf8, MAX_PROJECT_INPUT_BYTES};
use anyhow::{Context, Result};
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::{Path, PathBuf};

pub(crate) const UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX: &str =
    "unresolved step-registration calls:";

/// Parser language selected for a definition source.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLanguage {
    /// JavaScript, JSX, or their module variants.
    JavaScript,
    /// TypeScript or its module variants.
    TypeScript,
    /// TypeScript with JSX syntax.
    Tsx,
}

/// Returns the tree-sitter grammar registered for a definition-source language.
pub(crate) fn grammar_for_language(language: SourceLanguage) -> tree_sitter::Language {
    match language {
        SourceLanguage::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        SourceLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        SourceLanguage::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
    }
}

/// A discovered definition source and the language used to parse it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Absolute source path.
    pub path: PathBuf,
    /// Parser language inferred from the registered extension.
    pub language: SourceLanguage,
}

/// Impact of a source-extraction diagnostic.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionDiagnosticLevel {
    /// Analysis continues, but a static-analysis limitation applies.
    Warning,
    /// The source contains a malformed step registration.
    Error,
}

/// A source-localized problem encountered while extracting definitions.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionDiagnostic {
    /// Diagnostic impact.
    pub level: ExtractionDiagnosticLevel,
    /// Source position of the affected matcher.
    pub location: SourceLocation,
    /// Human-readable explanation.
    pub message: String,
}

impl ExtractionDiagnostic {
    /// Creates a source-localized extraction diagnostic.
    pub fn new(
        level: ExtractionDiagnosticLevel,
        location: SourceLocation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            level,
            location,
            message: message.into(),
        }
    }
}

pub(crate) fn is_completeness_diagnostic(diagnostic: &ExtractionDiagnostic) -> bool {
    diagnostic
        .message
        .starts_with(UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX)
}

/// Definitions and non-fatal diagnostics extracted from one source.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extraction {
    /// Successfully extracted definitions.
    pub definitions: Vec<StepDefinition>,
    /// Problems that did not prevent extraction of the rest of the file.
    pub diagnostics: Vec<ExtractionDiagnostic>,
}

impl Extraction {
    /// Creates an extraction result from definitions and non-fatal diagnostics.
    pub fn new(definitions: Vec<StepDefinition>, diagnostics: Vec<ExtractionDiagnostic>) -> Self {
        Self {
            definitions,
            diagnostics,
        }
    }
}

/// Reusable state for extracting multiple definition sources in one analysis run.
///
/// Reusing a session shares bounded parser and module-resolution work across files. Create one
/// session per analysis root rather than sharing it between unrelated repositories.
#[derive(Default)]
pub struct SourceExtractionSession {
    root: Option<PathBuf>,
    registrations: Vec<String>,
    states: HashMap<TypeId, Box<dyn Any + Send + Sync + UnwindSafe + RefUnwindSafe>>,
}

/// Backend-owned state; related syntax variants can share the same state type.
/// The bounds preserve the public session's existing thread and unwind-safety guarantees.
pub(crate) trait AdapterSessionState:
    Any + Send + Sync + UnwindSafe + RefUnwindSafe
{
    fn initialize(root: Option<&Path>, registrations: &[String]) -> Self;
}

/// Internal stateful registrations must explicitly select their backend state.
pub(crate) trait StatefulSourceAdapter: SourceAdapter {
    type State: AdapterSessionState;
}

impl SourceExtractionSession {
    /// Creates an extraction session whose imported modules must remain inside `root`.
    pub fn new(root: &Path) -> Self {
        Self::with_registrations(root, &[])
    }

    /// Creates a session that also treats `registrations` as step-registration function names.
    ///
    /// Use this for project-declared wrappers that static inference cannot recognize, such as a
    /// helper that builds its registration call dynamically.
    pub fn with_registrations(root: &Path, registrations: &[String]) -> Self {
        let mut session = Self {
            root: Some(root.to_owned()),
            registrations: registrations.to_vec(),
            ..Self::default()
        };
        // Preserve construction-time root resolution, including a failed canonicalization.
        for registration in SOURCE_ADAPTER_REGISTRY {
            if let Some(initialize) = registration.initialize_session {
                initialize(&mut session);
            }
        }
        session
    }

    pub(crate) fn initialize<T: AdapterSessionState>(&mut self) {
        self.states
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::initialize(self.root.as_deref(), &self.registrations)));
    }

    pub(crate) fn state<T: AdapterSessionState>(&mut self) -> Result<&mut T> {
        // Explicit-root sessions must never establish a boundary later than construction.
        if self.root.is_none() {
            self.initialize::<T>();
        }
        self.states
            .get_mut(&TypeId::of::<T>())
            .and_then(|state| {
                let state: &mut dyn Any = state.as_mut();
                state.downcast_mut::<T>()
            })
            .with_context(|| {
                format!(
                    "source adapter state {} is uninitialized or incompatible",
                    std::any::type_name::<T>()
                )
            })
    }
}

/// Converts one supported definition-source language into the shared definition IR.
pub trait SourceAdapter: Sync {
    /// Stable adapter name used in diagnostics and registry inspection.
    fn name(&self) -> &'static str;

    /// Parser language produced by this adapter registration.
    fn language(&self) -> SourceLanguage;

    /// Extracts definitions from in-memory source using `file` for source locations.
    fn extract(&self, source: &str, file: &SourceFile) -> Result<Extraction>;

    /// Extracts definitions while sharing bounded state with other files in the same run.
    fn extract_with_session(
        &self,
        source: &str,
        file: &SourceFile,
        _session: &mut SourceExtractionSession,
    ) -> Result<Extraction> {
        self.extract(source, file)
    }

    /// Reads and extracts one source file.
    fn extract_file(&self, file: &SourceFile) -> Result<Extraction> {
        let source = read_utf8(&file.path, "definition source", MAX_PROJECT_INPUT_BYTES)?;
        self.extract(&source, file)
    }

    /// Reads and extracts one source file using shared run state.
    fn extract_file_with_session(
        &self,
        file: &SourceFile,
        session: &mut SourceExtractionSession,
    ) -> Result<Extraction> {
        let source = read_utf8(&file.path, "definition source", MAX_PROJECT_INPUT_BYTES)?;
        self.extract_with_session(&source, file, session)
    }
}

/// One suffix-to-adapter registration.
#[non_exhaustive]
#[derive(Clone, Copy)]
pub struct SourceAdapterRegistration {
    /// Filename suffix, including the leading dot.
    pub suffix: &'static str,
    /// Adapter responsible for matching sources.
    pub adapter: &'static dyn SourceAdapter,
    initialize_session: Option<fn(&mut SourceExtractionSession)>,
}

impl SourceAdapterRegistration {
    /// Creates one suffix-to-adapter registration.
    pub const fn new(suffix: &'static str, adapter: &'static dyn SourceAdapter) -> Self {
        Self {
            suffix,
            adapter,
            initialize_session: None,
        }
    }

    const fn with_session<A: StatefulSourceAdapter>(
        suffix: &'static str,
        adapter: &'static A,
    ) -> Self {
        Self {
            suffix,
            adapter,
            initialize_session: Some(SourceExtractionSession::initialize::<A::State>),
        }
    }
}

/// The authoritative language/adapter mapping: one table drives both the suffix registry and the
/// language lookup, so the two routing surfaces cannot drift apart. Adding a language means one
/// row here; the expansion stays an exhaustive match, so routing remains total without a
/// fallible lookup.
macro_rules! register_source_adapters {
    ( $( $language:ident => $adapter:path : $($suffix:literal),+ );+ ; ) => {
        /// Registered definition-source suffixes in deterministic lookup order.
        pub static SOURCE_ADAPTER_REGISTRY: &[SourceAdapterRegistration] = &[
            $(
                $(
                    SourceAdapterRegistration::with_session($suffix, &$adapter),
                )+
            )+
        ];

        /// Returns the adapter for an already classified source language.
        pub fn adapter_for_language(language: SourceLanguage) -> &'static dyn SourceAdapter {
            match language {
                $(SourceLanguage::$language => &$adapter,)+
            }
        }
    };
}

register_source_adapters! {
    JavaScript => crate::typescript::JAVASCRIPT_ADAPTER : ".mjs", ".cjs", ".jsx", ".js";
    TypeScript => crate::typescript::TYPESCRIPT_ADAPTER : ".mts", ".cts", ".ts";
    Tsx => crate::typescript::TSX_ADAPTER : ".tsx";
}

/// Returns the adapter registered for `path`, rejecting declaration and source-map files.
pub fn adapter_for_path(path: &Path) -> Option<&'static dyn SourceAdapter> {
    let extension = path.extension()?.to_str()?;
    if extension == "map" || is_declaration_file(path, extension) {
        return None;
    }
    SOURCE_ADAPTER_REGISTRY
        .iter()
        .find(|registration| extension == registration.suffix.trim_start_matches('.'))
        .map(|registration| registration.adapter)
}

fn is_declaration_file(path: &Path, extension: &str) -> bool {
    matches!(extension, "ts" | "mts" | "cts")
        && path.file_stem().is_some_and(|stem| {
            stem == ".d"
                || Path::new(stem)
                    .extension()
                    .is_some_and(|extension| extension == "d")
        })
}

/// Classifies a path using the registered suffixes.
pub fn language_for_path(path: &Path) -> Option<SourceLanguage> {
    adapter_for_path(path).map(SourceAdapter::language)
}

/// Returns the registered suffixes for tooling and adapter conformance tests.
pub fn registered_suffixes() -> Vec<&'static str> {
    SOURCE_ADAPTER_REGISTRY
        .iter()
        .map(|registration| registration.suffix)
        .collect()
}

#[cfg(test)]
mod tests {
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
            fn initialize(_: Option<&Path>, _: &[String]) -> Self {
                Self(0)
            }
        }
        struct Other;
        impl AdapterSessionState for Other {
            fn initialize(_: Option<&Path>, _: &[String]) -> Self {
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
        let mut first =
            SourceExtractionSession::with_registrations(project.path(), &["SetupA".into()]);
        let mut second =
            SourceExtractionSession::with_registrations(other.path(), &["SetupB".into()]);
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
}
