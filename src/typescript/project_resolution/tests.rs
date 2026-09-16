use super::*;
use std::fs;

fn config_error(source: &str) -> String {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("tsconfig.json"), source).unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut resolver = ProjectResolution::for_root(&root);
    resolver
        .load_project_config(&root.join("tsconfig.json"), &root, 0)
        .unwrap_err()
        .to_string()
}

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
fn pure_resolution_helpers_cover_valid_and_rejected_shapes() {
    assert!(is_bare_module("package"));
    for value in ["", "./local", "../parent", "/absolute", "https://host/mod"] {
        assert!(!is_bare_module(value), "{value}");
    }
    assert_eq!(
        split_package_specifier("pkg/sub/path"),
        Some(("pkg", "sub/path"))
    );
    assert_eq!(split_package_specifier("pkg"), Some(("pkg", "")));
    assert_eq!(
        split_package_specifier("@scope/pkg/sub"),
        Some(("@scope/pkg", "sub"))
    );
    assert_eq!(split_package_specifier("@scope"), None);

    assert_eq!(pattern_capture("exact", "exact"), Some(None));
    assert_eq!(pattern_capture("exact", "other"), None);
    assert_eq!(
        pattern_capture("@app/*/test", "@app/world/test"),
        Some(Some("world".to_owned()))
    );
    assert_eq!(pattern_capture("prefix*suffix", "short"), None);
    assert_eq!(substitute_star("src/*/index", Some("bdd")), "src/bdd/index");
    assert_eq!(substitute_star("src/index", None), "src/index");
    assert!(validate_single_star("one/*/two/*", "mapping").is_err());
    assert!(validate_single_star("one/*", "mapping").is_ok());

    let mappings = vec![
        PathMapping {
            pattern: "@app/*".to_owned(),
            targets: vec![],
            origin: PathBuf::new(),
        },
        PathMapping {
            pattern: "@app/exact".to_owned(),
            targets: vec![],
            origin: PathBuf::new(),
        },
    ];
    assert_eq!(
        best_mapping(&mappings, "@app/exact").unwrap().0.pattern,
        "@app/exact"
    );
    assert!(best_mapping(&mappings, "other").is_none());

    assert_eq!(
        string_or_string_array(&serde_json::json!("a")).unwrap(),
        ["a"]
    );
    assert_eq!(
        string_or_string_array(&serde_json::json!(["a", "b"])).unwrap(),
        ["a", "b"]
    );
    assert!(string_or_string_array(&serde_json::json!(1)).is_err());
    assert!(string_or_string_array(&serde_json::json!(["a", 1])).is_err());

    assert_eq!(
        extension_substitutions(Path::new("entry.js"))
            .unwrap()
            .len(),
        2
    );
    for (path, expected) in [
        ("entry.mjs", "entry.mts"),
        ("entry.cjs", "entry.cts"),
        ("entry.jsx", "entry.tsx"),
    ] {
        assert_eq!(
            extension_substitutions(Path::new(path)).unwrap(),
            [PathBuf::from(expected)]
        );
    }
    assert!(extension_substitutions(Path::new("entry.ts")).is_none());
    assert!(extension_substitutions(Path::new("entry")).is_none());
    assert_eq!(
        with_appended_suffix(Path::new("entry"), ".ts"),
        Path::new("entry.ts")
    );
}

