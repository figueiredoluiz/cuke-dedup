use super::ast::is_relative_module;
use crate::resource_limits::{
    read_utf8, WorkBudget, MAX_CONFIG_INPUT_BYTES, MAX_PROJECT_CONFIG_EXTENDS_DEPTH,
    MAX_PROJECT_METADATA_BYTES, MAX_PROJECT_METADATA_FILES, MAX_PROJECT_MODULE_MAPPINGS,
    MAX_WORKSPACE_SCAN_ENTRIES,
};
use crate::source_adapter::{language_for_path, SourceLanguage};
use anyhow::{bail, Context, Result};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const MODULE_SUFFIXES: [&str; 8] = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

/// Returns the language of an ECMAScript module path, or `None` for declaration files, source maps,
/// other adapters' languages and every unregistered suffix: only ECMAScript sources can register
/// steps through an import.
pub(super) fn ecmascript_language(path: &Path) -> Option<SourceLanguage> {
    language_for_path(path).filter(|language| {
        matches!(
            *language,
            super::JAVASCRIPT | super::TYPESCRIPT | super::TSX
        )
    })
}

#[derive(Clone, Debug, Default)]
struct EffectiveProjectConfig {
    base_url: Option<PathBuf>,
    paths: Option<Vec<PathMapping>>,
}

impl EffectiveProjectConfig {
    /// Overrides inherited options only where the later configuration supplies a value.
    fn merge(&mut self, later: Self) {
        if later.base_url.is_some() {
            self.base_url = later.base_url;
        }
        if later.paths.is_some() {
            self.paths = later.paths;
        }
    }
}

#[derive(Clone, Debug)]
struct PathMapping {
    pattern: String,
    targets: Vec<String>,
    origin: PathBuf,
}

#[derive(Clone, Debug)]
struct PackageInfo {
    root: PathBuf,
    name: Option<String>,
    imports: Option<Value>,
    exports: Option<Value>,
    main: Option<String>,
    /// The `tsconfig` field TypeScript reads when a config `extends` the bare package name.
    tsconfig: Option<String>,
    workspaces: Vec<String>,
}

/// A failed load or scan, kept so later resolutions report it without reading or charging the
/// same input again. Consumers read only its `{:#}` text: an I/O failure, the one typed cause they
/// test, is never kept, because a read can fail transiently.
#[derive(Clone)]
pub(super) struct KeptFailure {
    message: String,
}

impl KeptFailure {
    /// Keeps `error`'s full cause chain, or `None` for a failed read. A workspace walk error holds
    /// its I/O cause without exposing it as a source, so it is checked apart from `is_io_failure`.
    pub(super) fn new(error: &anyhow::Error) -> Option<Self> {
        let walk_read = |cause: &(dyn std::error::Error + 'static)| {
            cause
                .downcast_ref::<ignore::Error>()
                .is_some_and(|walk| walk.io_error().is_some())
        };
        (!is_io_failure(error) && !error.chain().any(walk_read)).then(|| Self {
            message: format!("{error:#}"),
        })
    }

    /// An error with the kept text.
    pub(super) fn error(&self) -> anyhow::Error {
        anyhow::anyhow!(self.message.clone())
    }
}

/// Returns whether `error` was caused by the filesystem refusing a read.
///
/// Resource limits carry their own typed cause, so an `io::Error` anywhere in the chain means a
/// genuine read failure — a permission change, a vanished file, or exhausted descriptors.
pub(super) fn is_io_failure(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some())
}

pub(super) struct ProjectResolution {
    root: Option<PathBuf>,
    config_for_directory: BTreeMap<PathBuf, Option<PathBuf>>,
    configs: BTreeMap<PathBuf, EffectiveProjectConfig>,
    /// Top-level config loads that failed; a nested failure depends on the depth and the configs in
    /// flight, so only its top-level load is kept.
    failed_configs: BTreeMap<PathBuf, KeptFailure>,
    config_active: BTreeSet<PathBuf>,
    packages: BTreeMap<PathBuf, Result<PackageInfo, KeptFailure>>,
    workspace_packages: Option<Result<BTreeMap<String, PackageInfo>, KeptFailure>>,
    metadata_files: WorkBudget,
    metadata_bytes: WorkBudget,
    mappings: WorkBudget,
}

impl Default for ProjectResolution {
    /// No root, empty caches and the release metadata and mapping limits.
    fn default() -> Self {
        Self {
            root: None,
            config_for_directory: BTreeMap::new(),
            configs: BTreeMap::new(),
            failed_configs: BTreeMap::new(),
            config_active: BTreeSet::new(),
            packages: BTreeMap::new(),
            workspace_packages: None,
            metadata_files: WorkBudget::new(MAX_PROJECT_METADATA_FILES),
            metadata_bytes: WorkBudget::new(MAX_PROJECT_METADATA_BYTES),
            mappings: WorkBudget::new(MAX_PROJECT_MODULE_MAPPINGS),
        }
    }
}

impl ProjectResolution {
    /// Creates a metadata resolver anchored to a canonical root when canonicalization succeeds.
    pub(super) fn for_root(root: &Path) -> Self {
        Self {
            root: root.canonicalize().ok(),
            ..Self::default()
        }
    }

