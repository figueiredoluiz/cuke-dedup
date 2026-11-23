//! Gitignore-aware repository discovery.

use crate::config::Config;
use crate::display_text::DiagnosticDetails;
use crate::gherkin::FeatureFormat;
use crate::resource_limits::{
    MAX_DISCOVERY_GLOB_EVALUATIONS, MAX_DISCOVERY_SELECTED_FILES, MAX_DISCOVERY_SOURCE_BYTES,
    MAX_DISCOVERY_WALK_ENTRIES, MAX_PROJECT_INPUT_BYTES,
};
use crate::source_adapter;
pub use crate::source_adapter::{SourceFile, SourceLanguage};
use anyhow::{bail, Context, Result};
use globset::{Glob, GlobMatcher, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

mod ignore_rules;
use ignore_rules::IgnoreRules;

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

/// Discovers configured feature and definition files while respecting ignore rules, within a
/// fresh run-wide discovery budget.
pub fn discover(config: &Config) -> Result<DiscoveredFiles> {
    discover_within(config, &DiscoveryBudget::default())
}

/// Run-wide discovery work: walk entries, selected files, selected source bytes and glob
/// evaluations. One budget spans every discovery of a run. Totals only grow; the first total past
/// its limit is recorded, and discovery stops at it.
#[derive(Debug)]
pub(crate) struct DiscoveryBudget(Arc<DiscoveryCounters>);

/// Shared by the walk's entry filter, which the walker requires to be `Send + Sync + 'static`.
#[derive(Debug)]
struct DiscoveryCounters {
    limits: [usize; 4],
    spent: [AtomicUsize; 4],
    /// One past the index of the first exhausted unit; 0 while none is.
    exhausted: AtomicUsize,
}

const ENTRIES: usize = 0;
const FILES: usize = 1;
const BYTES: usize = 2;
const GLOBS: usize = 3;
const UNITS: [&str; 4] = [
    "walk entries",
    "selected files",
    "selected source bytes",
    "glob evaluations",
];

impl Default for DiscoveryBudget {
    /// The run-wide discovery caps.
    fn default() -> Self {
        Self::with_limits([
            MAX_DISCOVERY_WALK_ENTRIES,
            MAX_DISCOVERY_SELECTED_FILES,
            MAX_DISCOVERY_SOURCE_BYTES,
            MAX_DISCOVERY_GLOB_EVALUATIONS,
        ])
    }
}

impl DiscoveryBudget {
    /// A budget with nothing spent, limits indexed as [`UNITS`].
    fn with_limits(limits: [usize; 4]) -> Self {
        Self(Arc::new(DiscoveryCounters {
            limits,
            spent: Default::default(),
            exhausted: AtomicUsize::new(0),
        }))
    }

    /// Charges `amount` to `unit` and reports whether every total is still within its limit.
    /// Discovery runs on one thread; atomics only satisfy the walker's filter bounds.
    fn charge(&self, unit: usize, amount: usize) -> bool {
        let counters = &self.0;
        let total = counters.spent[unit]
            .fetch_add(amount, Ordering::Relaxed)
            .saturating_add(amount);
        if total > counters.limits[unit] {
            counters.exhausted.store(unit + 1, Ordering::Relaxed);
            return false;
        }
        true
    }

    /// The recorded exhaustion as the run's fatal error.
    fn check(&self) -> Result<()> {
        match self.0.exhausted.load(Ordering::Relaxed) {
            0 => Ok(()),
            unit => bail!(
                "discovery exceeds {} {}; exclude generated or vendored directories or narrow the \
                 feature and definition globs",
                self.0.limits[unit - 1],
                UNITS[unit - 1]
            ),
        }
    }

    /// Charges `amount` to `unit`, failing once any limit is exceeded.
    fn spend(&self, unit: usize, amount: usize) -> Result<()> {
        self.charge(unit, amount);
        self.check()
    }

    /// Evaluates one glob after charging it.
    fn matches(&self, matcher: &GlobMatcher, path: &Path) -> Result<bool> {
        self.spend(GLOBS, 1)?;
        Ok(matcher.is_match(path))
    }

    /// Totals charged so far, indexed as [`UNITS`].
    #[cfg(test)]
    pub(crate) fn spent(&self) -> [usize; 4] {
        std::array::from_fn(|unit| self.0.spent[unit].load(Ordering::Relaxed))
    }
}

/// [`discover`] charging `budget`. A run with a reference baseline discovers both trees within one
/// budget. Exceeding any limit is fatal: no partial file set is returned.
pub(crate) fn discover_within(
    config: &Config,
    budget: &DiscoveryBudget,
) -> Result<DiscoveredFiles> {
    let feature_globs = compile_pattern_matchers(&config.features, "feature")?;
    let definition_globs = if config.definitions.is_empty() {
        None
    } else {
        Some(compile_pattern_matchers(&config.definitions, "definition")?)
    };
    let excludes = ExcludeSet(compile_globs(&config.exclude, "exclude")?);
    let mut files = DiscoveredFiles::default();
    let mut errors = DiagnosticDetails::default();

    let root = config.root.clone();
    let walk_excludes = excludes.clone();
    let walk_budget = DiscoveryBudget(Arc::clone(&budget.0));
    let (rules, ancestor_error) = IgnoreRules::new(&config.root, !config.include_hidden)?;
    let rules = Arc::new(rules);
    let walk_rules = Arc::clone(&rules);
    if ancestor_error {
        budget.spend(ENTRIES, 1)?;
        errors.push(format!(
            "failed while walking target directory {}: an ignore file above it has invalid entries",
            config.root.display()
        ));
    }
    let mut builder = WalkBuilder::new(&config.root);
    // Ignore files and the hidden filter are applied by `IgnoreRules`, which reads ignore files
    // under the root only when they resolve inside it; the walker only traverses.
    builder
        .standard_filters(false)
        .follow_links(false)
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true;
            }
            // Ignored and hidden entries are dropped before any charge, as the walker dropped them.
            if !walk_rules.admits(entry) {
                return false;
            }
            // Past a limit, admit the entry unexamined so the loop below stops at it instead of
            // the walker filtering the rest of the tree.
            if !walk_budget.charge(ENTRIES, 1) || !walk_budget.charge(GLOBS, 2) {
                return true;
            }
            let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            let directory_form = format!("{}/", relative.to_string_lossy().replace('\\', "/"));
            !walk_excludes.is_match(relative) && !walk_excludes.is_match(directory_form)
        });
    let walker = builder.build();

    for entry in walker {
        budget.check()?;
        rules.check()?;
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                budget.spend(ENTRIES, 1)?;
                errors.push(format!(
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
        let relative = entry
            .path()
            .strip_prefix(&config.root)
            .unwrap_or(entry.path());
        budget.spend(GLOBS, 1)?;
        if excludes.is_match(relative) {
            continue;
        }

        let mut feature_pattern = None;
        for (matcher, pattern) in &feature_globs {
            if budget.matches(matcher, relative)? {
                feature_pattern = Some(pattern);
                break;
            }
        }
        if let Some(pattern) = feature_pattern {
            let format = FeatureFormat::from_path(relative);
            admit_selected(budget, &entry, true)?;
            files.features.push(FeatureFile {
                format,
                path: entry.into_path(),
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
                    // Stops as soon as selection is settled: the first matching glob selects a
                    // default language; an opt-in one needs a matching glob that names its suffix.
                    let mut selection = None;
                    for (matcher, _) in globs {
                        if !budget.matches(matcher, relative)? {
                            continue;
                        }
                        if !opt_in {
                            selection = Some(true);
                            break;
                        }
                        budget.spend(GLOBS, 1)?;
                        if names_suffix(matcher, relative) {
                            selection = Some(true);
                            break;
                        }
                        selection = Some(false);
                    }
                    selection
                }
            };
            if let Some(selected) = selection {
                admit_selected(budget, &entry, selected)?;
                let file = SourceFile {
                    path: entry.into_path(),
                    language,
                };
                if selected {
                    files.definitions.push(file);
                } else {
                    files.skipped_definitions.push(file);
                }
            }
        }
    }
    budget.check()?;
    // A refusal recorded at an entry the excludes then dropped surfaces here.
    rules.check()?;

    files
        .features
        .sort_by(|left, right| left.path.cmp(&right.path));
    files.unmatched_feature_patterns = unmatched_patterns(
        budget,
        &feature_globs,
        &config.root,
        files.features.iter().map(|file| &file.path),
    )?;
    select_conventional_suite(config, budget, &mut files)?;
    files
        .definitions
        .sort_by(|left, right| left.path.cmp(&right.path));
    files.unmatched_definition_patterns = match &definition_globs {
        None => Vec::new(),
        Some(globs) => unmatched_patterns(
            budget,
            globs,
            &config.root,
            files
                .definitions
                .iter()
                .chain(&files.skipped_definitions)
                .map(|file| &file.path),
        )?,
    };
    files.errors = errors.into_lines();
    config
        .adapters()
        .discover_dependencies(config, &excludes, &mut files);
    Ok(files)
}

/// Finds the walked spelling of a conventional top-level directory without following links.
pub(crate) fn conventional_directory<'a>(
    root: &Path,
    name: &str,
    paths: impl Iterator<Item = &'a Path>,
) -> Option<PathBuf> {
    let literal = root.join(name);
    if !literal
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_dir())
    {
        return None;
    }
    let name = std::ffi::OsStr::new(name);
    let mut seen = std::collections::BTreeSet::new();
    paths
        .filter_map(|path| path.strip_prefix(root).ok()?.iter().next())
        .filter(|spelling| spelling.eq_ignore_ascii_case(name) && seen.insert(*spelling))
        .map(|spelling| root.join(spelling))
        .find(|directory| {
            directory == &literal
                || crate::contained_read::same_directory(directory, &literal).unwrap_or(false)
        })
}

