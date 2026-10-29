//! Definition-source adapter contracts and extension-based routing.

mod config;
mod finalization;
mod registry_validation;
pub(crate) use config::FrontendConfig;
// Every frontend's keys of one configuration source, flattened into the core config. Only Ruby
// has keys today. A wrapper struct flattening it would nest two flattens, and serde then rejects
// every frontend key under the core's `deny_unknown_fields`, so a second frontend's keys need a
// single flattened struct, not a wrapper.
pub(crate) use crate::ruby::config::RawRubyConfig as RawFrontendConfig;
pub(crate) mod semantics;
pub(crate) mod suppression;
pub use crate::framework_descriptor::{
    DiscoveredFeatures, DiscoveryInput, FeatureDiscovery, FrameworkDescriptor,
};
pub use crate::matcher::{IdentityFlags, RegexKey, TranslatedRegex};
pub use finalization::{
    SourceAdvisory, SourceFinalization, SourceUncertainty, UncertaintyCause, UncertaintyScope,
};
pub use registry_validation::RegistryError;
pub use semantics::{
    AnalysisPolicy, DomainSpec, HandlerScope, RegexDialect, StepMatching, StreamEvidence,
};

use crate::model::{Framework, SourceLocation, StepDefinition};
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

/// Parser language of a definition source: an open id that the adapter parsing the language
/// declares through [`SourceAdapter::language`]. Two values are equal when their ids are, so
/// `SourceLanguage::new("ruby")` equals [`crate::ruby::RUBY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceLanguage(&'static str);

impl SourceLanguage {
    /// A language with the given id.
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    /// The language id.
    pub const fn id(self) -> &'static str {
        self.0
    }
}

/// Frontend-owned reason for excluding a discovered source before extraction.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceExclusion {
    /// A compressed container prefix.
    Compressed,
    /// A binary-source prefix.
    Binary,
    /// Generated or minified source geometry.
    Minified,
}

impl SourceExclusion {
    /// Stable diagnostic label for this exclusion.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compressed => "compressed",
            Self::Binary => "binary",
            Self::Minified => "minified",
        }
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

/// Whether `diagnostic` signals possibly missed definitions: kinds `Incomplete`, `Unparseable` and
/// `Dependency`. A `LegacyMessage` diagnostic is classified by its message text.
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
    /// Canonical source included by that call.
    pub target: PathBuf,
    /// True only when discovery proves this target was not independently selected.
    pub dependency_only: bool,
}

impl SourceDependency {
    /// Supplies explicit dependency-only origin evidence; unknown origins must remain false.
    pub fn with_dependency_only_target(mut self, dependency_only: bool) -> Self {
        self.dependency_only = dependency_only;
        self
    }

    /// Records a loading call and its resolved source.
    pub fn new(location: SourceLocation, target: PathBuf) -> Self {
        Self {
            location,
            target,
            dependency_only: false,
        }
    }
}

/// A source-loading call that discovery deliberately did not follow, with the reason.
///
/// Extraction keeps the call's dependency diagnostic, so analysis stays incomplete; the reason
/// replaces the generic "unresolved" wording at that location.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyGap {
    /// Source location of the loading call.
    pub location: SourceLocation,
    /// Why the target was not admitted, such as an exceeded resource limit.
    pub reason: String,
}