    /// Resolves relative paths, package imports, TypeScript path aliases, and workspace packages within the analysis boundary.
    pub(super) fn resolve(
        &mut self,
        importer: &Path,
        specifier: &str,
        fallback_boundary: &Path,
    ) -> Result<Option<PathBuf>> {
        let boundary = self.root.as_deref().unwrap_or(fallback_boundary).to_owned();
        if is_relative_module(specifier) {
            return self.resolve_path(
                &importer.parent().unwrap_or(&boundary).join(specifier),
                &boundary,
            );
        }
        let Some(root) = self.root.clone() else {
            return Ok(None);
        };
        if specifier.starts_with('#') {
            return self.resolve_package_import(importer, specifier, &root, 0);
        }
        if let Some(path) = self.resolve_tsconfig_path(importer, specifier, &root)? {
            return Ok(Some(path));
        }
        self.resolve_workspace_specifier(importer, specifier, &root, 0)
    }

    /// Tries targets from the most specific matching path alias in the nearest effective project configuration.
    fn resolve_tsconfig_path(
        &mut self,
        importer: &Path,
        specifier: &str,
        root: &Path,
    ) -> Result<Option<PathBuf>> {
        let Some(config_path) = self.nearest_project_config(importer, root)? else {
            return Ok(None);
        };
        let config = self.load_project_config(&config_path, root, 0)?.clone();
        let Some(mappings) = config.paths.as_ref() else {
            return Ok(None);
        };
        let Some((mapping, capture)) = best_mapping(mappings, specifier) else {
            return Ok(None);
        };
        let anchor = config.base_url.as_deref().unwrap_or(&mapping.origin);
        for target in &mapping.targets {
            let target = substitute_star(target, capture.as_deref());
            if let Some(path) = self.resolve_path(&anchor.join(target), root)? {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// Caches the nearest contained tsconfig or jsconfig, preferring tsconfig within each ancestor directory.
    fn nearest_project_config(&mut self, importer: &Path, root: &Path) -> Result<Option<PathBuf>> {
        let directory = importer
            .parent()
            .unwrap_or(root)
            .canonicalize()
            .with_context(|| {
                format!(
                    "failed to resolve importer directory for {}",
                    importer.display()
                )
            })?;
        if let Some(cached) = self.config_for_directory.get(&directory) {
            return Ok(cached.clone());
        }
        if !directory.starts_with(root) {
            bail!(
                "importer {} resolves outside the analysis root {}",
                importer.display(),
                root.display()
            );
        }
        let mut selected = None;
        for ancestor in directory.ancestors() {
            if !ancestor.starts_with(root) {
                break;
            }
            for name in ["tsconfig.json", "jsconfig.json"] {
                let candidate = ancestor.join(name);
                if candidate.is_file() {
                    selected =
                        Some(self.canonical_contained(&candidate, root, "project config")?);
                    break;
                }
            }
            if selected.is_some() || ancestor == root {
                break;
            }
        }
        self.config_for_directory
            .insert(directory, selected.clone());
        Ok(selected)
    }

    /// Caches effective contained configurations, rejecting extends cycles and excessive inheritance depth.
    fn load_project_config(
        &mut self,
        path: &Path,
        root: &Path,
        depth: usize,
    ) -> Result<&EffectiveProjectConfig> {
        if depth >= MAX_PROJECT_CONFIG_EXTENDS_DEPTH {
            bail!(
                "project config extends exceeds the {}-file depth limit at {}",
                MAX_PROJECT_CONFIG_EXTENDS_DEPTH,
                path.display()
            );
        }
        let canonical = self.canonical_contained(path, root, "project config")?;
        let top_level = depth == 0;
        // Before the success cache: a nested load that succeeded later must not hide a kept failure.
        if let Some(failure) = self.failed_configs.get(&canonical).filter(|_| top_level) {
            return Err(failure.error());
        }
        if self.configs.contains_key(&canonical) {
            return self
                .configs
                .get(&canonical)
                .context("project config cache entry disappeared");
        }
        if !self.config_active.insert(canonical.clone()) {
            bail!("project config extends cycle at {}", canonical.display());
        }
        let result = self.load_project_config_uncached(&canonical, root, depth);
        self.config_active.remove(&canonical);
        let loaded = result.inspect_err(|error| {
            if let Some(failure) = KeptFailure::new(error).filter(|_| top_level) {
                self.failed_configs.insert(canonical.clone(), failure);
            }
        })?;
        self.configs.insert(canonical.clone(), loaded);
        self.configs
            .get(&canonical)
            .context("project config cache insertion failed")
    }

    /// Reads static JSONC configuration, merges available bases in order, and validates and charges local path mappings.
    fn load_project_config_uncached(
        &mut self,
        path: &Path,
        root: &Path,
        depth: usize,
    ) -> Result<EffectiveProjectConfig> {
        let text = self.read_metadata(path, "project config")?;
        let normalized = normalize_jsonc(&text)?;
        let value: Value = serde_json::from_str(&normalized)
            .with_context(|| format!("failed to parse static project config {}", path.display()))?;
        let object = value
            .as_object()
            .with_context(|| format!("project config {} must contain an object", path.display()))?;
        let mut effective = EffectiveProjectConfig::default();
        if let Some(extends) = object.get("extends") {
            let entries = string_or_string_array(extends).with_context(|| {
                format!(
                    "project config extends in {} must be a string or string array",
                    path.display()
                )
            })?;
            for entry in entries {
                // A package base that is not available inside the root is skipped; see
                // `resolve_extends`.
                if let Some(base) = self.resolve_extends(path, &entry, root)? {
                    effective.merge(self.load_project_config(&base, root, depth + 1)?.clone());
                }
            }
        }
        let Some(options) = object.get("compilerOptions") else {
            return Ok(effective);
        };
        let options = options.as_object().with_context(|| {
            format!(
                "compilerOptions in {} must contain an object",
                path.display()
            )
        })?;
        if let Some(base_url) = options.get("baseUrl") {
            let base_url = base_url.as_str().with_context(|| {
                format!(
                    "compilerOptions.baseUrl in {} must be a string",
                    path.display()
                )
            })?;
            let candidate = path.parent().unwrap_or(root).join(base_url);
            effective.base_url = Some(canonicalize_existing_ancestor(&candidate, root)?);
        }
        if let Some(paths) = options.get("paths") {
            let paths = paths.as_object().with_context(|| {
                format!(
                    "compilerOptions.paths in {} must contain an object",
                    path.display()
                )
            })?;
            self.charge_mappings(paths.len())?;
            let mut mappings = Vec::with_capacity(paths.len());
            for (pattern, targets) in paths {
                validate_single_star(pattern, "tsconfig path pattern")?;
                let targets = string_or_string_array(targets).with_context(|| {
                    format!(
                        "path mapping `{pattern}` in {} must contain string targets",
                        path.display()
                    )
                })?;
                self.charge_mappings(targets.len())?;
                for target in &targets {
                    validate_single_star(target, "tsconfig path target")?;
                }
                mappings.push(PathMapping {
                    pattern: pattern.clone(),
                    targets,
                    origin: path.parent().unwrap_or(root).to_owned(),
                });
            }
            effective.paths = Some(mappings);
        }
        Ok(effective)
    }

    /// Resolves one `extends` entry to a static JSON base inside the analysis root, or `None` when a
    /// package base is not available here.
    ///
    /// A relative entry must resolve: a missing base is a repository configuration error. A package
    /// entry — how shared bases such as `@tsconfig/recommended/tsconfig.json` or `expo/tsconfig.base`
    /// are written — resolves only to a file inside a workspace package in the root, where a missing
    /// base is also an error; resolution never reads `node_modules`. Any other package base is
    /// skipped rather than rejecting the whole config, so the project's own `baseUrl` and `paths`
    /// still apply. An alias that only a skipped base defines stays unresolved and is reported,
    /// never invented.
    fn resolve_extends(
        &mut self,
        config: &Path,
        extends: &str,
        root: &Path,
    ) -> Result<Option<PathBuf>> {
        if is_executable_config(Path::new(extends)) {
            bail!(
                "project config extends only supports static JSON paths; `{extends}` in {} would require executing repository code",
                config.display()
            );
        }
        if is_relative_module(extends) {
            let base = config.parent().unwrap_or(root).join(extends);
            for candidate in config_base_candidates(&base) {
                if candidate.is_file() {
                    return self
                        .canonical_contained(&candidate, root, "project config extends target")
                        .map(Some);
                }
            }
            bail!(
                "project config extends target `{extends}` from {} could not be resolved",
                config.display()
            );
        }
        if !is_bare_module(extends) {
            bail!(
                "project config extends only supports relative paths or package names inside the analysis root: `{extends}` in {}",
                config.display()
            );
        }
        let Some((name, subpath)) = split_package_specifier(extends) else {
            return Ok(None);
        };
        let Some(package) = self.workspace_packages(root)?.get(name).cloned() else {
            return Ok(None);
        };
        // A package base stays inside its package, like an `exports` target: no traversal, no
        // `node_modules`, and no symlink out of the package root.
        let mut candidates = Vec::new();
        if subpath.is_empty() {
            if let Some(field) = package.tsconfig.as_deref() {
                // A leading separator is rooted on every platform; Windows `is_absolute` also wants a
                // drive, so `/etc/x` alone does not count there.
                if Path::new(field).is_absolute() || field.starts_with(['/', '\\']) {
                    bail!(
                        "package `{name}` `tsconfig` field `{field}` resolves outside the package"
                    );
                }
                // `./` and Windows `.\` both name the package directory itself.
                let relative = field
                    .strip_prefix("./")
                    .or_else(|| field.strip_prefix(".\\"))
                    .unwrap_or(field);
                let target = format!("./{relative}");
                reject_invalid_package_target(&target, "package `tsconfig` field")?;
                let field = package.root.join(target);
                if is_executable_config(&field) {
                    bail!(
                        "package `{name}` names an executable `tsconfig` base, which static resolution refuses to run"
                    );
                }
                candidates.extend(config_base_candidates(&field));
            }
            candidates.push(package.root.join("tsconfig.json"));
        } else {
            reject_invalid_package_target(
                &format!("./{subpath}"),
                "project config extends subpath",
            )?;
            candidates.extend(config_base_candidates(&package.root.join(subpath)));
        }
        for candidate in candidates {
            if candidate.is_file() {
                let base =
                    self.canonical_contained(&candidate, root, "project config extends target")?;
                // Checked on the canonical path, so a symlink cannot lead out of the package or into
                // its `node_modules`.
                let inside = base.strip_prefix(&package.root).is_ok_and(|relative| {
                    !relative
                        .components()
                        .any(|component| component.as_os_str() == "node_modules")
                });
                if !inside {
                    bail!(
                        "project config extends target `{extends}` resolves outside workspace package `{name}` or into its `node_modules`"
                    );
                }
                return Ok(Some(base));
            }
        }
        // The package is known to exist here, so a missing base is a configuration error, not an
        // unavailable dependency.
        bail!(
            "project config extends target `{extends}` from {} names workspace package `{name}` but no config file there",
            config.display()
        )
    }

    /// Follows nearest-package import targets within a bounded chain, allowing contained paths and workspace mappings.
    fn resolve_package_import(
        &mut self,
        importer: &Path,
        specifier: &str,
        root: &Path,
        depth: usize,
    ) -> Result<Option<PathBuf>> {
        if depth >= MAX_PROJECT_CONFIG_EXTENDS_DEPTH {
            bail!(
                "package import mapping exceeds the {}-step resolution limit",
                MAX_PROJECT_CONFIG_EXTENDS_DEPTH
            );
        }
        let Some(package) = self.nearest_package(importer, root)? else {
            return Ok(None);
        };
        let Some(imports) = package.imports.as_ref() else {
            return Ok(None);
        };
        let Some(targets) = package_map_targets(imports, specifier, "package imports")? else {
            return Ok(None);
        };
        for target in targets {
            if target.starts_with("./") {
                reject_invalid_package_target(&target, "package import target")?;
                if let Some(path) = self.resolve_path(&package.root.join(&target), &package.root)? {
                    return Ok(Some(path));
                }
            } else if target.starts_with('#') {
                if let Some(path) =
                    self.resolve_package_import(importer, &target, root, depth + 1)?
                {
                    return Ok(Some(path));
                }
            } else if is_bare_module(&target) {
                if let Some(path) =
                    self.resolve_workspace_specifier(importer, &target, root, depth + 1)?
                {
                    return Ok(Some(path));
                }
            } else {
                bail!("package import target `{target}` is not a static contained module target");
            }
        }
        Ok(None)
    }

    /// Resolves package self-references before declared workspace packages, enforcing the mapping-depth limit.
    fn resolve_workspace_specifier(
        &mut self,
        importer: &Path,
        specifier: &str,
        root: &Path,
        depth: usize,
    ) -> Result<Option<PathBuf>> {
        if depth >= MAX_PROJECT_CONFIG_EXTENDS_DEPTH {
            bail!(
                "package mapping exceeds the {}-step resolution limit",
                MAX_PROJECT_CONFIG_EXTENDS_DEPTH
            );
        }
        let Some((name, subpath)) = split_package_specifier(specifier) else {
            return Ok(None);
        };
        if let Some(scope) = self.nearest_package(importer, root)? {
            if scope.name.as_deref() == Some(name) {
                return self.resolve_package_entry(&scope, subpath, root);
            }
        }
        let package = self.workspace_packages(root)?.get(name).cloned();
        match package {
            Some(package) => self.resolve_package_entry(&package, subpath, root),
            None => Ok(None),
        }
    }

    /// Resolves package exports when present, otherwise trying a contained subpath, main entry, or index module.
    fn resolve_package_entry(
        &mut self,
        package: &PackageInfo,
        subpath: &str,
        root: &Path,
    ) -> Result<Option<PathBuf>> {
        let key = if subpath.is_empty() {
            ".".to_owned()
        } else {
            format!("./{subpath}")
        };
        if let Some(exports) = package.exports.as_ref() {
            let Some(targets) = package_map_targets(exports, &key, "package exports")? else {
                return Ok(None);
            };
            for target in targets {
                reject_invalid_package_target(&target, "package export target")?;
                if let Some(path) = self.resolve_path(&package.root.join(target), &package.root)? {
                    if !path.starts_with(root) {
                        bail!(
                            "package export target resolves outside the analysis root {}",
                            root.display()
                        );
                    }
                    return Ok(Some(path));
                }
            }
            return Ok(None);
        }
        if !subpath.is_empty() {
            return self.resolve_path(&package.root.join(subpath), &package.root);
        }
        if let Some(main) = package.main.as_deref() {
            if Path::new(main).is_absolute() || main.starts_with("../") {
                bail!(
                    "package main `{main}` resolves outside package {}",
                    package.root.display()
                );
            }
            if let Some(path) = self.resolve_path(&package.root.join(main), &package.root)? {
                return Ok(Some(path));
            }
        }
        self.resolve_path(&package.root.join("index"), &package.root)
    }

    /// Loads the first package manifest found walking from the importer directory to the analysis root.
    fn nearest_package(&mut self, importer: &Path, root: &Path) -> Result<Option<PackageInfo>> {
        let directory = importer
            .parent()
            .unwrap_or(root)
            .canonicalize()
            .with_context(|| {
                format!("failed to resolve package scope for {}", importer.display())
            })?;
        for ancestor in directory.ancestors() {
            if !ancestor.starts_with(root) {
                break;
            }
            let manifest = ancestor.join("package.json");
            if manifest.is_file() {
                return self.load_package(ancestor, root).map(Some);
            }
            if ancestor == root {
                break;
            }
        }
        Ok(None)
    }

    /// The workspace packages under `root` by name, scanned once per run; a failed scan is kept
    /// and reported again without rescanning.
    fn workspace_packages(&mut self, root: &Path) -> Result<&BTreeMap<String, PackageInfo>> {
        let scanned = match self.workspace_packages.take() {
            Some(scanned) => scanned,
            None => match self.scan_workspaces(root) {
                Ok(packages) => Ok(packages),
                Err(error) => Err(KeptFailure::new(&error).ok_or(error)?),
            },
        };
        match self.workspace_packages.insert(scanned) {
            Ok(packages) => Ok(packages),
            Err(failure) => Err(failure.error()),
        }
    }

    /// Walks `root` for directories matching the root manifest's workspace patterns and loads each
    /// package, refusing duplicate names and scans past `MAX_WORKSPACE_SCAN_ENTRIES`.
    fn scan_workspaces(&mut self, root: &Path) -> Result<BTreeMap<String, PackageInfo>> {
        let root_package = if root.join("package.json").is_file() {
            Some(self.load_package(root, root)?)
        } else {
            None
        };
        let mut packages = BTreeMap::new();
        if let Some(package) = root_package.as_ref() {
            if let Some(name) = package.name.as_ref() {
                packages.insert(name.clone(), package.clone());
            }
        }
        let patterns = root_package.map_or_else(Vec::new, |package| package.workspaces);
        if patterns.is_empty() {
            return Ok(packages);
        }
        let globs = compile_workspace_globs(&patterns)?;
        let mut builder = ignore::WalkBuilder::new(root);
        builder
            .hidden(false)
            .git_ignore(false)
            .git_exclude(false)
            .ignore(false)
            .follow_links(false)
            .filter_entry(|entry| {
                entry.depth() == 0
                    || !entry.file_name().to_str().is_some_and(|name| {
                        matches!(name, ".git" | "node_modules" | "target" | ".features-gen")
                    })
            });
        let mut visited = WorkBudget::new(MAX_WORKSPACE_SCAN_ENTRIES);
        for entry in builder.build() {
            let entry = entry.context("failed while scanning declared workspace packages")?;
            if visited.charge(1) {
                bail!(
                    "workspace scan exceeds the {}-entry limit",
                    MAX_WORKSPACE_SCAN_ENTRIES
                );
            }
            if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                continue;
            }
            let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
            if relative.as_os_str().is_empty() || !globs.is_match(relative) {
                continue;
            }
            if !entry.path().join("package.json").is_file() {
                continue;
            }
            let package_root = self.canonical_contained(entry.path(), root, "workspace package")?;
            let package = self.load_package(&package_root, root)?;
            let Some(name) = package.name.clone() else {
                continue;
            };
            if packages.insert(name.clone(), package).is_some() {
                bail!(
                    "duplicate workspace package name `{name}` inside analysis root {}",
                    root.display()
                );
            }
        }
        Ok(packages)
    }

    /// Caches a contained static package manifest, or the failure to load it, by canonical root.
    fn load_package(&mut self, package_root: &Path, root: &Path) -> Result<PackageInfo> {
        let package_root = self.canonical_contained(package_root, root, "package")?;
        let loaded = match self.packages.get(&package_root) {
            Some(loaded) => loaded.clone(),
            None => {
                let loaded = match self.read_package(&package_root) {
                    Ok(package) => Ok(package),
                    Err(error) => Err(KeptFailure::new(&error).ok_or(error)?),
                };
                self.packages.insert(package_root, loaded.clone());
                loaded
            }
        };
        loaded.map_err(|failure| failure.error())
    }

    /// Reads a contained static package manifest, validating fields and charging its import and
    /// export mappings.
    fn read_package(&mut self, package_root: &Path) -> Result<PackageInfo> {
        let manifest = self.canonical_contained(
            &package_root.join("package.json"),
            package_root,
            "package manifest",
        )?;
        let text = self.read_metadata(&manifest, "package manifest")?;
        let value: Value = serde_json::from_str(&text).with_context(|| {
            format!(
                "failed to parse static package manifest {}",
                manifest.display()
            )
        })?;
        let object = value.as_object().with_context(|| {
            format!(
                "package manifest {} must contain an object",
                manifest.display()
            )
        })?;
        let imports = object.get("imports").cloned();
        let exports = object.get("exports").cloned();
        self.charge_mappings(map_size(imports.as_ref()) + map_size(exports.as_ref()))?;
        Ok(PackageInfo {
            root: package_root.to_owned(),
            name: optional_string(object, "name", &manifest)?,
            imports,
            exports,
            main: optional_string(object, "main", &manifest)?,
            tsconfig: optional_string(object, "tsconfig", &manifest)?,
            workspaces: workspace_patterns(object.get("workspaces"), &manifest)?,
        })
    }

    /// Tries supported exact, extension-substituted, suffixed, and index paths, rejecting canonical targets outside the container.
    fn resolve_path(&self, base: &Path, container: &Path) -> Result<Option<PathBuf>> {
        let mut candidates = vec![base.to_owned()];
        if let Some(substitutions) = extension_substitutions(base) {
            candidates.extend(substitutions);
        }
        for suffix in MODULE_SUFFIXES {
            candidates.push(with_appended_suffix(base, suffix));
        }
        for suffix in MODULE_SUFFIXES {
            candidates.push(base.join(format!("index{suffix}")));
        }
        for candidate in candidates {
            if ecmascript_language(&candidate).is_none() || !candidate.is_file() {
                continue;
            }
            let canonical = candidate.canonicalize().with_context(|| {
                format!(
                    "failed to resolve static module target {}",
                    candidate.display()
                )
            })?;
            if !canonical.starts_with(container) {
                bail!(
                    "static module target {} resolves outside package or analysis root {}",
                    candidate.display(),
                    container.display()
                );
            }
            return Ok(Some(canonical));
        }
        Ok(None)
    }

    /// Canonicalizes a path and rejects it when it escapes the supplied root.
    fn canonical_contained(&self, path: &Path, root: &Path, kind: &str) -> Result<PathBuf> {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("failed to resolve {kind} {}", path.display()))?;
        if !canonical.starts_with(root) {
            if kind == "package manifest" {
                bail!(
                    "package manifest {} resolves outside package {}",
                    path.display(),
                    root.display()
                );
            }
            bail!(
                "{kind} {} resolves outside the analysis root {}",
                path.display(),
                root.display()
            );
        }
        Ok(canonical)
    }

    /// Reads UTF-8 metadata within per-file and aggregate byte limits, charging successful reads against the file limit.
    fn read_metadata(&mut self, path: &Path, kind: &'static str) -> Result<String> {
        if !self.metadata_files.fits(1) {
            bail!(
                "project module metadata exceeds the {}-file limit",
                self.metadata_files.limit()
            );
        }
        let text = read_utf8(path, kind, MAX_CONFIG_INPUT_BYTES)?;
        if !self.metadata_bytes.admit(text.len()) {
            bail!(
                "project module metadata exceeds the {}-byte aggregate limit",
                self.metadata_bytes.limit()
            );
        }
        // Checked by `fits` above; charged only now so a failed or refused read costs no file.
        self.metadata_files.charge(1);
        Ok(text)
    }

    /// Adds mapping work to the aggregate budget, returning an error rather than exceeding the limit.
    fn charge_mappings(&mut self, count: usize) -> Result<()> {
        if !self.mappings.admit(count) {
            bail!(
                "project module metadata exceeds the {}-mapping limit",
                self.mappings.limit()
            );
        }
        Ok(())
    }
}

/// Checks for a nonempty specifier without a relative prefix, leading slash, or URL scheme separator.
fn is_bare_module(module: &str) -> bool {
    !module.is_empty()
        && !is_relative_module(module)
        && !module.starts_with('/')
        && !module.contains("://")
}

/// Splits a scoped or unscoped package name from its optional subpath, rejecting a scope without a slash.
fn split_package_specifier(specifier: &str) -> Option<(&str, &str)> {
    if specifier.starts_with('@') {
        let first = specifier.find('/')?;
        let after_scope = &specifier[first + 1..];
        let second = after_scope.find('/').map(|offset| first + 1 + offset);
        Some(match second {
            Some(index) => (&specifier[..index], &specifier[index + 1..]),
            None => (specifier, ""),
        })
    } else {
        Some(match specifier.find('/') {
            Some(index) => (&specifier[..index], &specifier[index + 1..]),
            None => (specifier, ""),
        })
    }
}

/// Chooses the matching alias with maximal exact-match, prefix, suffix, and lexical specificity.
fn best_mapping<'a>(
    mappings: &'a [PathMapping],
    specifier: &str,
) -> Option<(&'a PathMapping, Option<String>)> {
    mappings
        .iter()
        .filter_map(|mapping| {
            pattern_capture(&mapping.pattern, specifier).map(|capture| (mapping, capture))
        })
        .max_by(|(left, _), (right, _)| {
            mapping_specificity(&left.pattern).cmp(&mapping_specificity(&right.pattern))
        })
}

