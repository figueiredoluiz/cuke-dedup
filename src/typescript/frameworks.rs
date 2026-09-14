//! Framework registration vocabulary for the JavaScript/TypeScript adapter.
//!
//! FRAMEWORK_REGISTRY supports entrypoints sharing the existing call exports (optionally with
//! a createBdd-style factory) or the fixed Given/When/Then/Step decorator exports. Different
//! export sets or registration syntax require extraction support and fixtures, not just a row.
//! Framework configuration discovery and user-facing compatibility docs are maintained separately.
//! Package recognition alone must not invent bindings or confer assertion-library trust.

use crate::model::Framework;
use std::collections::BTreeMap;

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
}

pub(super) type RegistrationExports = BTreeMap<String, RegistrationExport>;

/// Every registration name CukeDedup understands, including lowercase aliases.
pub(super) const REGISTRATIONS: [&str; 8] = [
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
pub(super) const DEFAULT_REGISTRATIONS: [&str; 4] = ["Given", "When", "Then", "defineStep"];

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
}

// Match exact entrypoints, never prefixes: lookalike packages/subpaths are not trusted.
// The legacy Cypress root exports a preprocessor, not step registrations.
const FRAMEWORK_REGISTRY: &[FrameworkRegistration] = &[
    FrameworkRegistration {
        modules: &[CUCUMBER_MODULE, LEGACY_CUCUMBER_MODULE],
        framework: Framework::CucumberJs,
        exports: ExportStyle::Calls { factory: None },
    },
    FrameworkRegistration {
        modules: &[PLAYWRIGHT_MODULE],
        framework: Framework::PlaywrightBdd,
        exports: ExportStyle::Calls {
            factory: Some("createBdd"),
        },
    },
    FrameworkRegistration {
        modules: &[PLAYWRIGHT_DECORATORS_MODULE],
        framework: Framework::PlaywrightBdd,
        exports: ExportStyle::Decorators,
    },
    FrameworkRegistration {
        modules: &[CYPRESS_MODULE, LEGACY_CYPRESS_STEPS_MODULE],
        framework: Framework::CypressCucumber,
        exports: ExportStyle::Calls { factory: None },
    },
];

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
    registration_for_module(module).map_or(Framework::Unknown, |entry| entry.framework)
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
        return registration_exports_for_framework(Framework::Unknown);
    };
    let framework = entry.framework;
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
mod tests {
    use super::*;

    #[test]
    fn framework_entrypoints_preserve_export_kinds() {
        for module in FRAMEWORK_REGISTRY
            .iter()
            .flat_map(|entry| entry.modules.iter().copied())
        {
            let exports = registration_exports_for_module(module);
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
                assert_eq!(exports["createBdd"].framework, Framework::PlaywrightBdd);
            }
        }
    }

    #[test]
    fn framework_entrypoints_are_unique_and_have_exact_attribution() {
        let expected = [
            ("@cucumber/cucumber", Framework::CucumberJs),
            ("cucumber", Framework::CucumberJs),
            ("playwright-bdd", Framework::PlaywrightBdd),
            ("playwright-bdd/decorators", Framework::PlaywrightBdd),
            (
                "@badeball/cypress-cucumber-preprocessor",
                Framework::CypressCucumber,
            ),
            (
                "cypress-cucumber-preprocessor/steps",
                Framework::CypressCucumber,
            ),
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
        ] {
            assert!(!is_supported_module(unsupported), "{unsupported}");
            assert_eq!(
                framework_for_module(unsupported),
                Framework::Unknown,
                "{unsupported}"
            );
        }
    }

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
            registration_exports_for_framework(Framework::Unknown).len() + 1,
            REGISTRATIONS.len()
        );
        assert!(!registration_exports_for_framework(Framework::Unknown).contains_key("Step"));
        assert!(registration_exports_for_module(PLAYWRIGHT_DECORATORS_MODULE).contains_key("Step"));
    }
}