impl DependencyGap {
    /// Records a loading call that discovery refused to follow.
    pub fn new(location: SourceLocation, reason: impl Into<String>) -> Self {
        Self {
            location,
            reason: reason.into(),
        }
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
    /// Parameter declarations; merge them across every definition file, in order, with
    /// [`crate::analysis::merge_parameter_types`] before matching expressions.
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
    dependencies: std::collections::BTreeMap<(PathBuf, usize, usize), SourceDependency>,
    dependency_gaps: std::collections::BTreeMap<(PathBuf, usize, usize), String>,
    root: Option<PathBuf>,
    registrations: Vec<String>,
    assertion_modules: Vec<String>,
    states: HashMap<TypeId, Box<dyn Any + Send + Sync + UnwindSafe + RefUnwindSafe>>,
    adapters: AdapterRegistry,
}

/// Per-run state an adapter keeps in a [`SourceExtractionSession`], such as caches shared across
/// files; adapters of related syntax variants can share one state type. The bounds preserve the
/// session's thread and unwind-safety guarantees.
pub trait AdapterSessionState: Any + Send + Sync + UnwindSafe + RefUnwindSafe {
    /// Builds the backend's per-run state from the session root (`None` for a default session),
    /// configured registrations and assertion modules. Infallible; called at most once per state
    /// type per session, at construction for a rooted session and on first access otherwise.
    fn initialize(
        root: Option<&Path>,
        registrations: &[String],
        assertion_modules: &[String],
    ) -> Self;
}

/// An adapter with per-run session state and optional dependency discovery. Register it with
/// [`SourceAdapterRegistration::with_session`]; read its state with
/// [`SourceExtractionSession::state`].
pub trait StatefulSourceAdapter: SourceAdapter {
    /// The session state this adapter reads.
    type State: AdapterSessionState;
    /// Adds loaded sources, resolved edges and refused-load gaps to `files`, without executing them.
    /// Called once per adapter name when a selected definition has its language; whole-graph
    /// failures go into `files.errors`. `excludes` matches root-relative paths the configuration
    /// excludes. The default does nothing.
    fn discover_dependencies(
        _config: &crate::config::Config,
        _excludes: &crate::discovery::ExcludeSet,
        _files: &mut crate::discovery::DiscoveredFiles,
    ) {
    }
}

impl SourceExtractionSession {
    /// Supplies resolved loading edges from discovery without executing dependencies.
    pub fn with_dependencies(mut self, dependencies: &[SourceDependency]) -> Self {
        self.dependencies.extend(dependencies.iter().map(|edge| {
            (
                (
                    edge.location.path.clone(),
                    edge.location.line,
                    edge.location.column,
                ),
                edge.clone(),
            )
        }));
        self
    }

    /// Finalizes all selected frontends after extraction and applies cross-file trust effects.
    ///
    /// Call this before analyzing accumulated definitions. A later source can invalidate an
    /// earlier registration; individual extraction results are provisional until this operation.
    pub fn finalize(
        &mut self,
        files: &[SourceFile],
        definitions: &mut Vec<StepDefinition>,
    ) -> Result<SourceFinalization> {
        let mut result = SourceFinalization::default();
        let mut completed = std::collections::BTreeSet::new();
        for file in files {
            let adapter = self.adapters.registered_adapter(file.language)?;
            if completed.insert(adapter.name()) {
                let finalization = adapter.finalize_session(self)?;
                result.uncertainties.extend(finalization.uncertainties);
                result.advisories.extend(finalization.advisories);
            }
        }
        result.apply_with(&self.adapters, definitions);
        Ok(result)
    }

    /// Supplies loading calls that discovery refused to follow, so their diagnostics name the cause.
    pub fn with_dependency_gaps(mut self, gaps: &[DependencyGap]) -> Self {
        self.dependency_gaps.extend(gaps.iter().map(|gap| {
            (
                (
                    gap.location.path.clone(),
                    gap.location.line,
                    gap.location.column,
                ),
                gap.reason.clone(),
            )
        }));
        self
    }

    /// The reason discovery refused to follow the loading call at `location`, if it did.
    pub fn dependency_gap(&self, location: &SourceLocation) -> Option<&str> {
        self.dependency_gaps
            .get(&(location.path.clone(), location.line, location.column))
            .map(String::as_str)
    }

    /// Whether discovery supplied a resolved edge for the loading call at `location`, matched by
    /// path, start line and start column only.
    pub fn dependency_resolved(&self, location: &SourceLocation) -> bool {
        self.dependencies
            .contains_key(&(location.path.clone(), location.line, location.column))
    }

    /// Clones of the resolved loading edges, ordered by call path, line and column.
    pub fn dependency_edges(&self) -> Vec<SourceDependency> {
        self.dependencies.values().cloned().collect()
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
        Self::with_adapters(
            root,
            registrations,
            assertion_modules,
            AdapterRegistry::default(),
        )
    }

    /// Creates a session for `config`: its root, registrations, assertion modules and adapters.
    pub fn for_config(config: &crate::config::Config) -> Self {
        Self::with_adapters(
            &config.root,
            &config.registrations,
            &config.assertion_modules,
            config.adapters().clone(),
        )
    }

    /// Creates a rooted session whose adapters come from `adapters`.
    fn with_adapters(
        root: &Path,
        registrations: &[String],
        assertion_modules: &[String],
        adapters: AdapterRegistry,
    ) -> Self {
        let mut session = Self {
            root: Some(root.to_owned()),
            registrations: registrations.to_vec(),
            assertion_modules: assertion_modules.to_vec(),
            adapters: adapters.clone(),
            ..Self::default()
        };
        // Preserve construction-time root resolution, including a failed canonicalization.
        adapters.initialize_session(&mut session);
        session
    }