/// Builds the ordering key that favors exact mappings, then longer wildcard prefixes and suffixes.
fn mapping_specificity(pattern: &str) -> (bool, usize, usize, &str) {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => (false, prefix.len(), suffix.len(), pattern),
        None => (true, pattern.len(), 0, pattern),
    }
}

/// Matches an exact or single-star pattern, distinguishing no match from a match without a wildcard capture.
fn pattern_capture(pattern: &str, value: &str) -> Option<Option<String>> {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return (pattern == value).then_some(None);
    };
    if value.len() < prefix.len() + suffix.len()
        || !value.starts_with(prefix)
        || !value.ends_with(suffix)
    {
        return None;
    }
    Some(Some(
        value[prefix.len()..value.len() - suffix.len()].to_owned(),
    ))
}

/// Replaces wildcard occurrences with the capture, or copies the target unchanged when no capture exists.
fn substitute_star(target: &str, capture: Option<&str>) -> String {
    capture.map_or_else(|| target.to_owned(), |capture| target.replace('*', capture))
}

/// Rejects mapping strings containing more than one wildcard.
fn validate_single_star(value: &str, kind: &str) -> Result<()> {
    if value.bytes().filter(|byte| *byte == b'*').count() > 1 {
        bail!("{kind} `{value}` contains more than one wildcard");
    }
    Ok(())
}

