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
mod tests;