    /// Creates the `T` state for this session unless one already exists for that type.
    pub(crate) fn initialize<T: AdapterSessionState>(&mut self) {
        self.states.entry(TypeId::of::<T>()).or_insert_with(|| {
            Box::new(T::initialize(
                self.root.as_deref(),
                &self.registrations,
                &self.assertion_modules,
            ))
        });
    }

    /// Returns this session's `T` state. A session without a root creates it on first access; a
    /// rooted session errors when `T` was not initialized at construction. Also errors when the
    /// stored value is not a `T`. [`SourceExtractionSession::for_config`] initializes, at
    /// construction, the state of every registration of the configuration's registry whose hooks
    /// belong to its adapter (see [`SourceAdapterRegistration::with_session`]); `new`,
    /// `with_registrations` and `with_options` read the built-in registry.
    pub fn state<T: AdapterSessionState>(&mut self) -> Result<&mut T> {
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
pub trait SourceAdapter: Sync + RefUnwindSafe {
    /// Stable adapter name used in diagnostics and registry inspection.
    fn name(&self) -> &'static str;

    /// Parser language produced by this adapter registration.
    fn language(&self) -> SourceLanguage;

    /// Extracts definitions from in-memory source using `file` for source locations.
    fn extract(&self, source: &str, file: &SourceFile) -> Result<Extraction>;

    /// Whether this frontend participates when no definition globs are configured.
    fn discover_by_default(&self) -> bool {
        true
    }

    /// Classifies a source using this frontend's bounded input-filter policy.
    fn inspect_source(
        &self,
        _file: &SourceFile,
        _registrations: &[String],
    ) -> Option<SourceExclusion> {
        None
    }

    /// The comparison policy this adapter attaches to the definitions it extracts. The default is
    /// [`AnalysisPolicy::default`]; a framework-specific policy is attached per definition.
    fn analysis_policy(&self) -> AnalysisPolicy {
        AnalysisPolicy::default()
    }

    /// The frameworks this adapter attributes definitions to, in precedence order. Their ids are
    /// the only ones, besides [`Framework::UNKNOWN`], that a serialized definition may carry.
    fn framework_contracts(&self) -> &'static [FrameworkContract] {
        &[]
    }

    /// Default exclude globs this adapter's ecosystem needs whatever the framework, such as a
    /// package-manager directory. Framework-specific globs belong to [`FrameworkContract`].
    fn default_excludes(&self) -> &'static [&'static str] {
        &[]
    }

    /// Whether missing source metadata or reads leave indirect step usage unknown.
    fn supports_indirect_usage(&self) -> bool {
        false
    }

    /// Prepares selected-source evidence before extraction, without executing source code.
    fn prepare_session(
        &self,
        _files: &[SourceFile],
        _session: &mut SourceExtractionSession,
    ) -> Result<()> {
        Ok(())
    }

    /// Reports frontend-owned trust effects after every selected source has been extracted.
    fn finalize_session(
        &self,
        _session: &mut SourceExtractionSession,
    ) -> Result<SourceFinalization> {
        Ok(SourceFinalization::default())
    }

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

type DependencyDiscovery = fn(
    &crate::config::Config,
    &crate::discovery::ExcludeSet,
    &mut crate::discovery::DiscoveredFiles,
);

/// The session hooks of a stateful registration and the adapter they were made for.
#[derive(Clone, Copy)]
struct SessionHooks {
    adapter: &'static dyn SourceAdapter,
    initialize: fn(&mut SourceExtractionSession),
    discover_dependencies: DependencyDiscovery,
}

/// A framework an adapter recognizes: its report id, the feature discovery its configuration
/// supports, the generated or report directories it writes and the comparison policy its
/// definitions carry. An adapter lists its contracts in precedence order; shared code reads them in
/// registry order and never names a framework.
pub struct FrameworkContract {
    /// The id definitions attributed to this framework carry in reports.
    pub framework: Framework,
    pub(crate) discovery: Option<FrameworkDescriptor>,
    pub(crate) excludes: &'static [&'static str],
    /// `None` keeps the adapter's [`SourceAdapter::analysis_policy`].
    pub(crate) policy: Option<AnalysisPolicy>,
}

impl FrameworkContract {
    /// A contract for `framework` with no feature discovery and no exclude globs.
    pub const fn new(framework: Framework) -> Self {
        Self {
            framework,
            discovery: None,
            excludes: &[],
            policy: None,
        }
    }