/// Copies a JSON string or string array, rejecting other values and non-string array entries.
fn string_or_string_array(value: &Value) -> Result<Vec<String>> {
    if let Some(value) = value.as_str() {
        return Ok(vec![value.to_owned()]);
    }
    let Some(values) = value.as_array() else {
        bail!("expected a string or string array");
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .context("expected a string array")
        })
        .collect()
}

/// Selects an exact or most-specific package mapping and validates subpath and condition-key structure.
fn package_map_targets(map: &Value, key: &str, kind: &str) -> Result<Option<Vec<String>>> {
    if !map.is_object() {
        return (key == ".")
            .then(|| target_candidates(map, None, kind))
            .transpose();
    }
    let Some(object) = map.as_object() else {
        return Ok(None);
    };
    if kind == "package imports" {
        for pattern in object.keys() {
            if !pattern.starts_with('#') || matches!(pattern.as_str(), "#" | "#/") {
                bail!("package imports key `{pattern}` must begin with `#` and name a module");
            }
        }
    }
    let subpath_keys = object
        .keys()
        .filter(|candidate| candidate.starts_with('.') || candidate.starts_with('#'))
        .count();
    if subpath_keys != 0 && subpath_keys != object.len() {
        bail!("{kind} cannot mix subpath mappings with condition keys");
    }
    if subpath_keys == 0 {
        return (key == ".")
            .then(|| target_candidates(map, None, kind))
            .transpose();
    }
    let required_prefix = if kind == "package imports" { '#' } else { '.' };
    for pattern in object.keys() {
        if !pattern.starts_with(required_prefix) {
            bail!("{kind} key `{pattern}` has the wrong subpath prefix");
        }
        if kind == "package exports" && pattern != "." && !pattern.starts_with("./") {
            bail!("package exports key `{pattern}` must be `.` or begin with `./`");
        }
        validate_single_star(pattern, &format!("{kind} key"))?;
    }
    if let Some(target) = object.get(key) {
        return target_candidates(target, None, kind).map(Some);
    }
    let matched = object
        .iter()
        .filter_map(|(pattern, target)| {
            pattern_capture(pattern, key).map(|capture| (pattern, target, capture))
        })
        .max_by(|(left, _, _), (right, _, _)| {
            mapping_specificity(left).cmp(&mapping_specificity(right))
        });
    matched
        .map(|(_, target, capture)| target_candidates(target, capture.as_deref(), kind))
        .transpose()
}

