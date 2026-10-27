//! Framework registration vocabulary for the JavaScript/TypeScript adapter.
//!
//! FRAMEWORK_REGISTRY supports entrypoints sharing the existing call exports (optionally with
//! a createBdd-style factory), the fixed Given/When/Then/Step decorator exports, or a
//! `CallbackModel` whose steps are proven through callback provenance. Different export sets or
//! registration syntax require extraction support and fixtures, not just a row.
//! Framework configuration discovery and user-facing compatibility docs are maintained separately.
//! Package recognition alone must not invent bindings or confer assertion-library trust.

use super::framework_config::{
    CUCUMBER_JS_DISCOVERY, CYPRESS_CUCUMBER_DISCOVERY, PLAYWRIGHT_BDD_DISCOVERY,
};
use super::semantics::{ISOLATED, SCENARIO_LOCAL};
use crate::model::Framework;
use crate::source_adapter::FrameworkContract;
use std::collections::BTreeMap;
use RegistrationExportKind::{Call, Feature, NonRegistration, Unsupported};

/// Cucumber.js registration APIs.
pub(crate) const CUCUMBER_JS: Framework = Framework::new("cucumber-js");
/// Playwright BDD registration APIs.
pub(crate) const PLAYWRIGHT_BDD: Framework = Framework::new("playwright-bdd");
/// Cypress step definitions registered through the Badeball Cucumber preprocessor.
pub(crate) const CYPRESS_CUCUMBER: Framework = Framework::new("cypress-cucumber");
/// Jest-Cucumber's positional, scenario-local step callbacks.
pub(crate) const JEST_CUCUMBER: Framework = Framework::new("jest-cucumber");
/// Global registrations exported by vitest-cucumber-plugin.
pub(crate) const VITEST_CUCUMBER: Framework = Framework::new("vitest-cucumber");
/// Every framework this adapter attributes definitions to, in precedence order: feature
/// discovery tries them in this order, and evidence for two frameworks resolves to the earlier.
pub(crate) static FRAMEWORK_CONTRACTS: [FrameworkContract; 5] = [
    FrameworkContract::new(PLAYWRIGHT_BDD)
        .with_discovery(PLAYWRIGHT_BDD_DISCOVERY)
        .with_excludes(&[
            "**/.features-gen/**",
            "**/test-results/**",
            "**/playwright-report/**",
        ]),
    FrameworkContract::new(CYPRESS_CUCUMBER).with_discovery(CYPRESS_CUCUMBER_DISCOVERY),
    FrameworkContract::new(CUCUMBER_JS).with_discovery(CUCUMBER_JS_DISCOVERY),
    FrameworkContract::new(JEST_CUCUMBER).with_policy(SCENARIO_LOCAL),
    FrameworkContract::new(VITEST_CUCUMBER).with_policy(ISOLATED),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RegistrationExport {
    pub(super) canonical: String,
    pub(super) kind: RegistrationExportKind,
    pub(super) framework: Framework,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RegistrationExportKind {
    Call,
    Decorator,
    Factory,
    Feature,
    Unsupported,
    NonRegistration,
}

pub(super) type RegistrationExports = BTreeMap<String, RegistrationExport>;

/// Every registration name CukeDedup understands, including lowercase aliases.
pub(crate) const REGISTRATIONS: [&str; 8] = [
    "Given",
    "When",
    "Then",
    "Step",
    "defineStep",
    "given",
    "when",
    "then",
];

/// Registration names assumed to be globals when nothing in the file shadows them.
///
/// The lowercase aliases are deliberately absent: `given` is a plausible identifier in ordinary
/// code, so treating a bare call as a registration would invent step definitions.
pub(crate) const DEFAULT_REGISTRATIONS: [&str; 4] = ["Given", "When", "Then", "defineStep"];

pub(super) const CUCUMBER_MODULE: &str = "@cucumber/cucumber";
pub(super) const LEGACY_CUCUMBER_MODULE: &str = "cucumber";
pub(super) const PLAYWRIGHT_MODULE: &str = "playwright-bdd";
pub(super) const PLAYWRIGHT_DECORATORS_MODULE: &str = "playwright-bdd/decorators";
pub(super) const CYPRESS_MODULE: &str = "@badeball/cypress-cucumber-preprocessor";
pub(super) const LEGACY_CYPRESS_STEPS_MODULE: &str = "cypress-cucumber-preprocessor/steps";

/// One framework's known JavaScript/TypeScript registration entrypoints.
/// This metadata does not confer assertion-library trust or resolve lexical bindings.
struct FrameworkRegistration {
    modules: &'static [&'static str],
    framework: Framework,
    exports: ExportStyle,
}

#[derive(Clone, Copy)]
enum ExportStyle {
    Calls { factory: Option<&'static str> },
    Decorators,
    Callbacks(&'static CallbackModel),
}

/// How a framework whose steps are proven through callback provenance registers them.
pub(super) struct CallbackModel {
    /// The entrypoint's exports and their kinds.
    exports: &'static [(&'static str, RegistrationExportKind)],
    /// Properties of a scenario function that return the same scenario function.
    pub(super) scenario_modifiers: &'static [&'static str],
    /// Properties of the steps object a scenario callback receives.
    pub(super) steps: &'static [(&'static str, RegistrationExportKind)],
    /// Step calls register only inside a proven steps callback.
    pub(super) scenario_scoped: bool,
    /// A CommonJS `require` of the entrypoint is a supported runtime import.
    pub(super) commonjs: bool,
    /// String matchers are literal text, not Cucumber expressions.
    pub(super) literal_strings: bool,
}