    /// This contract with `excludes` as the framework's default exclude globs.
    pub const fn with_excludes(self, excludes: &'static [&'static str]) -> Self {
        Self { excludes, ..self }
    }

    /// This contract with `discovery` as its feature discovery. Discovery runs only when the
    /// configuration names no feature patterns, over every registered contract in registry order,
    /// and the first match wins: a registry listing the built-in registrations first reaches a
    /// caller contract only when no built-in one matches.
    pub const fn with_discovery(self, discovery: FrameworkDescriptor) -> Self {
        Self {
            discovery: Some(discovery),
            ..self
        }
    }

    /// This contract with `policy` in place of the adapter's default comparison policy.
    pub const fn with_policy(self, policy: AnalysisPolicy) -> Self {
        Self {
            policy: Some(policy),
            ..self
        }
    }

    /// The comparison policy of `framework` among `contracts`, or `default` when no contract
    /// declares that framework or its contract keeps the adapter's policy.
    pub fn policy_of(
        contracts: &[Self],
        framework: Framework,
        default: AnalysisPolicy,
    ) -> AnalysisPolicy {
        contracts
            .iter()
            .find(|contract| contract.framework == framework)
            .and_then(|contract| contract.policy)
            .unwrap_or(default)
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
    hooks: Option<SessionHooks>,
}

impl SourceAdapterRegistration {
    /// Creates one suffix-to-adapter registration.
    pub const fn new(suffix: &'static str, adapter: &'static dyn SourceAdapter) -> Self {
        Self {
            suffix,
            adapter,
            hooks: None,
        }
    }

    /// Creates a registration for `adapter` carrying its session-state initializer and its
    /// [`StatefulSourceAdapter::discover_dependencies`] hook. The hooks belong to `adapter`:
    /// replacing the registration's `adapter` drops them. Objects are told apart by address and
    /// name, and zero-sized adapters of different types may share an address, so give each adapter
    /// its own name.
    pub const fn with_session<A: StatefulSourceAdapter>(
        suffix: &'static str,
        adapter: &'static A,
    ) -> Self {
        Self {
            suffix,
            adapter,
            hooks: Some(SessionHooks {
                adapter,
                initialize: SourceExtractionSession::initialize::<A::State>,
                discover_dependencies: A::discover_dependencies,
            }),
        }
    }

    /// This registration with the session hooks made for its adapter object: its own when they
    /// were, else those of the built-in registration of the same adapter, else none. Hooks follow
    /// the adapter, so a built-in adapter registered under another suffix keeps them and a
    /// replaced `adapter` drops the hooks of the one it replaced.
    fn with_owned_hooks(self) -> Self {
        let owns = |hooks: &SessionHooks| {
            std::ptr::addr_eq(hooks.adapter, self.adapter)
                && hooks.adapter.name() == self.adapter.name()
        };
        let hooks = self.hooks.filter(owns).or_else(|| {
            SOURCE_ADAPTER_REGISTRY
                .iter()
                .find_map(|builtin| builtin.hooks.filter(owns))
        });
        Self { hooks, ..self }
    }
}

/// The authoritative adapter table: one row per adapter, which declares its own language. The
/// suffix registry and the language lookup both read it, so the two routing surfaces cannot
/// drift apart.
macro_rules! register_source_adapters {
    ( $( $adapter:path : $($suffix:literal),+ );+ ; ) => {
        /// Registered definition-source suffixes in deterministic lookup order.
        pub(crate) static SOURCE_ADAPTER_REGISTRY: &[SourceAdapterRegistration] = &[
            $(
                $(
                    SourceAdapterRegistration::with_session($suffix, &$adapter),
                )+
            )+
        ];
    };
}

register_source_adapters! {
    crate::ruby::RUBY_ADAPTER : ".rb";
    crate::typescript::JAVASCRIPT_ADAPTER : ".mjs", ".cjs", ".jsx", ".js";
    crate::typescript::TYPESCRIPT_ADAPTER : ".mts", ".cts", ".ts";
    crate::typescript::TSX_ADAPTER : ".tsx";
}

/// A set of source-adapter registrations that routes paths and languages to adapters. Discovery,
/// configuration defaults and extraction sessions read the registry they are given:
/// [`crate::config::Config::load_with_adapters`] attaches one and
/// [`SourceExtractionSession::for_config`] carries it. [`AdapterRegistry::builtin`] holds the
/// adapters this crate ships; [`AdapterRegistry::new`] validates any other set.
#[derive(Clone)]
pub struct AdapterRegistry {
    registrations: std::borrow::Cow<'static, [SourceAdapterRegistration]>,
}

static BUILTIN_REGISTRY: AdapterRegistry = AdapterRegistry {
    registrations: std::borrow::Cow::Borrowed(SOURCE_ADAPTER_REGISTRY),
};

impl AdapterRegistry {
    /// The adapters this crate ships.
    pub fn builtin() -> &'static Self {
        &BUILTIN_REGISTRY
    }

