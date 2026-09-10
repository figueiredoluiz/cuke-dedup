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
pub struct DiscoveredFiles {
    /// Gherkin feature files and their selected parser formats.
    pub features: Vec<FeatureFile>,
    /// JavaScript or TypeScript definition sources.
    pub definitions: Vec<SourceFile>,
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

        if let Some(language) = source_language(relative) {
            if definition_globs
                .as_ref()
                .map(|globs| globs.iter().any(|(matcher, _)| matcher.is_match(relative)))
                .unwrap_or(true)
            {
                files.definitions.push(SourceFile { path, language });
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
            !files.definitions.iter().any(|file| {
                matcher.is_match(file.path.strip_prefix(&config.root).unwrap_or(&file.path))
            })
        })
        .map(|(_, pattern)| pattern.clone())
        .collect();
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

fn source_language(path: &Path) -> Option<SourceLanguage> {
    source_adapter::language_for_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigOverrides;
    use std::fs;

    #[test]
    fn discovers_sources_and_features_but_uses_gitignore() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("features/steps")).unwrap();
        fs::create_dir_all(directory.path().join("ignored")).unwrap();
        fs::write(directory.path().join(".gitignore"), "ignored/\n").unwrap();
        fs::write(
            directory.path().join("features/example.feature"),
            "Feature: x",
        )
        .unwrap();
        fs::write(
            directory.path().join("features/steps/example.ts"),
            "Given('x', () => {})",
        )
        .unwrap();
        fs::write(
            directory.path().join("ignored/step.ts"),
            "Given('y', () => {})",
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();

        let files = discover(&config).unwrap();
        assert_eq!(files.features.len(), 1);
        assert_eq!(files.definitions.len(), 1);
    }

