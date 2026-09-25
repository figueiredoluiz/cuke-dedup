use super::*;
use std::fs;

#[test]
fn project_resolution_supports_relative_modules_and_rejects_escapes() {
    let directory = tempfile::tempdir().unwrap();
    let boundary = directory.path().canonicalize().unwrap();
    let importer = directory.path().join("nested/steps.ts");
    fs::create_dir(directory.path().join("nested")).unwrap();
    fs::create_dir(directory.path().join("support")).unwrap();
    fs::write(&importer, "").unwrap();
    fs::write(directory.path().join("direct.ts"), "").unwrap();
    fs::write(directory.path().join("support/index.ts"), "").unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("escaped.ts"), "").unwrap();
    let mut project = ProjectResolution::for_root(directory.path());

    let cases = [
        ("extension inference", "../direct", Some("direct.ts")),
        ("explicit extension", "../direct.ts", Some("direct.ts")),
        ("directory index", "../support", Some("support/index.ts")),
        ("bare package", "@example/support", None),
        ("missing relative", "../missing", None),
    ];
    for (name, specifier, expected_suffix) in cases {
        let resolved = project.resolve(&importer, specifier, &boundary).unwrap();
        assert_eq!(
            resolved.as_ref().map(|path| {
                path.strip_prefix(&boundary)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            }),
            expected_suffix.map(str::to_owned),
            "{name}"
        );
    }

    let escape = format!(
        "../../{}/escaped",
        outside.path().file_name().unwrap().to_string_lossy()
    );
    let error = project.resolve(&importer, &escape, &boundary).unwrap_err();
    assert!(error
        .to_string()
        .contains("outside package or analysis root"));
}

#[test]
fn branching_diamond_reexports_are_memoized_by_path_and_remaining_depth() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    let importer = directory.path().join("steps.ts");
    fs::write(&importer, "").unwrap();
    fs::write(
        directory.path().join("root.ts"),
        "export * from './a0';\nexport * from './b0';\n",
    )
    .unwrap();

    let layers = 10;
    for layer in 0..layers {
        let contents = if layer + 1 == layers {
            "export * from '@cucumber/cucumber';\n".to_owned()
        } else {
            format!(
                "export * from './a{}';\nexport * from './b{}';\n",
                layer + 1,
                layer + 1
            )
        };
        fs::write(directory.path().join(format!("a{layer}.ts")), &contents).unwrap();
        fs::write(directory.path().join(format!("b{layer}.ts")), &contents).unwrap();
    }

    let boundary = project_boundary(&importer).unwrap();
    let mut project = ProjectResolution::for_root(directory.path());
    let mut state = ResolutionState::default();
    let resolved =
        resolve_exports(&importer, "./root", &boundary, &mut project, &mut state, 0).unwrap();

    assert_eq!(
        resolved
            .exports
            .unwrap()
            .get("Given")
            .map(|export| export.canonical.as_str()),
        Some("Given")
    );
    assert_eq!(state.modules.len(), 1 + layers * 2);
    assert_eq!(state.resolutions_started, 1 + layers * 2);
}

#[test]
fn resolver_reuses_cached_modules_across_importing_files() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    fs::create_dir(directory.path().join("first")).unwrap();
    fs::create_dir(directory.path().join("second")).unwrap();
    let first = directory.path().join("first/steps.ts");
    let second = directory.path().join("second/steps.ts");
    fs::write(&first, "").unwrap();
    fs::write(&second, "").unwrap();
    fs::write(
        directory.path().join("support.ts"),
        "export { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();

    let mut resolver = RegistrationResolver::for_root(directory.path());
    let _ = resolver.registration_exports(&first, "../support");
    let modules_after_first = resolver.state.modules.len();
    let resolutions_after_first = resolver.state.resolutions_started;
    let _ = resolver.registration_exports(&second, "../support");

    assert_eq!(resolver.state.modules.len(), modules_after_first);
    assert_eq!(resolver.state.resolutions_started, resolutions_after_first);
}

