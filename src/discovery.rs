//! Gitignore-aware repository discovery.

use crate::config::Config;
use crate::gherkin::FeatureFormat;
use crate::source_adapter;
pub use crate::source_adapter::{SourceFile, SourceLanguage};
use anyhow::{Context, Result};
use globset::{Glob, GlobMatcher, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
/// A discovered feature file together with its parser and matching config pattern.
#[non_exhaustive]
pub struct FeatureFile {
    /// Absolute feature-file path.
    pub path: PathBuf,
    /// Parser selected from the discovered filename.
    pub format: FeatureFormat,
    /// Effective glob pattern that selected this file.
    pub pattern: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Deterministically ordered files discovered for one analysis run.
#[non_exhaustive]
pub struct DiscoveredFiles {
    /// Resolved source-loading edges used by definition extraction.
    pub dependencies: Vec<source_adapter::SourceDependency>,
    /// Source-loading calls that dependency discovery refused to follow, with the reason.
    pub dependency_gaps: Vec<source_adapter::DependencyGap>,
    /// Gherkin feature files and their selected parser formats.
    pub features: Vec<FeatureFile>,
    /// Definition sources selected through their registered frontend.
    pub definitions: Vec<SourceFile>,
    /// Sources of opt-in languages that were not selected because automatic discovery or only
    /// suffix-agnostic definition patterns matched them, less those that dependency resolution
    /// then followed a load to and analyzed.
    pub skipped_definitions: Vec<SourceFile>,
    /// Non-fatal filesystem traversal errors collected during discovery.
    pub errors: Vec<String>,
    /// Configured feature patterns that matched no files.
    pub unmatched_feature_patterns: Vec<String>,
    /// Configured definition patterns that matched no files.
    pub unmatched_definition_patterns: Vec<String>,
}

/// Discovers configured feature and definition files while respecting ignore rules.
pub fn discover(config: &Config) -> Result<DiscoveredFiles> {
    let feature_globs = compile_pattern_matchers(&config.features, "feature")?;
    let definition_globs = if config.definitions.is_empty() {
        None
    } else {
        Some(compile_pattern_matchers(&config.definitions, "definition")?)
    };
    let excludes = compile_globs(&config.exclude, "exclude")?;
    let mut files = DiscoveredFiles::default();

    let root = config.root.clone();
    let walk_excludes = excludes.clone();
    let mut builder = WalkBuilder::new(&config.root);
    builder
        .standard_filters(true)
        .add_custom_ignore_filename(".cuke-dedupignore")
        .require_git(false)
        .hidden(!config.include_hidden)
        .follow_links(false)
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            let directory_form = format!("{}/", relative.to_string_lossy().replace('\\', "/"));
            !walk_excludes.is_match(relative) && !walk_excludes.is_match(directory_form)
        });
    let walker = builder.build();

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                files.errors.push(format!(
                    "failed while walking target directory {}: {error}",
                    config.root.display()
                ));
                continue;
            }
        };
        match entry.file_type() {
            Some(file_type) if file_type.is_file() => {}
            _ => continue,
        }
        let path = entry.into_path();
        let relative = path.strip_prefix(&config.root).unwrap_or(&path);
        if excludes.is_match(relative) {
            continue;
        }

        if let Some((_, pattern)) = feature_globs
            .iter()
            .find(|(matcher, _)| matcher.is_match(relative))
        {
            files.features.push(FeatureFile {
                format: FeatureFormat::from_path(relative),
                path,
                pattern: pattern.clone(),
            });
            continue;
        }

        if let Some(adapter) = config.adapters().adapter_for_path(relative) {
            let language = adapter.language();
            let opt_in = !adapter.discover_by_default();
            let selection = match &definition_globs {
                None => Some(!opt_in),
                Some(globs) => {
                    let mut matching = globs
                        .iter()
                        .filter(|(matcher, _)| matcher.is_match(relative))
                        .peekable();
                    matching.peek().is_some().then(|| {
                        !opt_in || matching.any(|(matcher, _)| names_suffix(matcher, relative))
                    })
                }
            };
            match selection {
                Some(true) => files.definitions.push(SourceFile { path, language }),
                Some(false) => files
                    .skipped_definitions
                    .push(SourceFile { path, language }),
                None => {}
            }
        }
    }

    files
        .features
        .sort_by(|left, right| left.path.cmp(&right.path));
    files.unmatched_feature_patterns = feature_globs
        .iter()
        .filter(|(matcher, _)| {
            !files.features.iter().any(|file| {
                matcher.is_match(file.path.strip_prefix(&config.root).unwrap_or(&file.path))
            })
        })
        .map(|(_, pattern)| pattern.clone())
        .collect();
    files
        .definitions
        .sort_by(|left, right| left.path.cmp(&right.path));
    files.unmatched_definition_patterns = definition_globs
        .as_ref()
        .into_iter()
        .flatten()
        .filter(|(matcher, _)| {
            !files
                .definitions
                .iter()
                .chain(&files.skipped_definitions)
                .any(|file| {
                    matcher.is_match(file.path.strip_prefix(&config.root).unwrap_or(&file.path))
                })
        })
        .map(|(_, pattern)| pattern.clone())
        .collect();
    config
        .adapters()
        .discover_dependencies(config, &excludes, &mut files);
    Ok(files)
}

/// Compiles patterns into matchers paired with their original text for diagnostics.
fn compile_pattern_matchers(patterns: &[String], kind: &str) -> Result<Vec<(GlobMatcher, String)>> {
    patterns
        .iter()
        .map(|pattern| {
            let glob = Glob::new(pattern)
                .with_context(|| format!("invalid {kind} glob pattern `{pattern}`"))?;
            Ok((glob.compile_matcher(), pattern.clone()))
        })
        .collect()
}

/// Builds one `GlobSet` from `patterns`; an empty list matches nothing. Errors name the first
/// invalid pattern and its `kind`.
fn compile_globs(patterns: &[String], kind: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(
            Glob::new(pattern)
                .with_context(|| format!("invalid {kind} glob pattern `{pattern}`"))?,
        );
    }
    builder
        .build()
        .with_context(|| format!("failed to build {kind} glob set"))
}

/// Opt-in languages are selected only by a pattern that depends on their suffix, so a broad
/// pattern such as `features/**` keeps selecting the same files it did before the language existed.
fn names_suffix(matcher: &GlobMatcher, path: &Path) -> bool {
    !matcher.is_match(path.with_extension("cuke-dedup-suffix-probe"))
}

#[cfg(test)]
mod tests;