/// Selects a standalone conventional suite from files that already passed the walk's filters.
fn select_conventional_suite(
    config: &Config,
    budget: &DiscoveryBudget,
    files: &mut DiscoveredFiles,
) -> Result<()> {
    if !config.definitions.is_empty()
        || !files.definitions.is_empty()
        || files.features.is_empty()
        || files.skipped_definitions.is_empty()
    {
        return Ok(());
    }
    let mut scopes = config
        .adapters()
        .registrations()
        .iter()
        .filter_map(|entry| {
            let root = conventional_directory(
                &config.root,
                entry.adapter.conventional_suite_root(&config.root)?,
                files
                    .features
                    .iter()
                    .take(1)
                    .map(|feature| feature.path.as_path()),
            )?;
            files
                .features
                .iter()
                .all(|feature| {
                    feature.path.starts_with(&root)
                        && feature.path.extension().is_some_and(|ext| ext == "feature")
                })
                .then_some((entry.adapter.language(), root))
        });
    let scope = scopes
        .next()
        .filter(|scope| scopes.all(|other| &other == scope));
    let Some((language, root)) = scope else {
        return Ok(());
    };
    for file in std::mem::take(&mut files.skipped_definitions) {
        if file.language == language && file.path.starts_with(&root) {
            admit_source_bytes(budget, std::fs::symlink_metadata(&file.path).ok())?;
            files.definitions.push(file);
        } else {
            files.skipped_definitions.push(file);
        }
    }
    Ok(())
}