    /// A registry of `registrations` in lookup order, or the first conflict among them. To extend
    /// or replace built-in adapters, start from [`AdapterRegistry::builtin`]'s
    /// [`registrations`](Self::registrations): a suffix, language or adapter name that collides
    /// with a built-in one is rejected, not overridden.
    pub fn new(
        registrations: impl IntoIterator<Item = SourceAdapterRegistration>,
    ) -> std::result::Result<Self, RegistryError> {
        let registrations: Vec<_> = registrations
            .into_iter()
            .map(SourceAdapterRegistration::with_owned_hooks)
            .collect();
        registry_validation::validate(&registrations)?;
        Ok(Self {
            registrations: std::borrow::Cow::Owned(registrations),
        })
    }

    /// The registrations in lookup order.
    pub fn registrations(&self) -> &[SourceAdapterRegistration] {
        &self.registrations
    }

    /// The registered adapter that declares `language`, or `None` when no adapter does.
    pub fn adapter_for_language(
        &self,
        language: SourceLanguage,
    ) -> Option<&'static dyn SourceAdapter> {
        self.registrations
            .iter()
            .map(|registration| registration.adapter)
            .find(|adapter| adapter.language() == language)
    }

    /// The registered adapter that declares `language`, or an error naming the language.
    pub fn registered_adapter(
        &self,
        language: SourceLanguage,
    ) -> Result<&'static dyn SourceAdapter> {
        self.adapter_for_language(language).with_context(|| {
            format!(
                "no source adapter is registered for language `{}`",
                language.0
            )
        })
    }

    /// The adapter registered for `path`. A `.map` file and a TypeScript declaration file
    /// (`.d.ts`, `.d.mts`, `.d.cts`) route to no adapter, whichever adapter claims the suffix.
    pub fn adapter_for_path(&self, path: &Path) -> Option<&'static dyn SourceAdapter> {
        let extension = path.extension()?.to_str()?;
        if extension == "map" || is_declaration_file(path, extension) {
            return None;
        }
        self.registrations
            .iter()
            .find(|registration| extension == registration.suffix.trim_start_matches('.'))
            .map(|registration| registration.adapter)
    }

    /// The language of the adapter registered for `path`.
    pub fn language_for_path(&self, path: &Path) -> Option<SourceLanguage> {
        self.adapter_for_path(path).map(SourceAdapter::language)
    }

    /// The registered suffixes in lookup order.
    pub fn suffixes(&self) -> Vec<&'static str> {
        self.registrations
            .iter()
            .map(|registration| registration.suffix)
            .collect()
    }

    /// Comparison domain of `language`'s definitions, as its registered adapter declares it;
    /// `None` when no registered adapter declares the language.
    pub(crate) fn comparison_domain(
        &self,
        language: SourceLanguage,
    ) -> Option<&'static semantics::DomainSpec> {
        self.adapter_for_language(language)
            .map(|adapter| adapter.analysis_policy().domain())
    }

    /// Every registered framework contract, in registry order and then each adapter's precedence
    /// order. Adapter types serving several suffixes are read once: a repeated id is skipped.
    pub(crate) fn framework_contracts(
        &self,
    ) -> impl Iterator<Item = &'static FrameworkContract> + '_ {
        let mut seen = Vec::new();
        self.registrations
            .iter()
            .flat_map(|registration| registration.adapter.framework_contracts())
            .filter(move |contract| {
                let first = !seen.contains(&contract.framework);
                seen.push(contract.framework);
                first
            })
    }

    /// `core` followed by the default exclude globs every registered adapter declares, its own
    /// before its frameworks', in registry order without repeats.
    pub(crate) fn default_excludes(&self, core: &[&'static str]) -> Vec<&'static str> {
        let mut excludes = core.to_vec();
        for registration in self.registrations.iter() {
            let adapter = registration.adapter;
            let own = adapter.default_excludes().iter();
            let frameworks = adapter
                .framework_contracts()
                .iter()
                .flat_map(|contract| contract.excludes);
            for glob in own.chain(frameworks) {
                if !excludes.contains(glob) {
                    excludes.push(*glob);
                }
            }
        }
        excludes
    }

    /// Runs the session initializer of every registration on `session`.
    fn initialize_session(&self, session: &mut SourceExtractionSession) {
        for initialize in self
            .registrations
            .iter()
            .filter_map(|registration| registration.hooks.map(|hooks| hooks.initialize))
        {
            initialize(session);
        }
    }

    /// Runs each selected adapter's dependency discovery once, in registry order, then restores
    /// discovery's ordering: definitions sorted and unique by path, and no admitted file left
    /// among the skipped ones.
    pub(crate) fn discover_dependencies(
        &self,
        config: &crate::config::Config,
        excludes: &crate::discovery::ExcludeSet,
        files: &mut crate::discovery::DiscoveredFiles,
    ) {
        let mut completed = std::collections::BTreeSet::new();
        let callbacks: Vec<_> = self
            .registrations
            .iter()
            .filter(|entry| {
                files
                    .definitions
                    .iter()
                    .any(|file| file.language == entry.adapter.language())
            })
            // A registration without hooks must not claim the adapter's one run.
            .filter_map(|entry| entry.hooks.map(|hooks| (entry.adapter.name(), hooks)))
            .filter(|(name, _)| completed.insert(*name))
            .map(|(_, hooks)| hooks.discover_dependencies)
            .collect();
        for callback in callbacks {
            callback(config, excludes, files);
        }
        files
            .definitions
            .sort_by(|left, right| left.path.cmp(&right.path));
        files
            .definitions
            .dedup_by(|left, right| left.path == right.path);
        let admitted: std::collections::BTreeSet<_> =
            files.definitions.iter().map(|file| &file.path).collect();
        files
            .skipped_definitions
            .retain(|file| !admitted.contains(&file.path));
    }
}

impl Default for AdapterRegistry {
    /// The built-in registry.
    fn default() -> Self {
        Self::builtin().clone()
    }
}

impl std::fmt::Debug for AdapterRegistry {
    /// Lists each registration as its suffix and adapter name.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_map()
            .entries(
                self.registrations
                    .iter()
                    .map(|registration| (registration.suffix, registration.adapter.name())),
            )
            .finish()
    }
}