#[test]
fn configured_analysis_root_allows_cross_package_reexports() {
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir_all(directory.path().join("packages/app")).unwrap();
    fs::write(directory.path().join("packages/app/package.json"), "{}").unwrap();
    let importer = directory.path().join("packages/app/steps.ts");
    fs::write(&importer, "").unwrap();
    fs::write(
        directory.path().join("support.ts"),
        "export { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();

    let mut resolver = RegistrationResolver::for_root(directory.path());
    let exports = resolver
        .registration_exports(&importer, "../../support")
        .unwrap()
        .resolution
        .unwrap();

    assert_eq!(
        exports
            .exports
            .get("Given")
            .map(|export| export.canonical.as_str()),
        Some("Given")
    );
    assert_eq!(exports.framework, Framework::CucumberJs);
}

#[test]
fn depth_diverse_graphs_cache_each_module_depth_within_the_state_budget() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    let importer = directory.path().join("steps.ts");
    fs::write(&importer, "").unwrap();
    fs::write(
        directory.path().join("shared.ts"),
        "export { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("root.ts"),
        "export * from './shared';\nexport * from './chain0';\n",
    )
    .unwrap();
    let chain_depth = 8;
    for depth in 0..chain_depth {
        let next = if depth + 1 == chain_depth {
            String::new()
        } else {
            format!("export * from './chain{}';\n", depth + 1)
        };
        fs::write(
            directory.path().join(format!("chain{depth}.ts")),
            format!("export * from './shared';\n{next}"),
        )
        .unwrap();
    }

    let mut resolver = RegistrationResolver::for_root(directory.path());
    let exports = resolver
        .registration_exports(&importer, "./root")
        .unwrap()
        .resolution
        .unwrap();
    let shared = directory.path().join("shared.ts").canonicalize().unwrap();
    let shared_states = resolver
        .state
        .exports
        .keys()
        .filter(|(path, _)| path == &shared)
        .count();

    assert_eq!(
        exports
            .exports
            .get("Given")
            .map(|export| export.canonical.as_str()),
        Some("Given")
    );
    assert_eq!(exports.framework, Framework::CucumberJs);
    assert_eq!(shared_states, chain_depth + 1);
    assert!(resolver.state.resolutions_started < MAX_REGISTRATION_RESOLUTION_STATES);
}

#[test]
fn cycles_do_not_poison_context_dependent_export_results() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    let importer = directory.path().join("steps.ts");
    fs::write(&importer, "").unwrap();
    fs::write(
        directory.path().join("a.ts"),
        "export * from './b';\nexport { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();
    fs::write(directory.path().join("b.ts"), "export * from './a';\n").unwrap();

    let boundary = project_boundary(&importer).unwrap();
    let mut project = ProjectResolution::for_root(directory.path());
    let mut state = ResolutionState::default();
    let from_a = resolve_exports(&importer, "./a", &boundary, &mut project, &mut state, 0).unwrap();
    let from_b = resolve_exports(&importer, "./b", &boundary, &mut project, &mut state, 0).unwrap();

    assert_eq!(
        from_a
            .exports
            .unwrap()
            .get("Given")
            .map(|export| export.canonical.as_str()),
        Some("Given")
    );
    assert_eq!(
        from_b
            .exports
            .unwrap()
            .get("Given")
            .map(|export| export.canonical.as_str()),
        Some("Given")
    );
}

#[test]
fn aggregate_module_graph_limits_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    let importer = directory.path().join("steps.ts");
    let module = directory.path().join("module.ts");
    let module_source = "export { Given } from '@cucumber/cucumber';\n";
    fs::write(&importer, "").unwrap();
    fs::write(&module, module_source).unwrap();

    let mut module_count_state = ResolutionState::default();
    for index in 0..MAX_REGISTRATION_MODULES {
        module_count_state
            .modules
            .insert(PathBuf::from(format!("cached-{index}")), None);
    }
    let error = load_module(&module, &mut module_count_state).unwrap_err();
    assert!(error.to_string().contains("module resolution limit"));

    let mut byte_state = ResolutionState {
        module_bytes: MAX_REGISTRATION_MODULE_BYTES,
        ..ResolutionState::default()
    };
    let error = load_module(&module, &mut byte_state).unwrap_err();
    assert!(error.to_string().contains("aggregate resolution limit"));

    let mut exact_byte_state = ResolutionState {
        module_bytes: MAX_REGISTRATION_MODULE_BYTES - module_source.len(),
        ..ResolutionState::default()
    };
    assert!(load_module(&module, &mut exact_byte_state)
        .unwrap()
        .is_some());
    assert_eq!(exact_byte_state.module_bytes, MAX_REGISTRATION_MODULE_BYTES);

    let boundary = project_boundary(&importer).unwrap();
    let mut project = ProjectResolution::for_root(directory.path());
    let mut resolution_state = ResolutionState {
        resolutions_started: MAX_REGISTRATION_RESOLUTION_STATES,
        ..ResolutionState::default()
    };
    let error = resolve_exports(
        &importer,
        "./module",
        &boundary,
        &mut project,
        &mut resolution_state,
        0,
    )
    .unwrap_err();
    assert!(error.to_string().contains("state resolution limit"));
}