/// Charges one selected file and, when its source will be read, the bytes the bounded reader can
/// read from it. A metadata failure charges no bytes; the reader reports that file later.
fn admit_selected(budget: &DiscoveryBudget, entry: &ignore::DirEntry, read: bool) -> Result<()> {
    budget.spend(FILES, 1)?;
    if read {
        admit_source_bytes(budget, entry.metadata().ok())?;
    }
    Ok(())
}

/// Charges bytes for a discovered source that will be read, leaving read failures to extraction.
fn admit_source_bytes(budget: &DiscoveryBudget, metadata: Option<std::fs::Metadata>) -> Result<()> {
    let length = metadata.map_or(0, |entry| entry.len().min(MAX_PROJECT_INPUT_BYTES + 1));
    budget.spend(BYTES, usize::try_from(length).unwrap_or(usize::MAX))
}

/// Configured patterns, in order, that match none of `paths`; each evaluation is charged.
fn unmatched_patterns<'a>(
    budget: &DiscoveryBudget,
    globs: &[(GlobMatcher, String)],
    root: &Path,
    paths: impl Iterator<Item = &'a PathBuf> + Clone,
) -> Result<Vec<String>> {
    let mut unmatched = Vec::new();
    for (matcher, pattern) in globs {
        let mut matched = false;
        for path in paths.clone() {
            if budget.matches(matcher, path.strip_prefix(root).unwrap_or(path))? {
                matched = true;
                break;
            }
        }
        if !matched {
            unmatched.push(pattern.clone());
        }
    }
    Ok(unmatched)
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

/// The configuration's exclude globs, as a dependency-discovery hook reads them. The default
/// excludes nothing.
#[derive(Clone, Debug, Default)]
pub struct ExcludeSet(GlobSet);

impl ExcludeSet {
    /// Whether any exclude glob matches `path` itself, a path relative to the configuration root.
    pub fn is_match(&self, path: impl AsRef<Path>) -> bool {
        self.0.is_match(path)
    }

    /// Whether the configuration excludes `relative`, a path relative to its root: some glob
    /// matches the path or one of its ancestors, in its own or its trailing-`/` directory form, as
    /// discovery's walk prunes them.
    pub fn excludes(&self, relative: &Path) -> bool {
        relative
            .ancestors()
            .any(|part| self.0.is_match(part) || self.0.is_match(format!("{}/", part.display())))
    }
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