/// Jest-Cucumber: `defineFeature` passes a scenario function whose callback receives the steps.
const JEST_CALLBACKS: CallbackModel = CallbackModel {
    exports: &[
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
    ],
    scenario_modifiers: &["only", "skip", "concurrent"],
    steps: &[
        ("given", Call),
        ("when", Call),
        ("then", Call),
        ("and", Call),
        ("but", Call),
        ("defineStep", Call),
        ("pending", NonRegistration),
    ],
    scenario_scoped: true,
    commonjs: true,
    literal_strings: true,
};

/// vitest-cucumber-plugin: ESM-only global registrations.
const VITEST_CALLBACKS: CallbackModel = CallbackModel {
    exports: &[("Given", Call), ("When", Call), ("Then", Call)],
    scenario_modifiers: &[],
    steps: &[],
    scenario_scoped: false,
    commonjs: false,
    literal_strings: false,
};

// Match exact entrypoints, never prefixes: lookalike packages/subpaths are not trusted.
// The legacy Cypress root exports a preprocessor, not step registrations.
const FRAMEWORK_REGISTRY: &[FrameworkRegistration] = &[
    FrameworkRegistration {
        modules: &["jest-cucumber"],
        framework: JEST_CUCUMBER,
        exports: ExportStyle::Callbacks(&JEST_CALLBACKS),
    },
    FrameworkRegistration {
        modules: &["vitest-cucumber-plugin"],
        framework: VITEST_CUCUMBER,
        exports: ExportStyle::Callbacks(&VITEST_CALLBACKS),
    },
    FrameworkRegistration {
        modules: &[CUCUMBER_MODULE, LEGACY_CUCUMBER_MODULE],
        framework: CUCUMBER_JS,
        exports: ExportStyle::Calls { factory: None },
    },
    FrameworkRegistration {
        modules: &[PLAYWRIGHT_MODULE],
        framework: PLAYWRIGHT_BDD,
        exports: ExportStyle::Calls {
            factory: Some("createBdd"),
        },
    },
    FrameworkRegistration {
        modules: &[PLAYWRIGHT_DECORATORS_MODULE],
        framework: PLAYWRIGHT_BDD,
        exports: ExportStyle::Decorators,
    },
    FrameworkRegistration {
        modules: &[CYPRESS_MODULE, LEGACY_CYPRESS_STEPS_MODULE],
        framework: CYPRESS_CUCUMBER,
        exports: ExportStyle::Calls { factory: None },
    },
];

