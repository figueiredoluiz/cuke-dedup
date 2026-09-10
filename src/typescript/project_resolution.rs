use super::ast::is_relative_module;
use crate::resource_limits::{
    read_utf8, MAX_CONFIG_INPUT_BYTES, MAX_PROJECT_CONFIG_EXTENDS_DEPTH,
    MAX_PROJECT_METADATA_BYTES, MAX_PROJECT_METADATA_FILES, MAX_PROJECT_MODULE_MAPPINGS,
    MAX_WORKSPACE_SCAN_ENTRIES,
};
use crate::source_adapter::language_for_path;
use anyhow::{bail, Context, Result};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const MODULE_SUFFIXES: [&str; 8] = [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"];

#[derive(Clone, Debug, Default)]
struct EffectiveProjectConfig {
    base_url: Option<PathBuf>,
    paths: Option<Vec<PathMapping>>,
}

impl EffectiveProjectConfig {
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
    workspaces: Vec<String>,
}

#[derive(Default)]
pub(super) struct ProjectResolution {
    root: Option<PathBuf>,
    config_for_directory: BTreeMap<PathBuf, Option<PathBuf>>,
    configs: BTreeMap<PathBuf, EffectiveProjectConfig>,
    config_active: BTreeSet<PathBuf>,
    packages: BTreeMap<PathBuf, PackageInfo>,
    workspace_packages: Option<BTreeMap<String, PackageInfo>>,
    metadata_files: usize,
    metadata_bytes: usize,
    mappings: usize,
}

impl ProjectResolution {
    pub(super) fn for_root(root: &Path) -> Self {
        Self {
            root: root.canonicalize().ok(),
            ..Self::default()
        }
    }

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
        let loaded = result?;
        self.configs.insert(canonical.clone(), loaded);
        self.configs
            .get(&canonical)
            .context("project config cache insertion failed")
    }

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
                let base = self.resolve_extends(path, &entry, root)?;
                effective.merge(self.load_project_config(&base, root, depth + 1)?.clone());
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

    fn resolve_extends(&self, config: &Path, extends: &str, root: &Path) -> Result<PathBuf> {
        if !is_relative_module(extends) {
            bail!(
                "project config extends only supports static JSON paths inside the analysis root: `{extends}` in {}",
                config.display()
            );
        }
        let base = config.parent().unwrap_or(root).join(extends);
        if base
            .extension()
            .is_some_and(|extension| extension != "json")
        {
            bail!(
                "project config extends only supports static JSON paths; `{extends}` in {} would require executing repository code",
                config.display()
            );
        }
        for candidate in [
            base.clone(),
            base.with_extension("json"),
            base.join("tsconfig.json"),
        ] {
            if candidate.is_file() {
                return self.canonical_contained(&candidate, root, "project config extends target");
            }
        }
        bail!(
            "project config extends target `{extends}` from {} could not be resolved",
            config.display()
        )
    }

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
        self.initialize_workspaces(root)?;
        let package = self
            .workspace_packages
            .as_ref()
            .and_then(|packages| packages.get(name))
            .cloned();
        match package {
            Some(package) => self.resolve_package_entry(&package, subpath, root),
            None => Ok(None),
        }
    }

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

    fn initialize_workspaces(&mut self, root: &Path) -> Result<()> {
        if self.workspace_packages.is_some() {
            return Ok(());
        }
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
            self.workspace_packages = Some(packages);
            return Ok(());
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
        let mut visited = 0_usize;
        for entry in builder.build() {
            let entry = entry.context("failed while scanning declared workspace packages")?;
            visited = visited.saturating_add(1);
            if visited > MAX_WORKSPACE_SCAN_ENTRIES {
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
        self.workspace_packages = Some(packages);
        Ok(())
    }

    fn load_package(&mut self, package_root: &Path, root: &Path) -> Result<PackageInfo> {
        let package_root = self.canonical_contained(package_root, root, "package")?;
        if let Some(package) = self.packages.get(&package_root) {
            return Ok(package.clone());
        }
        let manifest = self.canonical_contained(
            &package_root.join("package.json"),
            &package_root,
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
        let package = PackageInfo {
            root: package_root.clone(),
            name: optional_string(object, "name", &manifest)?,
            imports,
            exports,
            main: optional_string(object, "main", &manifest)?,
            workspaces: workspace_patterns(object.get("workspaces"), &manifest)?,
        };
        self.packages.insert(package_root, package.clone());
        Ok(package)
    }

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
            if language_for_path(&candidate).is_none() || !candidate.is_file() {
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

    fn read_metadata(&mut self, path: &Path, kind: &'static str) -> Result<String> {
        if self.metadata_files >= MAX_PROJECT_METADATA_FILES {
            bail!(
                "project module metadata exceeds the {}-file limit",
                MAX_PROJECT_METADATA_FILES
            );
        }
        let text = read_utf8(path, kind, MAX_CONFIG_INPUT_BYTES)?;
        let total = self.metadata_bytes.saturating_add(text.len());
        if total > MAX_PROJECT_METADATA_BYTES {
            bail!(
                "project module metadata exceeds the {}-byte aggregate limit",
                MAX_PROJECT_METADATA_BYTES
            );
        }
        self.metadata_files += 1;
        self.metadata_bytes = total;
        Ok(text)
    }

    fn charge_mappings(&mut self, count: usize) -> Result<()> {
        let total = self.mappings.saturating_add(count);
        if total > MAX_PROJECT_MODULE_MAPPINGS {
            bail!(
                "project module metadata exceeds the {}-mapping limit",
                MAX_PROJECT_MODULE_MAPPINGS
            );
        }
        self.mappings = total;
        Ok(())
    }
}

fn is_bare_module(module: &str) -> bool {
    !module.is_empty()
        && !is_relative_module(module)
        && !module.starts_with('/')
        && !module.contains("://")
}

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

fn mapping_specificity(pattern: &str) -> (bool, usize, usize, &str) {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => (false, prefix.len(), suffix.len(), pattern),
        None => (true, pattern.len(), 0, pattern),
    }
}

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

fn substitute_star(target: &str, capture: Option<&str>) -> String {
    capture.map_or_else(|| target.to_owned(), |capture| target.replace('*', capture))
}

fn validate_single_star(value: &str, kind: &str) -> Result<()> {
    if value.bytes().filter(|byte| *byte == b'*').count() > 1 {
        bail!("{kind} `{value}` contains more than one wildcard");
    }
    Ok(())
}

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

fn reject_invalid_package_target(target: &str, kind: &str) -> Result<()> {
    if !target.starts_with("./")
        || target
            .split('/')
            .skip(1)
            .any(|segment| matches!(segment, "." | ".." | "node_modules"))
    {
        bail!("{kind} `{target}` resolves outside package or uses an invalid segment");
    }
    Ok(())
}

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

fn compile_workspace_globs(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        if pattern.starts_with('!') || Path::new(pattern).is_absolute() {
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

fn with_appended_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}

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

fn map_size(value: Option<&Value>) -> usize {
    value.and_then(Value::as_object).map_or(0, Map::len)
}

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
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn jsonc_preserves_comment_text_in_strings_and_removes_trailing_commas() {
        let source = "\u{feff}".to_owned()
            + r#"{"url":"https://example.test/*not-comment*/",// comment
          "paths":{"@/*":["src/*",],},}"#;
        let value: Value = serde_json::from_str(&normalize_jsonc(&source).unwrap()).unwrap();
        assert_eq!(value["url"], "https://example.test/*not-comment*/");
        assert_eq!(value["paths"]["@/*"][0], "src/*");
    }

    #[test]
    fn paths_imports_and_workspace_exports_resolve_statically() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("support")).unwrap();
        fs::create_dir_all(directory.path().join("packages/bdd/src")).unwrap();
        fs::write(
            directory.path().join("package.json"),
            r##"{"workspaces":["packages/*"],"imports":{"#bdd":"./support/world.ts"}}"##,
        )
        .unwrap();
        fs::write(
            directory.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@support/*":["support/*"]}}}"#,
        )
        .unwrap();
        fs::write(directory.path().join("support/world.ts"), "").unwrap();
        fs::write(
            directory.path().join("packages/bdd/package.json"),
            r#"{"name":"@example/bdd","exports":{".":"./src/index.ts"}}"#,
        )
        .unwrap();
        fs::write(directory.path().join("packages/bdd/src/index.ts"), "").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        let boundary = directory.path().canonicalize().unwrap();
        let mut resolver = ProjectResolution::for_root(directory.path());

        for specifier in ["@support/world", "#bdd", "@example/bdd"] {
            assert!(
                resolver
                    .resolve(&importer, specifier, &boundary)
                    .unwrap()
                    .is_some(),
                "{specifier}"
            );
        }
    }

    #[test]
    fn package_import_chains_respect_the_resolution_depth_limit() {
        let directory = tempfile::tempdir().unwrap();
        let mut imports = Map::new();
        for index in 0..=MAX_PROJECT_CONFIG_EXTENDS_DEPTH {
            imports.insert(
                format!("#level{index}"),
                Value::String(format!("#level{}", index + 1)),
            );
        }
        imports.insert(
            format!("#level{}", MAX_PROJECT_CONFIG_EXTENDS_DEPTH + 1),
            Value::String("./world.ts".to_owned()),
        );
        fs::write(
            directory.path().join("package.json"),
            serde_json::json!({ "imports": imports }).to_string(),
        )
        .unwrap();
        fs::write(directory.path().join("world.ts"), "").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        let boundary = directory.path().canonicalize().unwrap();
        let mut resolver = ProjectResolution::for_root(directory.path());

        let error = resolver
            .resolve(&importer, "#level0", &boundary)
            .unwrap_err();
        assert!(
            error.to_string().contains("16-step resolution limit"),
            "{error:#}"
        );
    }

    #[test]
    fn package_import_to_workspace_chains_share_the_resolution_depth_limit() {
        let directory = tempfile::tempdir().unwrap();
        let mut imports = Map::new();
        for index in 0..MAX_PROJECT_CONFIG_EXTENDS_DEPTH - 1 {
            imports.insert(
                format!("#level{index}"),
                Value::String(format!("#level{}", index + 1)),
            );
        }
        imports.insert(
            format!("#level{}", MAX_PROJECT_CONFIG_EXTENDS_DEPTH - 1),
            Value::String("@example/bdd".to_owned()),
        );
        fs::create_dir_all(directory.path().join("packages/bdd/src")).unwrap();
        fs::write(
            directory.path().join("package.json"),
            serde_json::json!({
                "imports": imports,
                "workspaces": ["packages/*"]
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            directory.path().join("packages/bdd/package.json"),
            r#"{"name":"@example/bdd","exports":"./src/index.ts"}"#,
        )
        .unwrap();
        fs::write(directory.path().join("packages/bdd/src/index.ts"), "").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        let boundary = directory.path().canonicalize().unwrap();
        let mut resolver = ProjectResolution::for_root(directory.path());

        let error = resolver
            .resolve(&importer, "#level0", &boundary)
            .unwrap_err();
        assert!(
            error.to_string().contains("16-step resolution limit"),
            "{error:#}"
        );
    }

    #[test]
    fn multiple_extends_use_later_base_then_child_precedence() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("one")).unwrap();
        fs::create_dir(directory.path().join("two")).unwrap();
        fs::write(directory.path().join("one/world.ts"), "").unwrap();
        fs::write(directory.path().join("two/world.ts"), "").unwrap();
        fs::write(
            directory.path().join("first.json"),
            r#"{"compilerOptions":{"paths":{"@bdd/*":["one/*"]}}}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join("second.json"),
            r#"{"compilerOptions":{"paths":{"@bdd/*":["two/*"]}}}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join("tsconfig.json"),
            r#"{"extends":["./first.json","./second.json"]}"#,
        )
        .unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        let boundary = directory.path().canonicalize().unwrap();
        let mut resolver = ProjectResolution::for_root(directory.path());
        let resolved = resolver
            .resolve(&importer, "@bdd/world", &boundary)
            .unwrap()
            .unwrap();
        assert!(resolved.ends_with("two/world.ts"));
    }

    #[test]
    fn later_extends_inherits_fields_it_does_not_specify() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("base")).unwrap();
        fs::write(directory.path().join("base/world.ts"), "").unwrap();
        fs::write(
            directory.path().join("first.json"),
            r#"{"compilerOptions":{"baseUrl":"base"}}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join("second.json"),
            r#"{"compilerOptions":{"paths":{"@bdd/*":["*"]}}}"#,
        )
        .unwrap();
        fs::write(
            directory.path().join("tsconfig.json"),
            r#"{"extends":["./first.json","./second.json"]}"#,
        )
        .unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        let boundary = directory.path().canonicalize().unwrap();
        let mut resolver = ProjectResolution::for_root(directory.path());

        let resolved = resolver
            .resolve(&importer, "@bdd/world", &boundary)
            .unwrap()
            .unwrap();
        assert!(resolved.ends_with("base/world.ts"));
    }

    #[test]
    fn module_files_precede_indexes_and_js_specifiers_substitute_typescript() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("thing")).unwrap();
        fs::write(directory.path().join("thing.ts"), "").unwrap();
        fs::write(directory.path().join("thing/index.ts"), "").unwrap();
        fs::write(directory.path().join("other.ts"), "").unwrap();
        fs::write(directory.path().join("other.js.ts"), "").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        let boundary = directory.path().canonicalize().unwrap();
        let mut resolver = ProjectResolution::for_root(directory.path());

        assert!(resolver
            .resolve(&importer, "./thing", &boundary)
            .unwrap()
            .unwrap()
            .ends_with("thing.ts"));
        assert!(resolver
            .resolve(&importer, "./other.js", &boundary)
            .unwrap()
            .unwrap()
            .ends_with("other.ts"));
    }

    #[test]
    fn conditional_targets_use_first_active_source_order_key() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("package.json"),
            r##"{"imports":{"#bdd":{"default":"./default.ts","node":"./node.ts"}}}"##,
        )
        .unwrap();
        fs::write(directory.path().join("default.ts"), "").unwrap();
        fs::write(directory.path().join("node.ts"), "").unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        let boundary = directory.path().canonicalize().unwrap();
        let mut resolver = ProjectResolution::for_root(directory.path());

        assert!(resolver
            .resolve(&importer, "#bdd", &boundary)
            .unwrap()
            .unwrap()
            .ends_with("default.ts"));
    }

    #[test]
    fn package_maps_reject_invalid_key_shapes() {
        for (map, key, kind, expected) in [
            (
                serde_json::json!({"plain": "./target.ts"}),
                "#plain",
                "package imports",
                "must begin with `#`",
            ),
            (
                serde_json::json!({"#": "./target.ts"}),
                "#",
                "package imports",
                "name a module",
            ),
            (
                serde_json::json!({".hidden": "./target.ts"}),
                ".hidden",
                "package exports",
                "must be `.` or begin with `./`",
            ),
        ] {
            let error = package_map_targets(&map, key, kind).unwrap_err();
            assert!(error.to_string().contains(expected), "{kind}: {error}");
        }
    }

    #[test]
    fn cached_configs_cannot_bypass_extends_depth_or_metadata_limits() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let shared = root.join("shared.json");
        fs::write(&shared, "{}").unwrap();
        fs::write(root.join("tsconfig.json"), r#"{"extends":"./0.json"}"#).unwrap();
        for depth in 0..15 {
            fs::write(
                root.join(format!("{depth}.json")),
                if depth == 14 {
                    r#"{"extends":"./shared.json"}"#.to_owned()
                } else {
                    format!(r#"{{"extends":"./{}.json"}}"#, depth + 1)
                },
            )
            .unwrap();
        }

        let mut resolver = ProjectResolution::for_root(&root);
        resolver.load_project_config(&shared, &root, 0).unwrap();
        let error = resolver
            .load_project_config(&root.join("tsconfig.json"), &root, 0)
            .unwrap_err();
        assert!(error.to_string().contains("16-file depth limit"));

        let mut file_limited = ProjectResolution {
            metadata_files: MAX_PROJECT_METADATA_FILES,
            ..ProjectResolution::for_root(&root)
        };
        let error = file_limited
            .read_metadata(&shared, "project config")
            .unwrap_err();
        assert!(error.to_string().contains("1024-file limit"));

        let mut byte_limited = ProjectResolution {
            metadata_bytes: MAX_PROJECT_METADATA_BYTES,
            ..ProjectResolution::for_root(&root)
        };
        let error = byte_limited
            .read_metadata(&shared, "project config")
            .unwrap_err();
        assert!(error.to_string().contains("aggregate limit"));
    }

    #[cfg(unix)]
    #[test]
    fn package_manifest_symlinks_cannot_escape_the_package() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("package.json"), r#"{"name":"outside"}"#).unwrap();
        symlink(
            outside.path().join("package.json"),
            directory.path().join("package.json"),
        )
        .unwrap();

        let root = directory.path().canonicalize().unwrap();
        let mut resolver = ProjectResolution::for_root(&root);
        let error = resolver.load_package(&root, &root).unwrap_err();
        assert!(error.to_string().contains("resolves outside package"));
    }
}
