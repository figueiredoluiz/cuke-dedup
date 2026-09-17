//! CukeDedup's reusable static-analysis library.
//!
//! The CLI is the primary integration for repository-wide analysis. Embedders can also parse
//! definitions and feature steps into the shared representation and run the same rules directly:
//!
//! ```
//! use cuke_dedup::analysis::analyze;
//! use cuke_dedup::config::{Config, ConfigOverrides};
//! use cuke_dedup::gherkin;
//! use cuke_dedup::source_adapter::{SourceFile, SourceLanguage};
//! use cuke_dedup::typescript;
//! use std::path::Path;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let root = std::env::current_dir()?;
//! let config = Config::load(&root, ConfigOverrides::default())?;
//! let definition_file = SourceFile {
//!     path: root.join("steps.ts"),
//!     language: SourceLanguage::TypeScript,
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
//! field to any of those is a breaking change and needs a major release.
//!
//! When adding a public type, follow that rule rather than copying a neighbour: mark it
//! `#[non_exhaustive]` if the analyzer returns it, leave it exhaustive if a caller has to build it,
//! and give it a constructor if it travels both ways.
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
mod framework_config;
pub mod gherkin;
pub mod model;
pub mod modes;
pub mod reporters;
mod resource_limits;
pub mod source_adapter;
pub mod typescript;