    #[test]
    fn cuke_dedup_ignore_supports_scoped_gitignore_rules_and_negation() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("ignored")).unwrap();
        fs::create_dir_all(directory.path().join("packages/first")).unwrap();
        fs::create_dir_all(directory.path().join("packages/second")).unwrap();
        fs::write(
            directory.path().join(".cuke-dedupignore"),
            "ignored/\n*.generated.ts\n!keep.generated.ts\n",
        )
        .unwrap();
        fs::write(
            directory.path().join("packages/first/.cuke-dedupignore"),
            "local.ts\n",
        )
        .unwrap();
        for path in [
            "included.ts",
            "ignored/step.ts",
            "drop.generated.ts",
            "keep.generated.ts",
            "packages/first/local.ts",
            "packages/second/local.ts",
        ] {
            fs::write(directory.path().join(path), "Given('x', () => work())").unwrap();
        }

        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        let root = config.root.clone();
        let files = discover(&config).unwrap();
        let relative = files
            .definitions
            .iter()
            .map(|file| {
                file.path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();

        assert_eq!(
            relative,
            vec![
                "included.ts",
                "keep.generated.ts",
                "packages/second/local.ts"
            ]
        );
    }

    #[test]
    fn discovers_classic_and_markdown_features_with_parser_metadata() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("features")).unwrap();
        fs::write(directory.path().join("features/a.feature"), "Feature: A").unwrap();
        fs::write(
            directory.path().join("features/b.feature.md"),
            "# Feature: B",
        )
        .unwrap();
        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();

        let files = discover(&config).unwrap();
        assert_eq!(files.features.len(), 2);
        assert_eq!(files.features[0].format, FeatureFormat::Gherkin);
        assert_eq!(files.features[1].format, FeatureFormat::GherkinMarkdown);
        assert!(files.unmatched_feature_patterns.is_empty());
    }

    #[test]
    fn overlapping_feature_patterns_are_both_counted_as_matched() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("features")).unwrap();
        fs::write(directory.path().join("features/a.feature"), "Feature: A").unwrap();
        let config = Config::load(
            directory.path(),
            ConfigOverrides {
                features: Some(vec![
                    "**/*.feature".to_owned(),
                    "features/**/*.feature".to_owned(),
                ]),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();

        let files = discover(&config).unwrap();
        assert!(files.unmatched_feature_patterns.is_empty());
    }

    #[test]
    fn configured_definition_globs_narrow_source_files() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("steps")).unwrap();
        fs::write(directory.path().join("steps/a.ts"), "").unwrap();
        fs::write(directory.path().join("other.ts"), "").unwrap();
        let overrides = ConfigOverrides {
            definitions: Some(vec!["steps/**/*.ts".to_owned()]),
            ..ConfigOverrides::default()
        };
        let config = Config::load(directory.path(), overrides).unwrap();

        let files = discover(&config).unwrap();
        assert_eq!(files.definitions.len(), 1);
        assert!(files.definitions[0].path.ends_with("steps/a.ts"));
        assert!(files.unmatched_definition_patterns.is_empty());
    }

    #[test]
    fn configured_globs_use_documented_cross_separator_and_brace_semantics() {
        let globs = compile_globs(
            &[
                "*.ts".to_owned(),
                "features/*.{feature,feature.md}".to_owned(),
            ],
            "test",
        )
        .unwrap();
        assert!(globs.is_match(Path::new("nested/deep/steps.ts")));
        assert!(globs.is_match(Path::new("features/nested/example.feature.md")));
        assert!(!globs.is_match(Path::new("features/example.md")));
    }

    #[test]
    fn reports_each_unmatched_definition_glob() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("steps.ts"),
            "Given('x', () => work())",
        )
        .unwrap();
        let config = Config::load(
            directory.path(),
            ConfigOverrides {
                definitions: Some(vec!["stpes/**/*.ts".to_owned()]),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();

        let files = discover(&config).unwrap();
        assert!(files.definitions.is_empty());
        assert_eq!(files.unmatched_definition_patterns, ["stpes/**/*.ts"]);
    }

    #[test]
    fn declaration_and_source_map_files_are_not_definition_sources() {
        for path in ["types.d.ts", "types.d.mts", "types.d.cts", "code.js.map"] {
            assert_eq!(source_language(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn built_in_excludes_prune_sources_unless_explicitly_disabled() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("node_modules/workspace")).unwrap();
        fs::write(
            directory.path().join("node_modules/workspace/steps.ts"),
            "Given('nested', () => work())",
        )
        .unwrap();

        let default_config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert!(discover(&default_config).unwrap().definitions.is_empty());

        let enabled_config = Config::load(
            directory.path(),
            ConfigOverrides {
                definitions: Some(vec!["node_modules/workspace/**/*.ts".to_owned()]),
                exclude_defaults: Some(false),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        let files = discover(&enabled_config).unwrap();
        assert_eq!(files.definitions.len(), 1);
    }

    #[test]
    fn hidden_sources_require_the_include_hidden_option() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join(".steps")).unwrap();
        fs::write(
            directory.path().join(".steps/example.ts"),
            "Given('hidden', () => work())",
        )
        .unwrap();
        let default_config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        assert!(discover(&default_config).unwrap().definitions.is_empty());

        let visible_config = Config::load(
            directory.path(),
            ConfigOverrides {
                definitions: Some(vec![".steps/**/*.ts".to_owned()]),
                include_hidden: Some(true),
                ..ConfigOverrides::default()
            },
        )
        .unwrap();
        assert_eq!(discover(&visible_config).unwrap().definitions.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn traversal_errors_are_collected_without_discarding_other_files() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("visible.ts"),
            "Given('visible', () => work())",
        )
        .unwrap();
        let locked = directory.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::write(locked.join("hidden.ts"), "Given('hidden', () => work())").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        let files = discover(&config).unwrap();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(files.definitions.len(), 1);
        assert!(!files.errors.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn excluded_directories_are_pruned_before_traversal() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let locked = directory.path().join("node_modules/locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("step.ts"), "Given('hidden', () => work())").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
        let files = discover(&config).unwrap();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(files.definitions.is_empty());
        assert!(files.errors.is_empty());
    }
}
