//! Definition-source adapter contracts and extension-based routing.

use crate::model::{SourceLocation, StepDefinition};
use crate::resource_limits::{read_utf8, MAX_PROJECT_INPUT_BYTES};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub(crate) const UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX: &str =
    "unresolved step-registration calls:";

/// Parser language selected for a definition source.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionDiagnosticLevel {
    /// Analysis continues, but a static-analysis limitation applies.
    Warning,
    /// The source contains a malformed step registration.
    Error,
}

/// A source-localized problem encountered while extracting definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionDiagnostic {
    /// Diagnostic impact.
    pub level: ExtractionDiagnosticLevel,
    /// Source position of the affected matcher.
    pub location: SourceLocation,
    /// Human-readable explanation.
    pub message: String,
}

pub(crate) fn is_completeness_diagnostic(diagnostic: &ExtractionDiagnostic) -> bool {
    diagnostic
        .message
        .starts_with(UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX)
}

/// Definitions and non-fatal diagnostics extracted from one source.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extraction {
    /// Successfully extracted definitions.
    pub definitions: Vec<StepDefinition>,
    /// Problems that did not prevent extraction of the rest of the file.
    pub diagnostics: Vec<ExtractionDiagnostic>,
}

/// Reusable state for extracting multiple definition sources in one analysis run.
///
/// Reusing a session shares bounded parser and module-resolution work across files. Create one
/// session per analysis root rather than sharing it between unrelated repositories.
#[derive(Default)]
pub struct SourceExtractionSession {
    pub(crate) typescript: crate::typescript::TypeScriptExtractionSession,
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
        Self {
            typescript: crate::typescript::TypeScriptExtractionSession::for_root(
                root,
                registrations,
            ),
        }
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
#[derive(Clone, Copy)]
pub struct SourceAdapterRegistration {
    /// Filename suffix, including the leading dot.
    pub suffix: &'static str,
    /// Adapter responsible for matching sources.
    pub adapter: &'static dyn SourceAdapter,
}

/// Registered definition-source suffixes in deterministic lookup order.
pub static SOURCE_ADAPTER_REGISTRY: [SourceAdapterRegistration; 8] = [
    SourceAdapterRegistration {
        suffix: ".mjs",
        adapter: &crate::typescript::JAVASCRIPT_ADAPTER,
    },
    SourceAdapterRegistration {
        suffix: ".cjs",
        adapter: &crate::typescript::JAVASCRIPT_ADAPTER,
    },
    SourceAdapterRegistration {
        suffix: ".jsx",
        adapter: &crate::typescript::JAVASCRIPT_ADAPTER,
    },
    SourceAdapterRegistration {
        suffix: ".js",
        adapter: &crate::typescript::JAVASCRIPT_ADAPTER,
    },
    SourceAdapterRegistration {
        suffix: ".mts",
        adapter: &crate::typescript::TYPESCRIPT_ADAPTER,
    },
    SourceAdapterRegistration {
        suffix: ".cts",
        adapter: &crate::typescript::TYPESCRIPT_ADAPTER,
    },
    SourceAdapterRegistration {
        suffix: ".tsx",
        adapter: &crate::typescript::TSX_ADAPTER,
    },
    SourceAdapterRegistration {
        suffix: ".ts",
        adapter: &crate::typescript::TYPESCRIPT_ADAPTER,
    },
];

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

/// Returns the adapter for an already classified source language.
pub fn adapter_for_language(language: SourceLanguage) -> &'static dyn SourceAdapter {
    match language {
        SourceLanguage::JavaScript => &crate::typescript::JAVASCRIPT_ADAPTER,
        SourceLanguage::TypeScript => &crate::typescript::TYPESCRIPT_ADAPTER,
        SourceLanguage::Tsx => &crate::typescript::TSX_ADAPTER,
    }
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
}