/// Flattens static target alternatives, choosing the first supported condition in map order and rejecting invalid shapes.
fn target_candidates(value: &Value, capture: Option<&str>, kind: &str) -> Result<Vec<String>> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(target) => {
            validate_single_star(target, &format!("{kind} target"))?;
            if capture.is_none() && target.contains('*') {
                bail!("{kind} target `{target}` uses a wildcard without a wildcard key");
            }
            Ok(vec![substitute_star(target, capture)])
        }
        Value::Array(values) => {
            let mut targets = Vec::new();
            for value in values {
                targets.extend(target_candidates(value, capture, kind)?);
            }
            Ok(targets)
        }
        Value::Object(conditions) => {
            if conditions
                .keys()
                .any(|condition| condition.starts_with('.') || condition.starts_with('#'))
            {
                bail!("{kind} cannot mix condition keys with nested subpath mappings");
            }
            for (condition, value) in conditions {
                if matches!(condition.as_str(), "node" | "import" | "default") {
                    return target_candidates(value, capture, kind);
                }
            }
            Ok(Vec::new())
        }
        _ => bail!("{kind} target must be a string, array, condition object, or null"),
    }
}

/// Rejects targets without a relative package prefix or containing dot, parent, or node_modules segments.
fn reject_invalid_package_target(target: &str, kind: &str) -> Result<()> {
    if !target.starts_with("./")
        || target
            // A backslash separates path components on Windows, so it must not hide a segment.
            .split(['/', '\\'])
            .skip(1)
            .any(|segment| matches!(segment, "." | ".." | "node_modules"))
    {
        bail!("{kind} `{target}` resolves outside package or uses an invalid segment");
    }
    Ok(())
}

