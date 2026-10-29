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
//! attached, a [`source_adapter::AnalysisPolicy`]. A definition built by hand or deserialized has
//! [`source_adapter::AnalysisPolicy::default`], the JavaScript/TypeScript baseline, whatever its
//! `framework`; a hand-built Ruby definition takes
//! `adapter_for_language(ruby::RUBY).map(|ruby| ruby.analysis_policy())`, and a built-in
//! framework-specific policy (Jest-Cucumber, vitest-cucumber-plugin) comes from
//! [`source_adapter::FrameworkContract::policy_of`] over the adapter's
//! [`source_adapter::SourceAdapter::framework_contracts`]. The field is not serialized. A caller builds its own policy from a
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