#[test]
fn malformed_reexport_modules_cannot_supply_recovered_registrations() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    let importer = directory.path().join("steps.ts");
    fs::write(&importer, "").unwrap();
    fs::write(
        directory.path().join("broken.ts"),
        "export { Given } from '@cucumber/cucumber';\nconst broken = ;\n",
    )
    .unwrap();

    let mut resolver = RegistrationResolver::for_root(directory.path());
    let outcome = resolver
        .registration_exports(&importer, "./broken")
        .expect("a malformed barrel is not an operational failure");

    // A malformed barrel yields no registrations and an explainable reason, never an error
    // that would abort analysis of the importing file.
    assert!(outcome.resolution.is_none());
    let reason = outcome.reason.expect("static resolution reason");
    assert!(reason.contains("syntax errors"), "{reason}");
    assert!(reason.contains("refusing recovered exports"), "{reason}");
}

#[test]
fn namespace_star_exports_are_not_flattened_into_direct_registrations() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    let importer = directory.path().join("steps.ts");
    fs::write(&importer, "").unwrap();
    fs::write(
        directory.path().join("support.ts"),
        "export { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("plain.ts"),
        "export /* comment */ * from './support';\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("namespace.ts"),
        "export /* comment */ * as cucumber from './support';\n",
    )
    .unwrap();
    fs::write(
        directory.path().join("types.ts"),
        "export { type Config } from 'playwright-bdd';\n",
    )
    .unwrap();

    let mut resolver = RegistrationResolver::for_root(directory.path());
    let plain = resolver
        .registration_exports(&importer, "./plain")
        .unwrap()
        .resolution
        .unwrap();
    assert_eq!(
        plain
            .exports
            .get("Given")
            .map(|export| export.canonical.as_str()),
        Some("Given")
    );

    let namespace = resolver
        .registration_exports(&importer, "./namespace")
        .unwrap()
        .resolution
        .unwrap();
    assert!(namespace.exports.is_empty());
    assert_eq!(namespace.framework, Framework::Unknown);

    let types = resolver
        .registration_exports(&importer, "./types")
        .unwrap()
        .resolution
        .unwrap();
    assert!(types.exports.is_empty());
    assert_eq!(types.framework, Framework::Unknown);
}

/// Pins that a module whose path carries no supported grammar — a `.d.ts` declaration file is the
/// realistic case — contributes no registration exports instead of being parsed with some default
/// grammar. Declaration files describe types only, so treating their `export` lines as runtime
/// re-exports would invent registrations the compiled program never has.
#[test]
fn modules_without_a_supported_grammar_contribute_no_exports() {
    let directory = tempfile::tempdir().unwrap();
    let declarations = directory.path().join("support.d.ts");
    fs::write(
        &declarations,
        "export { Given } from '@cucumber/cucumber';\n",
    )
    .unwrap();
    let boundary = directory.path().canonicalize().unwrap();
    let mut project = ProjectResolution::for_root(directory.path());
    let mut state = ResolutionState::default();

    let resolved =
        resolve_active_exports(&declarations, &boundary, &mut project, &mut state, 0).unwrap();

    assert!(resolved.exports.is_none());
    assert_eq!(resolved.framework, Framework::Unknown);
    assert!(resolved.cacheable);
    // The refusal is remembered, so a second importer does not re-read the same file.
    assert_eq!(
        state.modules.get(&declarations).map(Option::is_none),
        Some(true)
    );
}

/// Pins the boundary chosen for an importer that no package manifest encloses: its own directory.
/// The boundary is what every relative specifier is checked against, so falling back to something
/// wider would let a loose step file reach modules outside the tree it lives in.
#[test]
fn project_boundary_falls_back_to_the_importer_directory_without_a_manifest() {
    let directory = tempfile::tempdir().unwrap();
    let canonical = directory.path().canonicalize().unwrap();
    assert!(
        !canonical
            .ancestors()
            .any(|ancestor| ancestor.join("package.json").is_file()),
        "the temporary directory unexpectedly sits inside a package"
    );

    assert_eq!(
        project_boundary(&canonical.join("steps.ts")),
        Some(canonical)
    );
}

