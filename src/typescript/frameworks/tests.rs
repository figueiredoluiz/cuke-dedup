use super::*;

/// Verify registered entrypoints expose the expected call, decorator, and factory names with attribution.
#[test]
fn framework_entrypoints_preserve_export_kinds() {
    for module in FRAMEWORK_REGISTRY
        .iter()
        .flat_map(|entry| entry.modules.iter().copied())
    {
        let exports = registration_exports_for_module(module);
        if matches!(module, "jest-cucumber" | "vitest-cucumber-plugin") {
            continue; // Their callback models are pinned below.
        }
        let decorators = module == PLAYWRIGHT_DECORATORS_MODULE;
        let expected_kind = if decorators {
            RegistrationExportKind::Decorator
        } else {
            RegistrationExportKind::Call
        };
        let expected_names: &[&str] = if decorators {
            &["Given", "When", "Then", "Step"]
        } else {
            &[
                "Given",
                "When",
                "Then",
                "defineStep",
                "given",
                "when",
                "then",
            ]
        };
        assert_eq!(
            exports.len(),
            expected_names.len() + usize::from(module == PLAYWRIGHT_MODULE)
        );
        for name in expected_names {
            assert_eq!(exports[*name].kind, expected_kind, "{module}: {name}");
            assert_eq!(exports[*name].canonical, *name);
            assert_eq!(exports[*name].framework, framework_for_module(module));
        }
        if module == PLAYWRIGHT_MODULE {
            assert_eq!(exports["createBdd"].kind, RegistrationExportKind::Factory);
            assert_eq!(exports["createBdd"].canonical, "createBdd");
            assert_eq!(exports["createBdd"].framework, PLAYWRIGHT_BDD);
        }
    }
}

/// Verify framework entrypoints are unique and reject unsupported paths and similar prefixes.
#[test]
fn framework_entrypoints_are_unique_and_have_exact_attribution() {
    let expected = [
        ("@cucumber/cucumber", CUCUMBER_JS),
        ("cucumber", CUCUMBER_JS),
        ("jest-cucumber", JEST_CUCUMBER),
        ("vitest-cucumber-plugin", VITEST_CUCUMBER),
        ("playwright-bdd", PLAYWRIGHT_BDD),
        ("playwright-bdd/decorators", PLAYWRIGHT_BDD),
        ("@badeball/cypress-cucumber-preprocessor", CYPRESS_CUCUMBER),
        ("cypress-cucumber-preprocessor/steps", CYPRESS_CUCUMBER),
    ];
    let mut modules = std::collections::BTreeSet::new();
    for module in FRAMEWORK_REGISTRY
        .iter()
        .flat_map(|entry| entry.modules.iter().copied())
    {
        assert!(
            modules.insert(module),
            "duplicate framework entrypoint: {module}"
        );
    }
    assert_eq!(modules.len(), expected.len());
    for (module, framework) in expected {
        assert!(is_supported_module(module), "{module}");
        assert_eq!(framework_for_module(module), framework, "{module}");
    }
    for unsupported in [
        "cypress-cucumber-preprocessor",
        "cypress-cucumber-preprocessor/steps-extra",
        "playwright-bdd/decorators-extra",
        "jest-cucumber-extra",
        "vitest-cucumber-plugin-extra",
    ] {
        assert!(!is_supported_module(unsupported), "{unsupported}");
        assert_eq!(
            framework_for_module(unsupported),
            Framework::UNKNOWN,
            "{unsupported}"
        );
    }
}

/// Verify ambient registration defaults exclude lowercase aliases and decorator-only `Step`.
#[test]
fn default_registrations_are_a_subset_of_every_known_registration() {
    for name in DEFAULT_REGISTRATIONS {
        assert!(REGISTRATIONS.contains(&name), "{name}");
    }
    // Lowercase aliases resolve through imports but are never assumed to be globals.
    for name in ["given", "when", "then"] {
        assert!(REGISTRATIONS.contains(&name));
        assert!(!DEFAULT_REGISTRATIONS.contains(&name));
    }
    assert_eq!(
        registration_exports_for_framework(Framework::UNKNOWN).len() + 1,
        REGISTRATIONS.len()
    );
    assert!(!registration_exports_for_framework(Framework::UNKNOWN).contains_key("Step"));
    assert!(registration_exports_for_module(PLAYWRIGHT_DECORATORS_MODULE).contains_key("Step"));
}

/// Only Jest-Cucumber and vitest-cucumber-plugin declare a callback model, each with its own
/// scoping, CommonJS and matcher facts; every other id takes the ordinary registration path.
#[test]
fn callback_models_are_declared_only_by_callback_frameworks() {
    let facts = |framework| {
        callback_model(framework)
            .map(|model| (model.scenario_scoped, model.commonjs, model.literal_strings))
    };
    assert_eq!(facts(JEST_CUCUMBER), Some((true, true, true)));
    assert_eq!(facts(VITEST_CUCUMBER), Some((false, false, false)));
    for framework in [
        CUCUMBER_JS,
        PLAYWRIGHT_BDD,
        CYPRESS_CUCUMBER,
        Framework::UNKNOWN,
        crate::ruby::CUCUMBER_RUBY,
        Framework::new("undeclared"),
    ] {
        assert_eq!(facts(framework), None, "{framework:?}");
    }
    // Steps that register only inside their scenario are compared only within it.
    for framework in [JEST_CUCUMBER, VITEST_CUCUMBER] {
        let scoped = callback_model(framework).is_some_and(|model| model.scenario_scoped);
        assert_eq!(
            super::super::semantics::policy(framework) == SCENARIO_LOCAL,
            scoped,
            "{framework:?}"
        );
    }
    // A framework with several entrypoints uses callbacks on all of them or on none.
    for entry in FRAMEWORK_REGISTRY {
        assert_eq!(
            matches!(entry.exports, ExportStyle::Callbacks(_)),
            callback_model(entry.framework).is_some(),
            "{:?}",
            entry.modules
        );
    }
}

/// Callback entrypoints export exactly their declared names, with their kinds and framework.
#[test]
fn callback_entrypoints_export_their_declared_kinds() {
    let jest = [
        ("defineFeature", Feature),
        ("autoBindSteps", Unsupported),
        ("loadFeature", NonRegistration),
        ("loadFeatures", NonRegistration),
        ("parseFeature", NonRegistration),
        ("setJestCucumberConfiguration", NonRegistration),
        ("generateCodeFromFeature", NonRegistration),
        (
            "generateCodeWithSeparateFunctionsFromFeature",
            NonRegistration,
        ),
    ];
    let vitest = [("Given", Call), ("When", Call), ("Then", Call)];
    for (module, framework, expected) in [
        ("jest-cucumber", JEST_CUCUMBER, &jest[..]),
        ("vitest-cucumber-plugin", VITEST_CUCUMBER, &vitest[..]),
    ] {
        let exports = registration_exports_for_module(module);
        let expected: RegistrationExports = expected
            .iter()
            .map(|&(name, kind)| (name.to_owned(), export(name, kind, framework)))
            .collect();
        assert_eq!(exports, expected, "{module}");
    }
}
