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
pub mod source_adapter;
pub mod typescript;