/// Every module path known to export step registrations.
///
/// Sourced from the framework registry so that adding a framework extends every caller, rather
/// than leaving a second list to drift out of step with this one.
pub(crate) fn registration_modules() -> impl Iterator<Item = &'static str> {
    FRAMEWORK_REGISTRY
        .iter()
        .flat_map(|registration| registration.modules.iter().copied())
}

/// Looks up registration metadata for an exact supported module specifier.
fn registration_for_module(module: &str) -> Option<&'static FrameworkRegistration> {
    FRAMEWORK_REGISTRY
        .iter()
        .find(|entry| entry.modules.contains(&module))
}

/// Returns whether an exact framework entrypoint has known registration exports.
pub(super) fn is_supported_module(module: &str) -> bool {
    registration_for_module(module).is_some()
}

/// Returns the framework a supported module identifies.
pub(super) fn framework_for_module(module: &str) -> Framework {
    registration_for_module(module).map_or(Framework::UNKNOWN, |entry| entry.framework)
}

/// The callback model of `framework`, when its steps need callback provenance.
pub(super) fn callback_model(framework: Framework) -> Option<&'static CallbackModel> {
    FRAMEWORK_REGISTRY
        .iter()
        .filter(|entry| entry.framework == framework)
        .find_map(|entry| match entry.exports {
            ExportStyle::Callbacks(model) => Some(model),
            ExportStyle::Calls { .. } | ExportStyle::Decorators => None,
        })
}

/// Builds the export `canonical` of `kind` attributed to `framework`.
pub(super) fn export(
    canonical: &str,
    kind: RegistrationExportKind,
    framework: Framework,
) -> RegistrationExport {
    RegistrationExport {
        canonical: canonical.to_owned(),
        kind,
        framework,
    }
}

/// Returns the ordinary registration exports attributed to one framework.
pub(super) fn registration_exports_for_framework(framework: Framework) -> RegistrationExports {
    REGISTRATIONS
        .into_iter()
        .filter(|name| *name != "Step")
        .map(|name| {
            (
                name.to_owned(),
                RegistrationExport {
                    canonical: name.to_owned(),
                    kind: RegistrationExportKind::Call,
                    framework,
                },
            )
        })
        .collect()
}

/// Returns the registration exports known for one exact framework entrypoint.
///
/// `Step` is a Playwright-BDD decorator export, not a general registration export. Keeping it on
/// that exact subpath avoids inventing imports from the other supported packages.
pub(super) fn registration_exports_for_module(module: &str) -> RegistrationExports {
    let Some(entry) = registration_for_module(module) else {
        // Preserve the existing fallback; callers must establish package/project provenance
        // before using these names to recognize registrations.
        return registration_exports_for_framework(Framework::UNKNOWN);
    };
    let framework = entry.framework;
    if let ExportStyle::Callbacks(model) = entry.exports {
        return model
            .exports
            .iter()
            .map(|&(name, kind)| (name.to_owned(), export(name, kind, framework)))
            .collect();
    }
    if let ExportStyle::Calls { factory } = entry.exports {
        let mut exports = registration_exports_for_framework(framework);
        if let Some(factory) = factory {
            exports.insert(
                factory.to_owned(),
                RegistrationExport {
                    canonical: factory.to_owned(),
                    kind: RegistrationExportKind::Factory,
                    framework,
                },
            );
        }
        return exports;
    }
    ["Given", "When", "Then", "Step"]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                RegistrationExport {
                    canonical: name.to_owned(),
                    kind: RegistrationExportKind::Decorator,
                    framework,
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests;