/// Reads direct or object-form workspace patterns, treating absent or null patterns as empty.
fn workspace_patterns(value: Option<&Value>, manifest: &Path) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = if let Some(object) = value.as_object() {
        object.get("packages").unwrap_or(&Value::Null)
    } else {
        value
    };
    if values.is_null() {
        return Ok(Vec::new());
    }
    string_or_string_array(values).with_context(|| {
        format!(
            "workspaces in {} must be a string array",
            manifest.display()
        )
    })
}

/// Reads an optional manifest string field, rejecting a present value of another JSON type.
fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    manifest: &Path,
) -> Result<Option<String>> {
    object
        .get(key)
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .with_context(|| format!("{key} in {} must be a string", manifest.display()))
        })
        .transpose()
}

/// Compiles portable workspace globs, rejecting negative or absolute patterns and invalid glob syntax.
fn compile_workspace_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        // package.json workspace patterns always use slash-separated portable syntax. A leading
        // slash must therefore be rejected even on Windows, where Path::is_absolute only treats
        // drive- or UNC-prefixed paths as absolute.
        if pattern.starts_with('!') || pattern.starts_with('/') || Path::new(pattern).is_absolute()
        {
            bail!("workspace pattern `{pattern}` is not a supported contained positive glob");
        }
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .with_context(|| format!("invalid workspace pattern `{pattern}`"))?,
        );
    }
    builder
        .build()
        .context("failed to compile workspace patterns")
}