#[test]
fn package_map_targets_cover_conditions_arrays_wildcards_and_errors() {
    assert_eq!(
        package_map_targets(&serde_json::json!("./index.ts"), ".", "package exports").unwrap(),
        Some(vec!["./index.ts".to_owned()])
    );
    assert_eq!(
        package_map_targets(&serde_json::json!("./index.ts"), "./sub", "package exports").unwrap(),
        None
    );
    assert_eq!(
        package_map_targets(
            &serde_json::json!({
                "./*": [null, {"browser": "./ignored.js", "import": "./src/*.ts"}]
            }),
            "./world",
            "package exports"
        )
        .unwrap(),
        Some(vec!["./src/world.ts".to_owned()])
    );
    assert_eq!(
        package_map_targets(
            &serde_json::json!({"browser": "./browser.ts"}),
            ".",
            "package exports"
        )
        .unwrap(),
        Some(Vec::new())
    );
    assert!(package_map_targets(
        &serde_json::json!({".": "./index.ts", "default": "./fallback.ts"}),
        ".",
        "package exports"
    )
    .unwrap_err()
    .to_string()
    .contains("cannot mix"));
    assert!(package_map_targets(
        &serde_json::json!({"./x": {"./nested": "./x.ts"}}),
        "./x",
        "package exports"
    )
    .is_err());
    assert!(target_candidates(&serde_json::json!("./*.ts"), None, "package exports").is_err());
    assert!(target_candidates(&serde_json::json!(42), None, "package exports").is_err());
    for target in [
        "../outside.ts",
        "./../outside.ts",
        "./node_modules/x.ts",
        "bare",
    ] {
        assert!(
            reject_invalid_package_target(target, "target").is_err(),
            "{target}"
        );
    }
    assert!(reject_invalid_package_target("./src/index.ts", "target").is_ok());
}