/// Pins which local bindings of a re-exporting barrel are recognized as registrations, across the
/// ESM, `import =`, CommonJS and `createBdd` binding forms it may mix. Only the four bindings that
/// provably name a framework registration may be re-exported as one: everything else here is a
/// destructure the analyzer cannot follow — a nested pattern, a rest element, a dynamic `require`
/// argument, a plain value, an unknown name, or a declaration with no initializer at all — and
/// attributing a registration to any of them would invent step definitions in every file that
/// imports this barrel.
#[test]
fn barrel_bindings_only_re_export_provable_registrations() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    let importer = directory.path().join("steps.ts");
    fs::write(&importer, "").unwrap();
    fs::write(
        directory.path().join("barrel.ts"),
        r#"import { createBdd } from 'playwright-bdd';
import check = require('expect');
import { Given as GivenEsm, World } from '@cucumber/cucumber';
const { Given: CjsGiven, World: CjsWorld, ...rest } = require('@cucumber/cucumber');
const { Then: { deepCjs } } = require('@cucumber/cucumber');
const { When: CjsWhen } = (require)('@cucumber/cucumber');
const { fromDynamic } = require(dynamicName);
const { fromPlainValue } = notACall;
let placeholder;
const { Given: BddGiven, Then: { deepBdd }, missing, ...others } = createBdd();
function nestedScope() {
  const { innerBinding } = someObject;
  return innerBinding;
}
export { GivenEsm, World, CjsGiven, CjsWorld, rest, deepCjs, CjsWhen, fromDynamic,
  fromPlainValue, placeholder, BddGiven, deepBdd, missing, others, nestedScope, check };
"#,
    )
    .unwrap();

    let mut resolver = RegistrationResolver::for_root(directory.path());
    let resolution = resolver
        .registration_exports(&importer, "./barrel")
        .unwrap()
        .resolution
        .unwrap();

    assert_eq!(
        resolution
            .exports
            .iter()
            .map(|(name, export)| (name.as_str(), export.canonical.as_str()))
            .collect::<Vec<_>>(),
        [
            ("BddGiven", "Given"),
            ("CjsGiven", "Given"),
            ("CjsWhen", "When"),
            ("GivenEsm", "Given"),
        ]
    );
    assert_eq!(resolution.framework, Framework::PlaywrightBdd);
}

/// Resolve `specifier` from a temp root holding the given `(filename, contents)` files.
fn resolve_barrel(files: &[(&str, &str)], specifier: &str) -> RegistrationOutcome {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("package.json"), "{}").unwrap();
    let importer = directory.path().join("steps.js");
    fs::write(&importer, "").unwrap();
    for (name, contents) in files {
        fs::write(directory.path().join(name), contents).unwrap();
    }
    let mut resolver = RegistrationResolver::for_root(directory.path());
    resolver.registration_exports(&importer, specifier).unwrap()
}

fn exported_names(outcome: &RegistrationOutcome) -> Vec<(String, String)> {
    outcome
        .resolution
        .as_ref()
        .unwrap()
        .exports
        .iter()
        .map(|(name, export)| (name.clone(), export.canonical.clone()))
        .collect()
}

/// `module.exports = { X }`, `{ X: local }`, and `exports.X = local` each re-export a registration.
#[test]
fn commonjs_module_exports_barrels_resolve_every_modeled_form() {
    let cucumber = "const { Given } = require('@cucumber/cucumber');\n";
    let cases: &[(&str, &str)] = &[
        ("object", "module.exports = { Given };"),
        ("pair", "module.exports = { Given: Given };"),
        ("member", "exports.Given = Given;"),
    ];
    for (label, exports) in cases {
        let outcome = resolve_barrel(
            &[("barrel.js", &format!("{cucumber}{exports}"))],
            "./barrel",
        );
        assert_eq!(
            exported_names(&outcome),
            [("Given".to_owned(), "Given".to_owned())],
            "{label}"
        );
        assert_eq!(outcome.reason, None, "{label}");
    }
}

/// `module.exports = require('./inner')`, its spread, and `Object.assign` star re-export a local
/// CJS barrel, which also pins that CJS re-exports chain through the resolver's recursion.
#[test]
fn commonjs_star_re_export_forms_follow_a_local_barrel() {
    let inner = (
        "inner.js",
        "const { When } = require('@cucumber/cucumber');\nmodule.exports = { When };",
    );
    let cases: &[(&str, &str)] = &[
        ("require", "module.exports = require('./inner');"),
        ("spread", "module.exports = { ...require('./inner') };"),
        (
            "object-assign",
            "Object.assign(module.exports, require('./inner'));",
        ),
    ];
    for (label, barrel) in cases {
        let outcome = resolve_barrel(&[("barrel.js", barrel), inner], "./barrel");
        assert_eq!(
            exported_names(&outcome),
            [("When".to_owned(), "When".to_owned())],
            "{label}"
        );
        assert_eq!(outcome.reason, None, "{label}");
    }
}

/// A spread re-export merged with a named export keeps both.
#[test]
fn commonjs_spread_barrel_merges_a_re_export_with_a_named_export() {
    let inner = (
        "inner.js",
        "const { When } = require('@cucumber/cucumber');\nmodule.exports = { When };",
    );
    let barrel = (
        "barrel.js",
        "const { Given } = require('@cucumber/cucumber');\nmodule.exports = { ...require('./inner'), Given };",
    );
    let outcome = resolve_barrel(&[barrel, inner], "./barrel");
    assert_eq!(
        exported_names(&outcome),
        [
            ("Given".to_owned(), "Given".to_owned()),
            ("When".to_owned(), "When".to_owned())
        ],
    );
    assert_eq!(outcome.reason, None);
}

