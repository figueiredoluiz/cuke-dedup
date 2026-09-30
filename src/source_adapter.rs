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

/// Prefix of the diagnostic emitted when a source file cannot be parsed as valid JavaScript or
/// TypeScript. A parse failure hides every definition the file would have contributed, so it is a
/// completeness signal; `--fail-on-unparseable` promotes it to a hard error.
pub(crate) const UNPARSEABLE_SOURCE_DIAGNOSTIC_PREFIX: &str =
    "source contains JavaScript/TypeScript syntax errors";

/// A recognized registration cannot contribute a statically known matcher.
pub(crate) const UNSUPPORTED_MATCHER_DIAGNOSTIC: &str =
    "dynamic or unsupported step matcher cannot be analyzed statically";
pub(crate) const INVALID_MATCHER_DIAGNOSTIC: &str =
    "invalid JavaScript escape sequence in step matcher";

/// Parser language selected for a definition source.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLanguage {
    /// Ruby Cucumber sources selected explicitly by definition globs.
    Ruby,
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
        SourceLanguage::Ruby => tree_sitter_ruby::LANGUAGE.into(),
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

/// Machine-readable effect of an extraction diagnostic, independent of its wording.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionDiagnosticKind {
    /// Compatibility mode used by [`ExtractionDiagnostic::new`]; classify legacy message prefixes.
    LegacyMessage,
    /// A source load whose target has not been included in the source graph.
    Dependency,
    /// An advisory or error that does not itself signal missing definitions.
    Other,
    /// Static extraction may have missed definitions.
    Incomplete,
    /// Source syntax could not be fully parsed; also signals incomplete extraction.
    Unparseable,
}

/// A source-localized problem encountered while extracting definitions.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionDiagnostic {
    /// Machine-readable completeness and parse status.
    pub kind: ExtractionDiagnosticKind,
    /// Diagnostic impact.
    pub level: ExtractionDiagnosticLevel,
    /// Source position of the affected matcher.
    pub location: SourceLocation,
    /// Human-readable explanation.
    pub message: String,
}

impl ExtractionDiagnostic {
    /// Creates a diagnostic with legacy message-prefix classification.
    ///
    /// New adapters should use [`Self::with_kind`] to classify diagnostics independently of text.
    pub fn new(
        level: ExtractionDiagnosticLevel,
        location: SourceLocation,
        message: impl Into<String>,
    ) -> Self {
        Self::with_kind(
            ExtractionDiagnosticKind::LegacyMessage,
            level,
            location,
            message,
        )
    }

    /// Creates an explicitly classified diagnostic without inferring its effect from its message.
    pub fn with_kind(
        kind: ExtractionDiagnosticKind,
        level: ExtractionDiagnosticLevel,
        location: SourceLocation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            level,
            location,
            message: message.into(),
        }
    }
}

pub(crate) fn is_completeness_diagnostic(diagnostic: &ExtractionDiagnostic) -> bool {
    match diagnostic.kind {
        ExtractionDiagnosticKind::Incomplete
        | ExtractionDiagnosticKind::Unparseable
        | ExtractionDiagnosticKind::Dependency => true,
        ExtractionDiagnosticKind::Other => false,
        ExtractionDiagnosticKind::LegacyMessage => {
            diagnostic
                .message
                .starts_with(UNRESOLVED_REGISTRATION_DIAGNOSTIC_PREFIX)
                || matches!(
                    diagnostic.message.as_str(),
                    UNSUPPORTED_MATCHER_DIAGNOSTIC | INVALID_MATCHER_DIAGNOSTIC
                )
                || is_unparseable_diagnostic(diagnostic)
        }
    }
}

/// Reports a parse failure separately from other causes of incomplete extraction.
pub(crate) fn is_unparseable_diagnostic(diagnostic: &ExtractionDiagnostic) -> bool {
    match diagnostic.kind {
        ExtractionDiagnosticKind::Unparseable => true,
        ExtractionDiagnosticKind::LegacyMessage => diagnostic
            .message
            .starts_with(UNPARSEABLE_SOURCE_DIAGNOSTIC_PREFIX),
        ExtractionDiagnosticKind::Other
        | ExtractionDiagnosticKind::Incomplete
        | ExtractionDiagnosticKind::Dependency => false,
    }
}

/// A statically resolved source-loading edge.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDependency {
    /// Source location of the loading call.
    pub location: SourceLocation,
    /// Canonical Ruby source included by that call.
    pub target: PathBuf,
}

impl SourceDependency {
    /// Records a loading call and its resolved source.
    pub fn new(location: SourceLocation, target: PathBuf) -> Self {
        Self { location, target }
    }
}

/// Potential Ruby step invocations, separate from concrete Gherkin execution evidence.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndirectStepUsage {
    /// Statically decoded possible step texts, including deferred calls.
    pub texts: std::collections::BTreeSet<String>,
    /// An unresolved invocation prevents proving any Ruby definition unused.
    pub unknown: bool,
}

