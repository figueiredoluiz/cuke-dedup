//! Evidence for conventional and configured feature ownership in automatic CLI runs.

use super::{
    admit_source_bytes, DiscoveredFiles, DiscoveryBudget, FeatureFile, FeatureFormat,
    SourceLanguage, FILES, MAX_DISCOVERY_SELECTED_FILES,
};
use crate::config::{Config, MixedFeatureSelection};
use anyhow::{bail, Result};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum FeatureOwner {
    #[default]
    Framework,
    Conventional,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MixedSuite {
    pub(crate) conventional_language: SourceLanguage,
}

/// An observed conventional root and feature files the framework matcher did not select.
pub(super) struct ConventionalCandidate {
    expected_root: PathBuf,
    walked_root: Option<PathBuf>,
    language: SourceLanguage,
    additional_features: Vec<PathBuf>,
    omitted_features: bool,
}

impl ConventionalCandidate {
    /// Enables mixed selection only for automatic feature and definition discovery.
    pub(super) fn new(config: &Config) -> Option<Self> {
        if config.mixed_feature_selection == MixedFeatureSelection::Disabled
            || !config.definitions.is_empty()
        {
            return None;
        }
        let mut defaults = config
            .adapters()
            .registrations()
            .iter()
            .filter_map(|entry| {
                let adapter = entry.adapter;
                let directory = adapter.conventional_suite_root(&config.root)?;
                Some((config.root.join(directory), adapter.language()))
            });
        let (expected_root, language) = defaults.next()?;
        if defaults.any(|entry| entry != (expected_root.clone(), language))
            || !expected_root
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_dir())
        {
            return None;
        }
        Some(Self {
            expected_root,
            walked_root: None,
            language,
            additional_features: Vec::new(),
            omitted_features: false,
        })
    }

    /// Retains the identity of a top-level directory delivered by the filtered no-follow walk.
    pub(super) fn observe_directory(&mut self, path: &Path) {
        let (Some(spelling), Some(expected)) = (path.file_name(), self.expected_root.file_name())
        else {
            return;
        };
        if spelling.eq_ignore_ascii_case(expected)
            && (path == self.expected_root
                || crate::contained_read::same_directory(path, &self.expected_root)
                    .unwrap_or(false))
        {
            self.walked_root = Some(path.to_path_buf());
        }
    }

    /// Buffers only conventional features missing from the configured selection.
    pub(super) fn observe_feature(&mut self, path: &Path) -> bool {
        if !self
            .walked_root
            .as_ref()
            .is_some_and(|root| conventional_feature(path, root))
        {
            return false;
        }
        if self.additional_features.len() == MAX_DISCOVERY_SELECTED_FILES {
            self.omitted_features = true;
        } else {
            self.additional_features.push(path.to_path_buf());
        }
        true
    }

    /// Activates two-domain routing when both source selections and a conventional feature exist.
    pub(super) fn select(
        self,
        config: &Config,
        budget: &DiscoveryBudget,
        files: &mut DiscoveredFiles,
    ) -> Result<()> {
        let Some(root) = self.walked_root else {
            return Ok(());
        };
        let Some(first) = files.definitions.first() else {
            return Ok(());
        };
        let adapters = config.adapters();
        let domain = adapters.comparison_domain(first.language);
        if adapters.comparison_domain(self.language) == domain
            || files
                .definitions
                .iter()
                .any(|file| adapters.comparison_domain(file.language) != domain)
            || !files
                .skipped_definitions
                .iter()
                .any(|file| file.language == self.language && file.path.starts_with(&root))
            || (self.additional_features.is_empty()
                && !files
                    .features
                    .iter()
                    .any(|feature| conventional_feature(&feature.path, &root)))
        {
            return Ok(());
        }
        if self.omitted_features {
            bail!("discovery exceeds {MAX_DISCOVERY_SELECTED_FILES} selected files in the conventional feature tree");
        }
        let framework_paths = config.mixed_feature_selection == MixedFeatureSelection::Framework;
        for feature in &mut files.features {
            if !framework_paths || conventional_feature(&feature.path, &root) {
                feature.owner = FeatureOwner::Unresolved;
            }
        }
        for path in self.additional_features {
            budget.spend(FILES, 1)?;
            admit_source_bytes(budget, path.symlink_metadata().ok())?;
            files.features.push(FeatureFile {
                path,
                format: FeatureFormat::Gherkin,
                pattern: format!(
                    "{}/**/*.feature",
                    self.expected_root
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
                owner: if framework_paths {
                    FeatureOwner::Conventional
                } else {
                    FeatureOwner::Unresolved
                },
            });
        }
        for file in std::mem::take(&mut files.skipped_definitions) {
            if file.language == self.language && file.path.starts_with(&root) {
                admit_source_bytes(budget, file.path.symlink_metadata().ok())?;
                files.definitions.push(file);
            } else {
                files.skipped_definitions.push(file);
            }
        }
        files.mixed_suite = Some(MixedSuite {
            conventional_language: self.language,
        });
        Ok(())
    }
}

/// The conventional Ruby runner reads plain Gherkin under its observed suite root.
fn conventional_feature(path: &Path, root: &Path) -> bool {
    path.starts_with(root) && path.extension() == Some(OsStr::new("feature"))
}