/// An unmodeled `module.exports` form fails closed with a reason; a genuinely empty module stays
/// quiet — the opposite-answer control proving the reason means "unmodeled", not "no exports".
#[test]
fn an_unmodeled_commonjs_export_form_marks_the_module_incomplete() {
    let unmodeled = resolve_barrel(
        &[("barrel.js", "module.exports = makeSteps();")],
        "./barrel",
    );
    assert!(exported_names(&unmodeled).is_empty());
    assert!(
        unmodeled.reason.is_some(),
        "unmodeled form must record a reason"
    );

    let empty = resolve_barrel(&[("barrel.js", "const unrelated = 1;")], "./barrel");
    assert!(exported_names(&empty).is_empty());
    assert_eq!(
        empty.reason, None,
        "a genuinely empty module must stay quiet"
    );
}

/// `exports.x = require('./inner').x` re-exports one member; `exports.x = require('./inner')`
/// assigns the whole module object to a name the analyzer cannot introspect, so it fails closed.
#[test]
fn commonjs_named_exports_resolve_member_re_exports_and_flag_whole_module_assignment() {
    let inner = (
        "inner.js",
        "const { Then } = require('@cucumber/cucumber');\nmodule.exports = { Then };",
    );
    let member = resolve_barrel(
        &[
            ("barrel.js", "exports.Then = require('./inner').Then;"),
            inner,
        ],
        "./barrel",
    );
    assert_eq!(
        exported_names(&member),
        [("Then".to_owned(), "Then".to_owned())]
    );
    assert_eq!(member.reason, None);

    let whole = resolve_barrel(
        &[("barrel.js", "exports.api = require('./inner');"), inner],
        "./barrel",
    );
    assert!(exported_names(&whole).is_empty());
    assert!(whole.reason.is_some());
}

/// Every `module.exports` shape the analyzer cannot introspect marks the module incomplete, while
/// an assignment that is not an export and an unrelated call export nothing and stay quiet.
#[test]
fn commonjs_unmodeled_export_shapes_flag_incomplete_and_non_exports_stay_quiet() {
    for barrel in [
        "module.exports = { Given: makeGiven() };",
        "module.exports = { [key]: Given };",
        "module.exports = { register(text, fn) { Given(text, fn); } };",
        "module.exports = { ...other };",
        "Object.assign(module.exports, other);",
    ] {
        let outcome = resolve_barrel(&[("barrel.js", barrel)], "./barrel");
        assert!(exported_names(&outcome).is_empty(), "{barrel}");
        assert!(outcome.reason.is_some(), "{barrel}");
    }

    for barrel in [
        "other.field = Given;",
        "doSomething();",
        "Object.assign(target, source);",
    ] {
        let outcome = resolve_barrel(&[("barrel.js", barrel)], "./barrel");
        assert!(exported_names(&outcome).is_empty(), "{barrel}");
        assert_eq!(outcome.reason, None, "{barrel}");
    }
}

/// A string-literal object key is a valid export name, resolved exactly as a bare identifier key.
#[test]
fn commonjs_object_export_resolves_a_string_literal_key() {
    let barrel = concat!(
        "const { Given } = require('@cucumber/cucumber');\n",
        "module.exports = { \"Given\": Given };"
    );
    let outcome = resolve_barrel(&[("barrel.js", barrel)], "./barrel");
    assert_eq!(
        exported_names(&outcome),
        [("Given".to_owned(), "Given".to_owned())]
    );
    assert_eq!(outcome.reason, None);
}

/// Degenerate and malformed export shapes never panic: an empty `Object.assign`, a nested-member
/// assignment target, and syntactically broken export forms all export nothing.
#[test]
fn commonjs_degenerate_and_malformed_export_shapes_are_handled() {
    for barrel in [
        "Object.assign();",
        "a.b.c = Given;",
        "module.exports = ;",
        "module.exports = { Given: };",
    ] {
        let outcome = resolve_barrel(&[("barrel.js", barrel)], "./barrel");
        let surfaced = outcome
            .resolution
            .as_ref()
            .is_some_and(|resolution| !resolution.exports.is_empty());
        assert!(!surfaced, "{barrel}");
    }
}

/// A star re-export of a module that does not resolve contributes nothing and does not panic.
#[test]
fn commonjs_star_re_export_of_a_missing_module_resolves_to_nothing() {
    let outcome = resolve_barrel(
        &[("barrel.js", "module.exports = require('./missing');")],
        "./barrel",
    );
    let surfaced = outcome
        .resolution
        .as_ref()
        .is_some_and(|resolution| !resolution.exports.is_empty());
    assert!(!surfaced);
}