impl IndirectStepUsage {
    /// Combines source metadata; absent metadata is conservatively unknown.
    pub fn extend(&mut self, other: Option<Self>) {
        match other {
            Some(other) => {
                self.texts.extend(other.texts);
                self.unknown |= other.unknown;
            }
            None => self.unknown = true,
        }
    }
}

/// A source-declared parameter type, resolved without executing its transformer.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceParameterType {
    /// Literal registry name, or unknown when static name resolution failed.
    pub name: Option<String>,
    /// Validated Rust-compatible matching expression; absent for an unresolved declaration.
    pub expression: Option<String>,
    /// Location of the declaration, used to diagnose collisions across files.
    pub location: SourceLocation,
}

impl SourceParameterType {
    /// Constructs source metadata; unresolved names or expressions must retain uncertainty.
    pub fn new(name: Option<String>, expression: Option<String>, location: SourceLocation) -> Self {
        Self {
            name,
            expression,
            location,
        }
    }
}

/// Definitions and non-fatal diagnostics extracted from one source.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extraction {
    /// Successfully extracted definitions.
    pub definitions: Vec<StepDefinition>,
    /// Parameter declarations; merge the complete source registry before matching expressions.
    pub parameter_types: Vec<SourceParameterType>,
    /// Optional indirect usage metadata; merge across every selected Ruby source.
    pub indirect_usage: Option<IndirectStepUsage>,
    /// Problems that did not prevent extraction of the rest of the file.
    pub diagnostics: Vec<ExtractionDiagnostic>,
}

impl Extraction {
    /// Creates an extraction result from definitions and non-fatal diagnostics.
    pub fn new(definitions: Vec<StepDefinition>, diagnostics: Vec<ExtractionDiagnostic>) -> Self {
        Self {
            definitions,
            diagnostics,
            indirect_usage: None,
            parameter_types: Vec::new(),
        }
    }
}

/// Reusable state for extracting multiple definition sources in one analysis run.
///
/// Reusing a session shares bounded parser and module-resolution work across files. Create one
/// session per analysis root rather than sharing it between unrelated repositories.
#[derive(Default)]
pub struct SourceExtractionSession {
    dependencies: std::collections::BTreeSet<(PathBuf, usize, usize)>,
    root: Option<PathBuf>,
    registrations: Vec<String>,
    assertion_modules: Vec<String>,
    states: HashMap<TypeId, Box<dyn Any + Send + Sync + UnwindSafe + RefUnwindSafe>>,
}

/// Backend-owned state; related syntax variants can share the same state type.
/// The bounds preserve the public session's existing thread and unwind-safety guarantees.
pub(crate) trait AdapterSessionState:
    Any + Send + Sync + UnwindSafe + RefUnwindSafe
{
    fn initialize(
        root: Option<&Path>,
        registrations: &[String],
        assertion_modules: &[String],
    ) -> Self;
}

/// Internal stateful registrations must explicitly select their backend state.
pub(crate) trait StatefulSourceAdapter: SourceAdapter {
    type State: AdapterSessionState;
}

impl SourceExtractionSession {
    /// Supplies resolved loading edges from discovery without executing dependencies.
    pub fn with_dependencies(mut self, dependencies: &[SourceDependency]) -> Self {
        self.dependencies.extend(dependencies.iter().map(|edge| {
            (
                edge.location.path.clone(),
                edge.location.line,
                edge.location.column,
            )
        }));
        self
    }

    pub(crate) fn dependency_resolved(&self, location: &SourceLocation) -> bool {
        self.dependencies
            .contains(&(location.path.clone(), location.line, location.column))
    }

    /// Creates an extraction session whose imported modules must remain inside `root`.
    pub fn new(root: &Path) -> Self {
        Self::with_registrations(root, &[])
    }

    /// Creates a session that also treats `registrations` as step-registration function names.
    ///
    /// Use this for project-declared wrappers that static inference cannot recognize, such as a
    /// helper that builds its registration call dynamically.
    pub fn with_registrations(root: &Path, registrations: &[String]) -> Self {
        Self::with_options(root, registrations, &[])
    }

    /// Creates a session that also trusts `assertion_modules` as assertion-factory providers.
    ///
    /// Use this for a local module that re-exports `expect` from a supported assertion package, or
    /// for a runner this build does not recognize by name. Without it such a factory is untrusted
    /// and its expected values are not treated as behaviour.
    pub fn with_options(
        root: &Path,
        registrations: &[String],
        assertion_modules: &[String],
    ) -> Self {
        let mut session = Self {
            root: Some(root.to_owned()),
            registrations: registrations.to_vec(),
            assertion_modules: assertion_modules.to_vec(),
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
        self.states.entry(TypeId::of::<T>()).or_insert_with(|| {
            Box::new(T::initialize(
                self.root.as_deref(),
                &self.registrations,
                &self.assertion_modules,
            ))
        });
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
    Ruby => crate::ruby::RUBY_ADAPTER : ".rb";
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
