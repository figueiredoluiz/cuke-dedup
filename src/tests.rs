//! Shared fixture builders and bounded generators for behavioral regression tests.

/// Creates automatic-file inputs with known file types for refusal and replacement controls.
pub(crate) mod automatic_file;
/// Materializes feature documents from the generated semantic model.
pub(crate) mod feature_model;
/// Runs generated cases with explicit, repeatable property-test settings.
pub(crate) mod property_runner;