/// `module.exports.Given = Given` names a single export off the module object and resolves.
#[test]
fn commonjs_nested_module_exports_member_resolves_a_named_export() {
    let barrel = concat!(
        "const { Given } = require('@cucumber/cucumber');\n",
        "module.exports.Given = Given;"
    );
    let outcome = resolve_barrel(&[("barrel.js", barrel)], "./barrel");
    assert_eq!(
        exported_names(&outcome),
        [("Given".to_owned(), "Given".to_owned())]
    );
    assert_eq!(outcome.reason, None);
}

/// A named export whose value is a call or namespace member can hide a registration, so it fails
/// closed; an inert literal value exports nothing and stays quiet.
#[test]
fn commonjs_opaque_named_export_values_fail_closed_but_inert_values_stay_quiet() {
    for barrel in [
        "exports.Given = makeGiven();",
        "exports.Given = cucumber.Given;",
    ] {
        let outcome = resolve_barrel(&[("barrel.js", barrel)], "./barrel");
        assert!(exported_names(&outcome).is_empty(), "{barrel}");
        assert!(outcome.reason.is_some(), "{barrel}");
    }

    let inert = resolve_barrel(&[("barrel.js", "exports.version = 5;")], "./barrel");
    assert!(exported_names(&inert).is_empty());
    assert_eq!(inert.reason, None);
}

/// A `module.exports` assignment or `Object.assign` nested in a function that may never run is not
/// a module export: only a statement of the program body is.
#[test]
fn commonjs_exports_inside_a_function_are_not_module_exports() {
    for barrel in [
        "const { Given } = require('@cucumber/cucumber');\nfunction setup() { module.exports = { Given }; }",
        "const { Given } = require('@cucumber/cucumber');\nfunction setup() { Object.assign(module.exports, { Given }); }",
        "const { Given } = require('@cucumber/cucumber');\nclass C { m() { module.exports = { Given }; } }",
    ] {
        let outcome = resolve_barrel(&[("barrel.js", barrel)], "./barrel");
        assert!(exported_names(&outcome).is_empty(), "{barrel}");
        assert_eq!(outcome.reason, None, "{barrel}");
    }
}

/// A `module.exports`/`Object.assign` export under module-level control flow cannot be resolved,
/// but it touches the exports object, so it fails closed rather than passing as clean. A top-level
/// conditional that assigns something else, and an assignment inside a function, stay quiet.
#[test]
fn commonjs_conditional_module_level_exports_fail_closed() {
    let prelude = "const { Given } = require('@cucumber/cucumber');\n";
    for barrel in [
        "if (enabled) { module.exports = { Given }; }",
        "for (;;) { Object.assign(module.exports, { Given }); }",
        "try { module.exports = { Given }; } catch (e) {}",
    ] {
        let outcome = resolve_barrel(&[("barrel.js", &format!("{prelude}{barrel}"))], "./barrel");
        assert!(exported_names(&outcome).is_empty(), "{barrel}");
        assert!(outcome.reason.is_some(), "{barrel}");
    }

    for barrel in [
        "if (x) { other = 1; }",
        "function setup() { module.exports = { Given }; }",
    ] {
        let outcome = resolve_barrel(&[("barrel.js", &format!("{prelude}{barrel}"))], "./barrel");
        assert!(exported_names(&outcome).is_empty(), "{barrel}");
        assert_eq!(outcome.reason, None, "{barrel}");
    }
}

/// A bracket-notation export target on `exports`/`module.exports` — static or dynamic key — names
/// something the analyzer does not model, so it fails closed instead of being dropped silently.
#[test]
fn commonjs_bracket_notation_exports_fail_closed() {
    let prelude = "const { Given } = require('@cucumber/cucumber');\n";
    for barrel in [
        "exports['Given'] = Given;",
        "module.exports['Given'] = Given;",
        "exports[name] = Given;",
    ] {
        let outcome = resolve_barrel(&[("barrel.js", &format!("{prelude}{barrel}"))], "./barrel");
        assert!(exported_names(&outcome).is_empty(), "{barrel}");
        assert!(outcome.reason.is_some(), "{barrel}");
    }
}