#[test]
fn project_config_validation_reports_each_unsafe_static_shape() {
    for (source, expected) in [
        ("[]", "must contain an object"),
        (r#"{"extends":42}"#, "string or string array"),
        (r#"{"compilerOptions":[]}"#, "must contain an object"),
        (r#"{"compilerOptions":{"baseUrl":[]}}"#, "must be a string"),
        (
            r#"{"compilerOptions":{"paths":[]}}"#,
            "must contain an object",
        ),
        (
            r#"{"compilerOptions":{"paths":{"@/*/*":["src/*"]}}}"#,
            "more than one wildcard",
        ),
        (
            r#"{"compilerOptions":{"paths":{"@/*":["src/*/*"]}}}"#,
            "more than one wildcard",
        ),
        (
            r#"{"compilerOptions":{"paths":{"@/*":[1]}}}"#,
            "string targets",
        ),
        (
            r#"{"extends":"package-config"}"#,
            "only supports static JSON paths",
        ),
        (r#"{"extends":"./config.js"}"#, "would require executing"),
        (r#"{"extends":"./missing"}"#, "could not be resolved"),
    ] {
        let error = config_error(source);
        assert!(error.contains(expected), "{source}: {error}");
    }

    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("tsconfig.json"),
        r#"{"extends":"./base.json"}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("base.json"),
        r#"{"extends":"./tsconfig.json"}"#,
    )
    .unwrap();
    let root = directory.path().canonicalize().unwrap();
    let error = ProjectResolution::for_root(&root)
        .load_project_config(&root.join("tsconfig.json"), &root, 0)
        .unwrap_err();
    assert!(error.to_string().contains("extends cycle"));
    assert!(normalize_jsonc("{/* never closed")
        .unwrap_err()
        .to_string()
        .contains("unterminated"));
}

#[test]
fn package_metadata_and_workspace_shapes_are_validated() {
    let manifest = Path::new("package.json");
    assert_eq!(
        workspace_patterns(None, manifest).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        workspace_patterns(
            Some(&serde_json::json!({"packages": ["packages/*"]})),
            manifest
        )
        .unwrap(),
        ["packages/*"]
    );
    assert_eq!(
        workspace_patterns(Some(&serde_json::json!({})), manifest).unwrap(),
        Vec::<String>::new()
    );
    assert!(workspace_patterns(Some(&serde_json::json!(42)), manifest).is_err());
    assert!(compile_workspace_globs(&["!excluded".to_owned()]).is_err());
    assert!(compile_workspace_globs(&["/absolute".to_owned()]).is_err());
    assert!(compile_workspace_globs(&["[".to_owned()]).is_err());
    assert!(compile_workspace_globs(&["packages/*".to_owned()])
        .unwrap()
        .is_match("packages/a"));

    let mut object = Map::new();
    object.insert("name".to_owned(), Value::String("pkg".to_owned()));
    assert_eq!(
        optional_string(&object, "name", manifest)
            .unwrap()
            .as_deref(),
        Some("pkg")
    );
    assert_eq!(optional_string(&object, "main", manifest).unwrap(), None);
    object.insert("main".to_owned(), Value::Bool(true));
    assert!(optional_string(&object, "main", manifest).is_err());
    assert_eq!(map_size(Some(&serde_json::json!({"a": 1}))), 1);
    assert_eq!(map_size(Some(&serde_json::json!([]))), 0);
    assert_eq!(map_size(None), 0);

    for (source, expected) in [
        ("[]", "must contain an object"),
        (r#"{"name":1}"#, "name"),
        (r#"{"main":1}"#, "main"),
        (r#"{"workspaces":1}"#, "workspaces"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("package.json"), source).unwrap();
        let root = directory.path().canonicalize().unwrap();
        let error = ProjectResolution::for_root(&root)
            .load_package(&root, &root)
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{source}: {error:#}");
    }
}

#[test]
fn resolver_covers_relative_fallback_package_main_subpaths_and_missing_targets() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("src")).unwrap();
    fs::create_dir_all(directory.path().join("sub")).unwrap();
    fs::write(directory.path().join("src/local.ts"), "").unwrap();
    fs::write(directory.path().join("sub/index.ts"), "").unwrap();
    let importer = directory.path().join("src/steps.ts");
    fs::write(&importer, "").unwrap();
    let boundary = directory.path().canonicalize().unwrap();

    let mut fallback = ProjectResolution::default();
    assert!(fallback
        .resolve(&importer, "./local", &boundary)
        .unwrap()
        .is_some());
    assert!(fallback
        .resolve(&importer, "package", &boundary)
        .unwrap()
        .is_none());

    fs::write(
        directory.path().join("package.json"),
        r#"{"name":"root","main":"src/local.ts"}"#,
    )
    .unwrap();
    let mut resolver = ProjectResolution::for_root(directory.path());
    assert!(resolver
        .resolve(&importer, "root", &boundary)
        .unwrap()
        .is_some());
    assert!(resolver
        .resolve(&importer, "root/sub", &boundary)
        .unwrap()
        .is_some());
    assert!(resolver
        .resolve(&importer, "unknown", &boundary)
        .unwrap()
        .is_none());
    assert!(resolver
        .resolve(&importer, "#missing", &boundary)
        .unwrap()
        .is_none());

    fs::write(
        directory.path().join("package.json"),
        r#"{"name":"root","main":"../outside.ts"}"#,
    )
    .unwrap();
    let mut resolver = ProjectResolution::for_root(directory.path());
    assert!(resolver.resolve(&importer, "root", &boundary).is_err());
}

#[test]
fn canonical_paths_and_mapping_limits_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let nested = canonicalize_existing_ancestor(&root.join("missing/deeper"), &root).unwrap();
    assert_eq!(nested, root.join("missing/deeper"));

    let mut resolver = ProjectResolution {
        mappings: MAX_PROJECT_MODULE_MAPPINGS,
        ..ProjectResolution::for_root(&root)
    };
    assert!(resolver.charge_mappings(1).is_err());

    let outside = tempfile::tempdir().unwrap();
    assert!(canonicalize_existing_ancestor(outside.path(), &root).is_err());
    assert!(resolver
        .nearest_project_config(&outside.path().join("steps.ts"), &root)
        .is_err());
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

/// Pins that JSONC normalization survives the two shapes a hand-written `tsconfig.json` most
/// often mixes: a block comment spanning several lines, and a string carrying backslash escapes
/// around an escaped quote. The comment must collapse to blanks while keeping its newlines, so
/// later parse errors still report the right line, and the escaped quote must not be mistaken for
/// the end of the string — that would make the rest of the file read as string content and the
/// whole config fail to parse.
#[test]
fn jsonc_normalization_spans_block_comments_and_preserves_string_escapes() {
    let source = r#"{
  /* a block comment
     spanning "several" lines */
  "compilerOptions": {
    "paths": { "@/*": ["src\\star \" quoted\\", ] },
  },
}"#;

    let normalized = normalize_jsonc(source).unwrap();

    assert_eq!(normalized.lines().count(), source.lines().count());
    assert!(!normalized.contains("block comment"));
    let value: Value = serde_json::from_str(&normalized).unwrap();
    assert_eq!(
        value["compilerOptions"]["paths"]["@/*"][0],
        "src\\star \" quoted\\"
    );
}

/// Pins every static target shape a package `imports` map may hold: a `#` target that forwards to
/// another entry, a bare workspace package name, a relative file that does not exist, and a target
/// that is neither relative nor bare. Each arm decides whether the step file importing `#bdd` sees
/// the registrations behind it at all, and the rejected shape must keep failing loudly rather than
/// resolving a specifier the analyzer cannot prove is contained.
#[test]
fn package_import_targets_forward_reach_workspaces_dead_end_or_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("support")).unwrap();
    fs::create_dir_all(directory.path().join("packages/bdd/src")).unwrap();
    fs::write(
        directory.path().join("package.json"),
        r##"{
            "name": "root",
            "workspaces": ["packages/*"],
            "imports": {
                "#chain": "#final",
                "#final": "./support/world.ts",
                "#dead": "./missing.ts",
                "#workspace": "@example/bdd",
                "#escaping": "/etc/world.ts",
                "#fallback": ["#dead", "@example/unknown", "./support/world.ts"]
            }
        }"##,
    )
    .unwrap();
    fs::write(directory.path().join("support/world.ts"), "").unwrap();
    fs::write(
        directory.path().join("packages/bdd/package.json"),
        r#"{"name":"@example/bdd","main":"src/index.ts"}"#,
    )
    .unwrap();
    fs::write(directory.path().join("packages/bdd/src/index.ts"), "").unwrap();
    let importer = directory.path().join("steps.ts");
    fs::write(&importer, "").unwrap();
    let boundary = directory.path().canonicalize().unwrap();
    let mut resolver = ProjectResolution::for_root(directory.path());

    assert!(resolver
        .resolve(&importer, "#chain", &boundary)
        .unwrap()
        .unwrap()
        .ends_with("support/world.ts"));
    assert!(resolver
        .resolve(&importer, "#workspace", &boundary)
        .unwrap()
        .unwrap()
        .ends_with("packages/bdd/src/index.ts"));
    assert_eq!(
        resolver.resolve(&importer, "#dead", &boundary).unwrap(),
        None
    );
    // A list of targets is tried in order: the unresolvable `#` and workspace targets are skipped
    // rather than ending the lookup, so the relative target behind them still wins.
    assert!(resolver
        .resolve(&importer, "#fallback", &boundary)
        .unwrap()
        .unwrap()
        .ends_with("support/world.ts"));
    assert_eq!(
        resolver.resolve(&importer, "#unmapped", &boundary).unwrap(),
        None
    );
    let error = resolver
        .resolve(&importer, "#escaping", &boundary)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("not a static contained module target"),
        "{error:#}"
    );
}

/// Pins which directories a declared workspace glob turns into resolvable packages, and how a
/// matched package chooses its entry point. A directory without a manifest and a manifest without
/// a name name no specifier and must be skipped silently; a `main` that does not resolve must fall
/// back to the package index instead of failing the import; and an `exports` map owns the package
/// completely, so a subpath it does not list resolves to nothing rather than to a file on disk.
#[test]
fn workspace_scanning_skips_unnamed_packages_and_entry_points_fall_back_to_the_index() {
    let directory = tempfile::tempdir().unwrap();
    for package in ["fallback", "entry", "exports", "empty", "unnamed", "plain"] {
        fs::create_dir_all(directory.path().join("packages").join(package)).unwrap();
    }
    fs::write(
        directory.path().join("package.json"),
        r#"{"name":"root","workspaces":["packages/*"]}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("packages/fallback/package.json"),
        r#"{"name":"@example/fallback","main":"missing.js"}"#,
    )
    .unwrap();
    fs::write(directory.path().join("packages/fallback/index.ts"), "").unwrap();
    fs::write(
        directory.path().join("packages/entry/package.json"),
        r#"{"name":"@example/entry"}"#,
    )
    .unwrap();
    fs::write(directory.path().join("packages/entry/index.ts"), "").unwrap();
    fs::write(
        directory.path().join("packages/exports/package.json"),
        r#"{"name":"@example/exports","exports":{"./only":"./only.ts"}}"#,
    )
    .unwrap();
    fs::write(directory.path().join("packages/exports/only.ts"), "").unwrap();
    fs::write(directory.path().join("packages/exports/other.ts"), "").unwrap();
    fs::write(
        directory.path().join("packages/empty/package.json"),
        r#"{"name":"@example/empty","exports":{".":"./missing.ts"}}"#,
    )
    .unwrap();
    fs::write(
        directory.path().join("packages/unnamed/package.json"),
        r#"{"version":"1.0.0"}"#,
    )
    .unwrap();
    fs::write(directory.path().join("packages/plain/index.ts"), "").unwrap();
    let importer = directory.path().join("steps.ts");
    fs::write(&importer, "").unwrap();
    let boundary = directory.path().canonicalize().unwrap();
    let mut resolver = ProjectResolution::for_root(directory.path());

    assert!(resolver
        .resolve(&importer, "@example/fallback", &boundary)
        .unwrap()
        .unwrap()
        .ends_with("packages/fallback/index.ts"));
    assert!(resolver
        .resolve(&importer, "@example/entry", &boundary)
        .unwrap()
        .unwrap()
        .ends_with("packages/entry/index.ts"));
    assert!(resolver
        .resolve(&importer, "@example/exports/only", &boundary)
        .unwrap()
        .unwrap()
        .ends_with("packages/exports/only.ts"));
    assert_eq!(
        resolver
            .resolve(&importer, "@example/exports/other", &boundary)
            .unwrap(),
        None
    );
    assert_eq!(
        resolver
            .resolve(&importer, "@example/empty", &boundary)
            .unwrap(),
        None
    );
    assert_eq!(
        resolver
            .workspace_packages
            .as_ref()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        [
            "@example/empty",
            "@example/entry",
            "@example/exports",
            "@example/fallback",
            "root"
        ]
    );
}

/// Pins that two workspace packages claiming the same name abort resolution instead of letting
/// whichever directory the scan reached last win. A silent winner would attribute a step file's
/// registrations to an arbitrary one of two real packages.
#[test]
fn duplicate_workspace_package_names_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    for package in ["first", "second"] {
        fs::create_dir_all(directory.path().join("packages").join(package)).unwrap();
        fs::write(
            directory
                .path()
                .join("packages")
                .join(package)
                .join("package.json"),
            r#"{"name":"@example/bdd"}"#,
        )
        .unwrap();
    }
    fs::write(
        directory.path().join("package.json"),
        r#"{"name":"root","workspaces":["packages/*"]}"#,
    )
    .unwrap();
    let importer = directory.path().join("steps.ts");
    fs::write(&importer, "").unwrap();
    let boundary = directory.path().canonicalize().unwrap();
    let mut resolver = ProjectResolution::for_root(directory.path());

    let error = resolver
        .resolve(&importer, "@example/bdd", &boundary)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("duplicate workspace package name `@example/bdd`"),
        "{error:#}"
    );
}