/// Returns the built-in adapter that declares `language`, or `None` when none does.
pub fn adapter_for_language(language: SourceLanguage) -> Option<&'static dyn SourceAdapter> {
    AdapterRegistry::builtin().adapter_for_language(language)
}

/// Returns the built-in adapter that declares `language`, or an error naming the language.
pub(crate) fn registered_adapter(language: SourceLanguage) -> Result<&'static dyn SourceAdapter> {
    AdapterRegistry::builtin().registered_adapter(language)
}

/// Returns the built-in adapter registered for `path`, rejecting declaration and source-map files.
pub fn adapter_for_path(path: &Path) -> Option<&'static dyn SourceAdapter> {
    AdapterRegistry::builtin().adapter_for_path(path)
}

/// Whether `path` is a TypeScript declaration file: extension `ts`, `mts` or `cts` and a stem that
/// is `.d` or ends in `.d`. Comparisons are case-sensitive.
fn is_declaration_file(path: &Path, extension: &str) -> bool {
    matches!(extension, "ts" | "mts" | "cts")
        && path.file_stem().is_some_and(|stem| {
            stem == ".d"
                || Path::new(stem)
                    .extension()
                    .is_some_and(|extension| extension == "d")
        })
}

/// Classifies a path using the built-in suffixes.
pub fn language_for_path(path: &Path) -> Option<SourceLanguage> {
    AdapterRegistry::builtin().language_for_path(path)
}

/// Returns the built-in suffixes for tooling and adapter conformance tests.
pub fn registered_suffixes() -> Vec<&'static str> {
    AdapterRegistry::builtin().suffixes()
}

#[cfg(test)]
mod tests;