/// Appends a suffix to the whole path without replacing an existing extension.
fn with_appended_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

/// Files a config base may name, as TypeScript resolves them: the exact path, the path with `.json`
/// appended (so `tsconfig.base` means `tsconfig.base.json`, not `tsconfig.json`), and a directory's
/// `tsconfig.json`.
fn config_base_candidates(base: &Path) -> [PathBuf; 3] {
    [
        base.to_owned(),
        with_appended_suffix(base, ".json"),
        base.join("tsconfig.json"),
    ]
}

/// A config base with a script extension would have to be executed, which static resolution refuses.
/// Any other suffix — `tsconfig.base`, `tsconfig.app` — is a JSON file named without `.json`.
fn is_executable_config(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "js" | "cjs" | "mjs" | "ts" | "cts" | "mts"))
}

/// Returns supported TypeScript source replacements for JavaScript extensions, or `None` for other suffixes.
fn extension_substitutions(path: &Path) -> Option<Vec<PathBuf>> {
    let extension = path.extension()?.to_str()?;
    let replacements: &[&str] = match extension {
        "js" => &["ts", "tsx"],
        "mjs" => &["mts"],
        "cjs" => &["cts"],
        "jsx" => &["tsx"],
        _ => return None,
    };
    Some(
        replacements
            .iter()
            .map(|replacement| path.with_extension(replacement))
            .collect(),
    )
}