/// Which `module.exports`/`exports.x` values must fail closed. A value is quiet only when it provably
/// cannot carry a registration; each quiet row has an incomplete control that differs in the one
/// property that decides it. The observable is the incompleteness reason: `None` means the module
/// is treated as fully modeled, so a wrong "quiet" would hide a registration without a warning.
#[test]
fn commonjs_export_values_are_quiet_only_when_provably_inert() {
    let quiet = [
        // Value kind: a function, arrow, class, or literal that never names a registration.
        "module.exports = async function act(value) { return value; };",
        "module.exports = (a) => a + 1;",
        // A bracket key is evidence only when it can name a registration.
        "module.exports = (list, index) => list[index] + list['size'];",
        // Calling through a string key that is not a registration is not evidence either.
        "module.exports = (api, t) => api['run'](t);",
        // Calling a function received as a parameter stays inert here: tainting every callback
        // would flag ordinary helpers such as `items.map(fn)`. A registration passed in
        // (`helper(Given, …)`) is reported at the call site instead; see
        // `passing_a_registration_to_a_function_is_reported`.
        "module.exports = (register, t, f) => register(t, f);",
        "module.exports = class Page { open() { return 1; } };",
        "module.exports = 'text';",
        "module.exports = [{ matcher: 'a', code: 'b' }, 2];",
        "module.exports = { name: 'x', run() { return 1; }, nested: { list: [1] } };",
        // Builtin modules cannot register, so referring to one keeps a function inert.
        "const path = require('path');\nmodule.exports = function base(f) { return path.basename(f); };",
        "const fs = require('node:fs');\nmodule.exports = () => fs.existsSync('x');",
        // A comment inside an object literal is not a property.
        "module.exports = {\n  // helpers\n  run() { return 1; },\n};",
        // Identifier export: resolved through a single top-level declaration.
        "function helper() { return 1; }\nmodule.exports = helper;",
        // Calling or constructing a declared name cannot change it.
        "function helper() { return 1; }\nhelper();\nmodule.exports = helper;",
        "class Store { get() { return 1; } }\nnew Store();\nmodule.exports = Store;",
        "class Store { get() { return 1; } }\nmodule.exports = Store;",
        "const settings = { retries: 2 };\nmodule.exports = settings;",
        "const settings = ({ retries: 2 });\nmodule.exports = (settings);",
        // Reading a property into another name is a read, not a write into the object.
        "const settings = { retries: 2 };\nlet copy;\ncopy = settings.retries;\nmodule.exports = settings;",
        // Reading a property, or testing it, keeps the value.
        "const api = { nested: {} };\nif (api.nested && typeof api === 'object') { log(api.nested); }\nmodule.exports = api;",
        // A registration name the module declares itself is that local value, not the global.
        "const Given = {};\nGiven.gateway = function gateway() { return 1; };\nmodule.exports = Given;",
        // ...so a function that refers to it stays inert. Here the taint check alone decides.
        "const Given = { base: 1 };\nmodule.exports = function run() { return Given.base; };",
        // Named exports and `Object.assign` arguments follow the same rule.
        "exports.run = function run() { return 1; };",
        "exports.config = { retries: 2 };",
        "Object.assign(module.exports, { run() { return 1; } });",
    ];
    for barrel in quiet {
        let outcome = resolve_barrel(&[("barrel.js", barrel)], "./barrel");
        assert!(exported_names(&outcome).is_empty(), "{barrel}");
        assert_eq!(outcome.reason, None, "must be quiet: {barrel}");
    }

    let incomplete = [
        // Taint through the body: a registration global, a registration member, a local
        // registration binding, a non-builtin or dynamic module, or a tainted top-level name.
        "module.exports = function steps() { Given('a', () => {}); };",
        "module.exports = (bdd) => bdd.Given('a', () => {});",
        "module.exports = (bdd) => bdd['Given']('a', () => {});",
        // A legacy octal escape (`\107` is `G`) is not decoded, so the key must still count.
        "module.exports = (bdd) => bdd['\\107iven']('a', () => {});",
        "module.exports = (bdd) => bdd[`Given`]('a', () => {});",
        // A member chosen at runtime and called may be a registration.
        "module.exports = (bdd, name, t, f) => bdd[name](t, f);",
        "module.exports = (bdd, name, t, f) => (bdd[name])(t, f);",
        "const { Given } = require('@cucumber/cucumber');\nmodule.exports = () => Given('a', () => {});",
        "const lib = require('./lib');\nmodule.exports = function run() { return lib.x; };",
        "module.exports = function load(name) { return require(name); };",
        "module.exports = () => import('./steps');",
        "const register = (text, fn) => Given(text, fn);\nmodule.exports = () => register('a', () => {});",
        "module.exports = class Steps { static Given = Given; };",
        // A renamed import binds only its alias, so the global `Given` stays a registration. A
        // builtin source is what makes this discriminating: a non-builtin import taints every name
        // it binds either way.
        "import { Given as G } from 'util';\nmodule.exports = () => Given('a', () => {});",
        "module.exports = { step: (text, fn) => Given(text, fn) };",
        // A later assignment carries taint as a declaration does, wherever it runs and whatever
        // it assigns to.
        "let register;\nregister = Given;\nmodule.exports = { step: (t, f) => register(t, f) };",
        "let register;\nfunction init() { register = Given; }\nmodule.exports = (t, f) => register(t, f);",
        "register = Given;\nmodule.exports = (t, f) => register(t, f);",
        "let register;\n({ Given: register } = require('@cucumber/cucumber'));\nmodule.exports = (t, f) => register(t, f);",
        "let register = noop;\nregister ||= Given;\nmodule.exports = (t, f) => register(t, f);",
        "const cache = {};\ncache.lib = require('./steps');\nmodule.exports = (t, f) => cache.lib.register(t, f);",
        // Unmodeled value kinds.
        "module.exports = new Logger();",
        "module.exports = makeSteps();",
        // A comment inside parentheses is not the value; the call is.
        "module.exports = (/* wrapped */ makeSteps());",
        // A default value is not a binding: `Given` stays the global, so the wrapper is tainted.
        "const { register = Given } = {};\nmodule.exports = (t, f) => Given(t, f);",
        "module.exports = flag ? a : b;",
        "module.exports = { nested: { Given } };",
        // Identifier export whose binding is not a single static value.
        "let settings = { retries: 2 };\nmodule.exports = settings;",
        "var settings = { retries: 2 };\nmodule.exports = settings;",
        "const settings = { a: 1 };\nconst settings = { b: 2 };\nmodule.exports = settings;",
        "const { settings } = require('./config');\nmodule.exports = settings;",
        "module.exports = undeclared;",
        "const settings = makeSettings();\nmodule.exports = settings;",
        "function helper() { return Given; }\nmodule.exports = helper;",
        // Mutation after declaration.
        "const api = {};\napi.Given = Given;\nmodule.exports = api;",
        "const api = {};\napi['Given'] = Given;\nmodule.exports = api;",
        // A write through a member chain changes the root: an importer can call `api.nested.step`.
        "const api = { nested: {} };\napi.nested.step = Given;\nmodule.exports = api;",
        // Parentheses around the member change nothing: these are still a write and a method call.
        "const api = {};\n(api.Given) = Given;\nmodule.exports = api;",
        "const api = {};\n(api.register)(Given);\nmodule.exports = api;",
        "const api = {};\nattach(api);\nmodule.exports = api;",
        "const api = {};\napi.register(Given);\nmodule.exports = api;",
        "function helper() {}\nhelper = other;\nmodule.exports = helper;",
        // Escape: once the value reaches another binding or a call, it can be written through
        // from there, however it got there.
        "const api = {};\nattach({ api });\nmodule.exports = api;",
        "const api = {};\nattach([api]);\nmodule.exports = api;",
        "const api = {};\nregistry.api = api;\nmodule.exports = api;",
        "const api = {};\nfunction expose() { return api; }\nmodule.exports = api;",
        "const settings = { retries: 2 };\nconst alias = settings;\nmodule.exports = alias;",
        // Named exports and `Object.assign` arguments that are not inert.
        "exports.api = { Given };",
        "exports.logger = new Logger();",
        "Object.assign(module.exports, { step: (t, f) => Given(t, f) });",
    ];
    for barrel in incomplete {
        let outcome = resolve_barrel(&[("barrel.js", barrel)], "./barrel");
        assert!(outcome.reason.is_some(), "must fail closed: {barrel}");
    }
}

