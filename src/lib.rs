//! CukeDedup's reusable static-analysis library.
//!
//! The CLI is the primary integration for repository-wide analysis. Embedders can also parse
//! definitions and feature steps into the shared representation and run the same rules directly:
//!
//! ```
//! use cuke_dedup::analysis::analyze;
//! use cuke_dedup::config::{Config, ConfigOverrides};
//! use cuke_dedup::gherkin;
//! use cuke_dedup::source_adapter::SourceFile;
//! use cuke_dedup::typescript;
//! use std::path::Path;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let root = std::env::current_dir()?;
//! let config = Config::load(&root, ConfigOverrides::default())?;
//! let definition_file = SourceFile {
//!     path: root.join("steps.ts"),
//!     language: typescript::TYPESCRIPT,
//! };
//! let definitions = typescript::extract(
//!     r#"import { Given } from "@cucumber/cucumber";
//! Given("a user exists", async () => { await createUser(); });"#,
//!     &definition_file,
//! )?;
//! let feature_steps = gherkin::extract(
//!     "Feature: Users\n  Scenario: Existing user\n    Given a user exists\n",
//!     Path::new("features/users.feature"),
//! )?;
//!
//! let result = analyze(definitions, feature_steps, &config)?;
//! assert!(result.findings.is_empty());
//! # Ok(())
//! # }
//! ```
//!
//! ## Public API stability
//!
//! Extensibility follows the direction the data travels, and the two halves are deliberately
//! different.
//!
//! Types the analyzer *produces* carry `#[non_exhaustive]`, so a later release can add a field
//! without breaking embedders. Where a caller still needs to build one — to assemble a partial
//! result, a diagnostic, or a baseline — the type also exposes a `new` constructor that takes the
//! fields required at that version.
//!
//! Types embedders *construct* to feed the analyzer stay exhaustive, because `#[non_exhaustive]`
//! would make them impossible to build from outside this crate. That covers
//! [`source_adapter::SourceFile`], [`model::SourceLocation`], [`model::StepDefinition`],
//! [`model::HandlerFingerprint`], [`model::InlineSuppression`], [`model::FeatureStep`],
//! [`config::Config`], [`config::ConfigOverrides`] and [`config::SuppressionConfig`]. Adding a
//! field to any of those is a breaking change: it ships in a major release, or, before 1.0, in the
//! next minor release.
//!
//! [`model::StepDefinition::analysis_policy`] holds the comparison policy the extracting adapter
//! attached, a [`source_adapter::AnalysisPolicy`]. The field is not serialized: a deserialized
//! definition gets its framework contract's policy, else its adapter's, where the adapter is the
//! one its location's suffix routes to when that adapter declares `framework` or `framework` is
//! `unknown`, else the first one declaring `framework`. Plain `Deserialize` reads against the
//! built-in adapters and [`model::RegistrySeed`] against its registry; a definition no adapter
//! claims gets [`source_adapter::AnalysisPolicy::unattached`], which compares only with other
//! unattached definitions, translates no regex, keeps each handler's declared evidence regime and
//! needs indirect step usage for unused findings; adapters cannot declare it. Built-in definitions
//! round-trip with their extracted policy, except one with framework `unknown` at a path that routes
//! to no adapter; a caller adapter that attaches a policy on other grounds does not. A definition
//! built by hand names its policy: [`source_adapter::AnalysisPolicy::unattached`], or for a
//! built-in language `adapter_for_language(ruby::RUBY).map(|ruby| ruby.analysis_policy())`, and a
//! built-in framework-specific policy (Jest-Cucumber, vitest-cucumber-plugin) comes from
//! [`source_adapter::FrameworkContract::policy_of`] over the adapter's
//! [`source_adapter::SourceAdapter::framework_contracts`]. A caller builds its own policy from a
//! [`source_adapter::DomainSpec`] declared in a `static`, a [`source_adapter::RegexDialect`] and
//! the capability enums ([`source_adapter::HandlerScope`], [`source_adapter::StepMatching`],
//! [`source_adapter::StreamEvidence`]) with [`source_adapter::AnalysisPolicy::new`], and keys
//! matchers with [`source_adapter::AnalysisPolicy::normalized_matcher`]. Definitions of different
//! domains are never compared: analysis rejects a mix.
//!
//! A source language is an open id that its adapter declares ([`source_adapter::SourceLanguage`]):
//! the JavaScript/TypeScript ids are the [`typescript::JAVASCRIPT`], [`typescript::TYPESCRIPT`] and
//! [`typescript::TSX`] constants, Ruby's is [`ruby::RUBY`]. A language no registered
//! adapter declares has no adapter: [`source_adapter::adapter_for_language`] returns `None` and
//! extraction entry points return an error. [`source_adapter::SourceAdapter`] and
//! [`source_adapter::RegexDialect`] implementations are `Sync` and `RefUnwindSafe`.
//!
//! Adapters reach the analyzer through a [`source_adapter::AdapterRegistry`].
//! [`source_adapter::AdapterRegistry::builtin`] holds the adapters this crate ships;
//! [`source_adapter::AdapterRegistry::new`] takes any other set and rejects the first conflict as
//! a [`source_adapter::RegistryError`]: an invalid or repeated suffix, a language or adapter name
//! claimed twice, a framework id declared twice or as `unknown`, two domains sharing a label, or a
//! contract policy outside its adapter's domain. A collision with a built-in adapter is rejected,
//! not overridden; start from the built-in registrations to extend or replace them.
//! An adapter that keeps per-run state implements [`source_adapter::StatefulSourceAdapter`] with an
//! [`source_adapter::AdapterSessionState`] type and registers with
//! [`source_adapter::SourceAdapterRegistration::with_session`]: a rooted session initializes the
//! state at construction, the adapter reads it with
//! [`source_adapter::SourceExtractionSession::state`], and discovery runs its
//! `discover_dependencies` hook once with the configured [`discovery::ExcludeSet`]. The session
//! exposes the discovered loading evidence (`dependency_resolved`, `dependency_gap`,
//! `dependency_edges`). Session hooks follow the adapter object they were made for: a built-in
//! adapter registered under another suffix keeps them, and a registration whose `adapter` is
//! replaced drops them.
//!
//! A framework contract finds its feature files through a
//! [`source_adapter::FrameworkDescriptor`] attached with
//! [`source_adapter::FrameworkContract::with_discovery`]. When the configuration names no feature
//! patterns, discovery runs each registered contract's descriptor in registry order and keeps the
//! first [`source_adapter::DiscoveredFeatures`]; a registry that lists the built-in registrations
//! first reaches a caller contract only when no built-in one matches. A descriptor error becomes a
//! configuration warning, except an input-limit error from
//! [`source_adapter::DiscoveryInput::read`], which fails loading.
//! [`source_adapter::DiscoveryInput::package`] returns a `serde_json::Value`, so `serde_json` 1.x
//! is part of the public API.
//! [`config::Config::load_with_adapters`] attaches a registry; discovery, default excludes,
//! framework detection, [`source_adapter::SourceExtractionSession::for_config`] and session
//! finalization read it, and [`source_adapter::SourceFinalization::apply_with`] takes one. These
//! read the built-in registry instead: the free lookup functions
//! ([`source_adapter::adapter_for_language`], [`source_adapter::adapter_for_path`],
//! [`source_adapter::language_for_path`], [`source_adapter::registered_suffixes`]),
//! [`source_adapter::SourceFinalization::apply`], sessions built with
//! [`source_adapter::SourceExtractionSession::new`], `with_registrations` or `with_options`,
//! the `Deserialize` impls of [`model::Framework`] and the types holding one
//! ([`model::StepDefinition`], [`model::AnalysisResult`]), and the JavaScript/TypeScript
//! extraction functions and module resolution. A caller framework id reads back through
//! [`source_adapter::AdapterRegistry::framework`], [`model::FrameworkSeed`] or
//! [`model::RegistrySeed`], which reads any type with a registry's framework ids in scope. Path
//! routing never sends a `.map` file or a TypeScript declaration file (`.d.ts`, `.d.mts`,
//! `.d.cts`) to an adapter, whichever adapter claims the suffix.
//!
//! When adding a public type, follow that rule rather than copying a neighbour: mark it
//! `#[non_exhaustive]` if the analyzer returns it, leave it exhaustive if a caller has to build it,
//! and give it a constructor if it travels both ways. The rule is about structs: a
//! `#[non_exhaustive]` enum's variants stay constructible outside this crate, so every public enum
//! carries it, including the ones callers build ([`model::MatcherKind`],
//! [`source_adapter::HandlerScope`], [`source_adapter::RegexKey`]).
//!
//! Machine-readable output is versioned independently of the crate, so a schema change is visible
//! to consumers that never read the crate version: [`reporters::JSON_SCHEMA_VERSION`],
//! [`reporters::JSONL_SCHEMA_VERSION`] and [`modes::BASELINE_SCHEMA_VERSION`]. Rendering always
//! goes through [`reporters::ReportContext`] and the [`reporters::render_json`],
//! [`reporters::render_jsonl`], [`reporters::render_html`], [`reporters::render_sarif`] and
//! [`reporters::write_terminal`] wrappers, which are the only supported rendering entry points.

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod analysis;
pub mod cli;
pub mod config;
pub mod discovery;
mod framework_descriptor;
mod framework_registry;
pub mod gherkin;
mod matcher;
pub mod model;
pub mod modes;
pub mod reporters;
mod resource_limits;
pub mod ruby;
pub mod source_adapter;
mod stub_vocabulary;
pub mod typescript;