/// Pins that a project config only claims a specifier it genuinely maps. A config without `paths`
/// and a mapping whose targets do not exist must both fall through to ordinary package resolution
/// — returning a phantom path instead would make the importing file read a module that is not
/// there.
#[test]
fn project_configs_without_matching_path_targets_fall_through() {
    for config in [
        r#"{"compilerOptions":{"strict":true}}"#,
        r#"{"compilerOptions":{"paths":{"@app/*":["nowhere/*"]}}}"#,
    ] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("tsconfig.json"), config).unwrap();
        let importer = directory.path().join("steps.ts");
        fs::write(&importer, "").unwrap();
        let boundary = directory.path().canonicalize().unwrap();
        let mut resolver = ProjectResolution::for_root(directory.path());

        assert_eq!(
            resolver
                .resolve(&importer, "@app/world", &boundary)
                .unwrap(),
            None,
            "{config}"
        );
    }
}

/// Pins how resolution reacts to importers it cannot place. A directory that does not exist is an
/// operational failure and must name the importer, while an importer outside the analysis root, a
/// root holding no manifest, and a scope-only specifier simply have no package to consult and must
/// resolve to nothing rather than reaching for an unrelated package.
#[test]
fn unplaceable_importers_fail_by_name_or_resolve_to_no_package() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut resolver = ProjectResolution::for_root(&root);
    let missing = root.join("gone/steps.ts");

    let error = resolver
        .nearest_project_config(&missing, &root)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("failed to resolve importer directory"),
        "{error:#}"
    );
    let error = resolver.nearest_package(&missing, &root).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("failed to resolve package scope"),
        "{error:#}"
    );

    let importer = root.join("steps.ts");
    fs::write(&importer, "").unwrap();
    assert_eq!(resolver.resolve(&importer, "#bdd", &root).unwrap(), None);
    assert_eq!(resolver.resolve(&importer, "@scope", &root).unwrap(), None);

    let outside = tempfile::tempdir().unwrap();
    let stranger = outside.path().join("steps.ts");
    fs::write(&stranger, "").unwrap();
    assert_eq!(resolver.resolve(&stranger, "#bdd", &root).unwrap(), None);
}