/// An identifier export follows a `const` to its value: an object literal exports its registrations
/// and a `require` re-exports the module, exactly as the direct forms do.
#[test]
fn commonjs_identifier_exports_follow_a_const_to_its_value() {
    let registration = "const { Given } = require('@cucumber/cucumber');\n";
    let object = resolve_barrel(
        &[(
            "barrel.js",
            &format!("{registration}const api = {{ Given }};\nmodule.exports = api;"),
        )],
        "./barrel",
    );
    assert_eq!(
        exported_names(&object),
        [("Given".to_owned(), "Given".to_owned())]
    );
    assert_eq!(object.reason, None);

    let module = resolve_barrel(
        &[
            (
                "barrel.js",
                "const inner = require('./inner');\nmodule.exports = inner;",
            ),
            (
                "inner.js",
                &format!("{registration}module.exports = {{ Given }};"),
            ),
        ],
        "./barrel",
    );
    assert_eq!(
        exported_names(&module),
        [("Given".to_owned(), "Given".to_owned())]
    );
    assert_eq!(module.reason, None);

    // A comment inside an exporting object is not a property. The object is not inert (it exports
    // a registration), so this reaches the property loop rather than being classified whole.
    let commented = resolve_barrel(
        &[(
            "barrel.js",
            &format!("{registration}module.exports = {{\n  // steps\n  Given,\n}};"),
        )],
        "./barrel",
    );
    assert_eq!(
        exported_names(&commented),
        [("Given".to_owned(), "Given".to_owned())]
    );
    assert_eq!(commented.reason, None);
}
