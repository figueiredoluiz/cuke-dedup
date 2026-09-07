//! Definition-source adapter contracts and extension-based routing.

use crate::model::{SourceLocation, StepDefinition};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

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

/// Definitions and non-fatal diagnostics extracted from one source.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extraction {
    /// Successfully extracted definitions.
    pub definitions: Vec<StepDefinition>,
    /// Problems that did not prevent extraction of the rest of the file.
    pub diagnostics: Vec<ExtractionDiagnostic>,
}

/// Converts one supported definition-source language into the shared definition IR.
pub trait SourceAdapter: Sync {
    /// Stable adapter name used in diagnostics and registry inspection.
    fn name(&self) -> &'static str;

    /// Parser language produced by this adapter registration.
    fn language(&self) -> SourceLanguage;

    /// Extracts definitions from in-memory source using `file` for source locations.
    fn extract(&self, source: &str, file: &SourceFile) -> Result<Extraction>;

    /// Reads and extracts one source file.
    fn extract_file(&self, file: &SourceFile) -> Result<Extraction> {
        let source = fs::read_to_string(&file.path)
            .with_context(|| format!("failed to read source file {}", file.path.display()))?;
        self.extract(&source, file)
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
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".d.ts")
        || name.ends_with(".d.mts")
        || name.ends_with(".d.cts")
        || name.ends_with(".map")
    {
        return None;
    }
    SOURCE_ADAPTER_REGISTRY
        .iter()
        .find(|registration| name.ends_with(registration.suffix))
        .map(|registration| registration.adapter)
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
            "steps.js.map",
            "steps.py",
        ] {
            assert!(adapter_for_path(Path::new(path)).is_none(), "{path}");
        }
    }
}