/// Canonicalizes the existing prefix of a config path within the root and restores its missing trailing components.
fn canonicalize_existing_ancestor(path: &Path, root: &Path) -> Result<PathBuf> {
    let mut existing = path;
    let mut tail = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            bail!(
                "project config path {} has no existing ancestor",
                path.display()
            );
        };
        tail.push(name.to_owned());
        existing = existing
            .parent()
            .context("project config path has no parent")?;
    }
    let mut canonical = existing
        .canonicalize()
        .with_context(|| format!("failed to resolve project config path {}", path.display()))?;
    if !canonical.starts_with(root) {
        bail!(
            "project config path {} resolves outside the analysis root {}",
            path.display(),
            root.display()
        );
    }
    for component in tail.into_iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}

/// Counts immediate object entries, returning zero for missing or non-object values.
fn map_size(value: Option<&Value>) -> usize {
    value.and_then(Value::as_object).map_or(0, Map::len)
}

/// Removes a BOM, blanks comments and trailing commas while preserving line breaks, and rejects unterminated block comments.
fn normalize_jsonc(source: &str) -> Result<String> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let bytes = source.as_bytes();
    let mut stripped = Vec::with_capacity(bytes.len());
    let mut index = 0_usize;
    let mut quote = None;
    let mut escaped = false;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active_quote) = quote {
            stripped.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            quote = Some(byte);
            stripped.push(byte);
            index += 1;
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            stripped.extend_from_slice(b"  ");
            index += 2;
            while index < bytes.len() && !matches!(bytes[index], b'\n' | b'\r') {
                stripped.push(b' ');
                index += 1;
            }
        } else if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            stripped.extend_from_slice(b"  ");
            index += 2;
            let mut closed = false;
            while index < bytes.len() {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    stripped.extend_from_slice(b"  ");
                    index += 2;
                    closed = true;
                    break;
                }
                stripped.push(if matches!(bytes[index], b'\n' | b'\r') {
                    bytes[index]
                } else {
                    b' '
                });
                index += 1;
            }
            if !closed {
                bail!("unterminated block comment in static project config");
            }
        } else {
            stripped.push(byte);
            index += 1;
        }
    }
    let mut normalized = stripped.clone();
    quote = None;
    escaped = false;
    for index in 0..stripped.len() {
        let byte = stripped[index];
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active_quote {
                quote = None;
            }
            continue;
        }
        if byte == b'"' {
            quote = Some(byte);
        } else if byte == b',' {
            let next = stripped[index + 1..]
                .iter()
                .copied()
                .find(|candidate| !candidate.is_ascii_whitespace());
            if matches!(next, Some(b'}' | b']')) {
                normalized[index] = b' ';
            }
        }
    }
    String::from_utf8(normalized)
        .context("static project config normalization produced invalid UTF-8")
}

#[cfg(test)]
mod tests;