/// Pins that project metadata the analyzer refuses to trust says which file it refused and why: a
/// manifest that is not JSON, and a project config outside the analysis root. Both messages are
/// surfaced verbatim as source-localized diagnostics, so a generic failure would leave a user
/// unable to find the offending file.
#[test]
fn unparseable_manifests_and_uncontained_configs_name_the_offending_path() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    fs::write(root.join("package.json"), r#"{"name": }"#).unwrap();

    let error = ProjectResolution::for_root(&root)
        .load_package(&root, &root)
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("failed to parse static package manifest"),
        "{error:#}"
    );

    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("tsconfig.json"), "{}").unwrap();
    let error = ProjectResolution::for_root(&root)
        .load_project_config(&outside.path().join("tsconfig.json"), &root, 0)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("resolves outside the analysis root"),
        "{error:#}"
    );
    assert!(error.to_string().contains("project config"), "{error:#}");

    let error = config_error(r#"{"compilerOptions":{"baseUrl":"missing/.."}}"#);
    assert!(error.contains("has no existing ancestor"), "{error}");
}

/// Pins two package-map rules that decide which file a subpath reaches: a key must carry the
/// prefix its map kind requires, and when several wildcard keys match the same subpath the most
/// specific key wins. Preferring the looser key would resolve `./steps/world` to the catch-all
/// target and read a different module than the package publishes.
#[test]
fn package_maps_require_key_prefixes_and_prefer_the_most_specific_wildcard() {
    let error = package_map_targets(
        &serde_json::json!({"#internal": "./internal.ts"}),
        "./internal",
        "package exports",
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("wrong subpath prefix"),
        "{error}"
    );

    assert_eq!(
        package_map_targets(
            &serde_json::json!({"./*": "./generic/*.ts", "./steps/*": "./specific/*.ts"}),
            "./steps/world",
            "package exports",
        )
        .unwrap(),
        Some(vec!["./specific/world.ts".to_owned()])
    );
}

/// Pins the containment backstop on package export targets. A package whose root sits outside the
/// analysis root resolves its own export map happily, and only this check stops the resolved file
/// from being handed back — which would pull definition sources from outside the analyzed tree.
#[test]
fn package_export_targets_outside_the_analysis_root_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("index.ts"), "").unwrap();
    let root = directory.path().canonicalize().unwrap();
    let package = PackageInfo {
        root: outside.path().canonicalize().unwrap(),
        name: Some("@example/outside".to_owned()),
        imports: None,
        exports: Some(serde_json::json!({".": "./index.ts"})),
        main: None,
        workspaces: Vec::new(),
    };

    let error = ProjectResolution::for_root(&root)
        .resolve_package_entry(&package, "", &root)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("package export target resolves outside the analysis root"),
        "{error:#}"
    );
}
