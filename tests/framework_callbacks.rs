use cuke_dedup::analysis::analyze;
use cuke_dedup::config::{Config, ConfigOverrides};
use cuke_dedup::model::{FeatureStep, Framework, Rule, SourceLocation};
use cuke_dedup::typescript::{self, SourceFile, SourceLanguage};
use std::collections::BTreeSet;
use std::path::PathBuf;

fn outcomes(
    source: &str,
    language: SourceLanguage,
) -> (Vec<Framework>, BTreeSet<Rule>, Vec<String>) {
    outcomes_with_steps(source, language, Vec::new())
}

fn outcomes_with_steps(
    source: &str,
    language: SourceLanguage,
    steps: Vec<FeatureStep>,
) -> (Vec<Framework>, BTreeSet<Rule>, Vec<String>) {
    let directory = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: PathBuf::from("steps.ts"),
        language,
    };
    let extracted = typescript::extract_detailed(source, &file).unwrap();
    let frameworks = extracted
        .definitions
        .iter()
        .map(|definition| definition.framework)
        .collect();
    let diagnostics = extracted
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    let result = analyze(
        extracted.definitions,
        steps,
        &Config::load(directory.path(), ConfigOverrides::default()).unwrap(),
    )
    .unwrap();
    let rules = result
        .findings
        .into_iter()
        .map(|finding| finding.rule)
        .collect();
    (frameworks, rules, diagnostics)
}

#[test]
fn positional_regex_steps_do_not_enter_global_usage_but_plugin_steps_do() {
    let body =
        "given(/the .+ is ready/, () => first()); given(/^the parcel is ready$/, () => second());";
    for (setup, positional) in [(format!("import {{defineFeature}} from 'jest-cucumber'; defineFeature(feature, test => test('one', ({{given}}) => {{{body}}}));"), true),
        (format!("import {{Given as given}} from 'vitest-cucumber-plugin'; {body}"), false)] {
        let steps = vec![FeatureStep { keyword: "Given".into(), text: "the parcel is ready".into(), location: SourceLocation::new("steps.feature", 3, 1, 3, 24) }];
        let (frameworks, rules, diagnostics) = outcomes_with_steps(&setup, SourceLanguage::TypeScript, steps);
        assert_eq!(frameworks.len(), 2);
        assert_eq!(rules.contains(&Rule::AmbiguousStep), !positional, "{rules:?}");
        assert!(!rules.contains(&Rule::UnusedDefinition));
        assert!(diagnostics.is_empty());
    }
}

#[test]
fn local_framework_barrels_resolve_without_granting_assertion_facade_trust() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("package.json"), "{}").unwrap();
    std::fs::write(directory.path().join("barrel.ts"), "export { defineFeature } from 'jest-cucumber'; export { Given } from 'vitest-cucumber-plugin'; export const expect = custom;").unwrap();
    for (setup, callee) in [
        ("", "Given"),
        ("function pluginStep(p, h) { Given(p, h); }", "pluginStep"),
    ] {
        let source = format!("import {{ defineFeature, Given, expect }} from './barrel'; {setup} defineFeature(feature, test => test('one', ({{given}}) => {{ given('first outcome', () => expect(value).toBe(1)); given('second outcome', () => expect(value).toBe(2)); }})); {callee}('plugin first', () => work()); {callee}('plugin second', () => work());");
        let file = SourceFile {
            path: directory.path().join("steps.ts"),
            language: SourceLanguage::TypeScript,
        };
        std::fs::write(&file.path, &source).unwrap();
        let extraction = typescript::extract_detailed(&source, &file).unwrap();
        assert_eq!(extraction.definitions.len(), 4);
        assert!(
            extraction.diagnostics.is_empty(),
            "{:?}",
            extraction.diagnostics
        );
        let result = analyze(
            extraction.definitions,
            Vec::new(),
            &Config::load(directory.path(), ConfigOverrides::default()).unwrap(),
        )
        .unwrap();
        assert!(result
            .findings
            .iter()
            .any(|finding| finding.rule == Rule::DuplicateHandler));
        // The untrusted expect is an ordinary call: its literal variation remains parameterizable.
        assert!(result
            .findings
            .iter()
            .any(|finding| finding.rule == Rule::ParameterizationCandidate));
    }
}

#[test]
fn unsupported_jest_setup_keeps_cli_reports_incomplete() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("steps.ts"),
        "import {autoBindSteps} from 'jest-cucumber'; autoBindSteps(features, helpers);",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("sample.feature"),
        "Feature: scope\n Scenario: first\n  Given first action\n",
    )
    .unwrap();
    let (output, value) = strict_report(directory.path());
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unresolved step-registration calls"));
    assert_eq!(value["corpus"]["incomplete"], true);
}

fn strict_report(directory: &std::path::Path) -> (std::process::Output, serde_json::Value) {
    let report = directory.join("report");
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin!("cuke-dedup"))
        .arg(directory)
        .args(["--fail-on-incomplete", "--reporters", "json", "--output"])
        .arg(&report)
        .output()
        .unwrap();
    let value =
        serde_json::from_slice(&std::fs::read(report.join("cuke-dedup.json")).unwrap()).unwrap();
    (output, value)
}

fn has(rules: &BTreeSet<Rule>, rule: Rule) -> bool {
    rules.contains(&rule)
}

#[test]
fn forwarding_wrappers_use_lexical_targets_and_immediate_execution() {
    let forms = [
        ("function step(p, h) { given(p, h); }", true),
        ("function step(p, h) { return given(p, h); }", true),
        (
            "function inner(p, h) { given(p, h); } function step(p, h) { inner(p, h); }",
            true,
        ),
        ("function step(p, h, given) { given(p, h); }", false),
        (
            "function step(p, h) { given(p, h); } step = unrelated;",
            false,
        ),
        (
            "function step(p, h) { given(p, h); } given = unrelated;",
            false,
        ),
        ("async function step(p, h) { given(p, h); }", false),
        ("function* step(p, h) { given(p, h); }", false),
        ("function step(p, h) { given(h, p); }", false),
        ("function step(p, h) { later(() => given(p, h)); }", false),
    ];
    for (wrapper, supported) in forms {
        let body = format!(
            "{wrapper} step('first action', () => work()); step('second action', () => work());"
        );
        for source in [
            format!("import {{Given as given}} from 'vitest-cucumber-plugin'; {body}"),
            format!("import {{defineFeature}} from 'jest-cucumber'; defineFeature(feature, test => test('one', ({{given}}) => {{{body}}}));"),
            format!("const {{defineFeature}} = require('jest-cucumber'); defineFeature(feature, test => test('one', ({{given}}) => {{{body}}}));"),
        ] {
            let (frameworks, rules, diagnostics) = outcomes(&source, SourceLanguage::TypeScript);
            assert_eq!(frameworks.len(), if supported { 2 } else { 0 }, "{source}: {diagnostics:?}");
            assert_eq!(has(&rules, Rule::DuplicateHandler), supported, "{source}: {rules:?}");
            assert_eq!(diagnostics.is_empty(), supported, "{source}: {diagnostics:?}");
        }
    }
}

#[test]
fn ambient_jest_fallback_preserves_literal_kind_and_positional_rules() {
    let source = "import 'jest-cucumber'; Given('literal { int }', () => work()); Given('literal {int}', () => work());";
    let file = SourceFile {
        path: "steps.ts".into(),
        language: SourceLanguage::TypeScript,
    };
    let extracted = typescript::extract_detailed(source, &file).unwrap();
    assert_eq!(extracted.definitions.len(), 2);
    for definition in extracted.definitions {
        assert_eq!(
            definition.matcher_kind,
            cuke_dedup::model::MatcherKind::Literal
        );
        assert_eq!(definition.matcher, definition.normalized_matcher);
    }
    let (_, rules, diagnostics) = outcomes(source, SourceLanguage::TypeScript);
    assert_eq!(rules, BTreeSet::from([Rule::DuplicateHandler]));
    assert!(diagnostics.is_empty());
}

#[test]
fn fallback_scope_and_writes_never_gain_registration_trust() {
    let calls = "Given('first action', () => work()); Given('second action', () => work());";
    for package in ["jest-cucumber", "vitest-cucumber-plugin"] {
        for (setup, body, supported) in [
            (
                "import type { Given } from 'unrelated';",
                calls.to_owned(),
                true,
            ),
            (
                "",
                format!("function unrelated(Given) {{ {calls} }}"),
                false,
            ),
            ("Given = unrelated;", calls.to_owned(), false),
            ("", format!("{{ const Given = unrelated; {calls} }}"), false),
        ] {
            let source = format!("import '{package}'; {setup} {body}");
            let (frameworks, rules, diagnostics) = outcomes(&source, SourceLanguage::TypeScript);
            assert_eq!(frameworks.len(), if supported { 2 } else { 0 }, "{source}");
            assert_eq!(has(&rules, Rule::DuplicateHandler), supported, "{source}");
            assert_eq!(
                diagnostics.is_empty(),
                supported,
                "{source}: {diagnostics:?}"
            );
        }
    }
}

fn assert_no_handler_findings(rules: &BTreeSet<Rule>) {
    for rule in [
        Rule::DuplicateHandler,
        Rule::NearDuplicateStep,
        Rule::ParameterizationCandidate,
    ] {
        assert!(!has(rules, rule), "unexpected {rule} in {rules:?}");
    }
}

const JEST_SCENARIOS: &str = r#"
  suite(feature, test => {
    test('first scenario', ({ given }) => given('first action', () => work()));
    test('second scenario', ({ given }) => given('second action', () => work()));
  });
"#;

#[test]
fn jest_import_and_binding_forms_preserve_handler_findings_but_skip_matcher_rules() {
    let import_forms = [
        (
            "ESM named alias",
            "import { defineFeature as suite } from 'jest-cucumber';",
        ),
        (
            "ESM namespace",
            "import * as jc from 'jest-cucumber'; const suite = jc.defineFeature;",
        ),
        (
            "CJS destructuring",
            "const { defineFeature: suite } = require('jest-cucumber');",
        ),
        (
            "CJS namespace",
            "const jc = require('jest-cucumber'); const suite = jc.defineFeature;",
        ),
        (
            "TypeScript import-equals",
            "import jc = require('jest-cucumber'); const suite = jc.defineFeature;",
        ),
    ];

    for (label, import) in import_forms {
        let source = format!("{import}\n{JEST_SCENARIOS}");
        let (frameworks, rules, diagnostics) = outcomes(&source, SourceLanguage::TypeScript);
        assert_eq!(frameworks.len(), 2, "{label}");
        assert!(
            frameworks
                .iter()
                .all(|framework| *framework == Framework::JestCucumber),
            "{label}"
        );
        assert!(has(&rules, Rule::DuplicateHandler), "{label}: {rules:?}");
        assert!(
            [
                Rule::DuplicateMatcher,
                Rule::NormalizedMatcher,
                Rule::AmbiguousStep,
                Rule::OverlappingMatcher,
                Rule::UnusedDefinition,
            ]
            .iter()
            .all(|rule| !has(&rules, *rule)),
            "{label}: scenario-local steps must not trigger matcher-only rules: {rules:?}"
        );
        assert!(diagnostics.is_empty(), "{label}: {diagnostics:?}");
    }
}

#[test]
fn matcher_rules_stay_inapplicable_for_identical_matchers_in_one_scenario() {
    let source = r#"
      import { defineFeature } from 'jest-cucumber';
      defineFeature(feature, test => test('one scenario', ({ given }) => {
        given('same matcher', () => first());
        given('same matcher', () => second());
      }));
    "#;
    let (frameworks, rules, diagnostics) = outcomes(source, SourceLanguage::TypeScript);

    assert_eq!(frameworks.len(), 2);
    assert!(frameworks
        .iter()
        .all(|framework| *framework == Framework::JestCucumber));
    assert!(!has(&rules, Rule::DuplicateMatcher));
    assert!(!has(&rules, Rule::NormalizedMatcher));
    assert!(!has(&rules, Rule::AmbiguousStep));
    assert!(!has(&rules, Rule::OverlappingMatcher));
    assert!(!has(&rules, Rule::UnusedDefinition));
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn jest_equal_text_keeps_handler_candidates_without_redundant_near_findings() {
    let source = "import { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => test('one', ({given}) => { given('same', () => work()); given('same', () => work()); }));";
    let (frameworks, rules, diagnostics) = outcomes(source, SourceLanguage::TypeScript);
    assert_eq!(frameworks, [Framework::JestCucumber; 2]);
    assert_eq!(rules, BTreeSet::from([Rule::DuplicateHandler]));
    assert!(diagnostics.is_empty());
}

#[test]
fn jest_literal_text_is_preserved_in_reports_and_handler_comparisons() {
    let source = "import { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => test('one', ({given}) => { given('literal { int }', () => work()); given('literal {int}', () => work()); }));";
    let extracted = typescript::extract_detailed(
        source,
        &SourceFile {
            path: "steps.ts".into(),
            language: SourceLanguage::TypeScript,
        },
    )
    .unwrap();
    assert_eq!(extracted.definitions.len(), 2);
    for definition in &extracted.definitions {
        assert_eq!(
            serde_json::to_value(definition.matcher_kind).unwrap(),
            "literal"
        );
        assert_eq!(definition.matcher, definition.normalized_matcher);
    }
    let (_, rules, _) = outcomes(source, SourceLanguage::TypeScript);
    assert_eq!(rules, BTreeSet::from([Rule::DuplicateHandler]));
}

#[test]
fn jest_assertion_conflicts_depend_on_trusted_provenance() {
    for (module, trusted) in [("@jest/globals", true), ("./custom-assertions", false)] {
        let source = r#"
      import { defineFeature } from 'jest-cucumber';
      import { expect } from 'ASSERTION_MODULE';
      defineFeature(feature, test => {
        test('first scenario', ({ given }) => given('first outcome', () => expect(value).toBe(1)));
        test('second scenario', ({ given }) => given('second outcome', () => expect(value).toBe(2)));
      });
    "#.replace("ASSERTION_MODULE", module);
        let (frameworks, rules, diagnostics) = outcomes(&source, SourceLanguage::TypeScript);
        assert_eq!(frameworks, [Framework::JestCucumber; 2]);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        if trusted {
            assert_no_handler_findings(&rules);
        } else {
            // Unknown assertions are ordinary calls with literal variation, not trusted conflicts.
            assert!(!has(&rules, Rule::DuplicateHandler));
            assert!(has(&rules, Rule::ParameterizationCandidate));
        }
    }
}

#[test]
fn ordinary_vitest_registrations_keep_matcher_rules_and_duplicate_handler_control() {
    let source = r#"
      import { Given } from 'vitest-cucumber-plugin';
      Given('same matcher', () => first());
      Given('same matcher', () => second());
      Given('different matcher', () => work());
      Given('another matcher', () => work());
    "#;
    let (frameworks, rules, diagnostics) = outcomes(source, SourceLanguage::TypeScript);

    assert_eq!(frameworks.len(), 4);
    assert!(frameworks
        .iter()
        .all(|framework| *framework == Framework::VitestCucumber));
    assert!(has(&rules, Rule::DuplicateMatcher), "{rules:?}");
    assert!(has(&rules, Rule::DuplicateHandler), "{rules:?}");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn jest_and_vitest_handlers_are_not_compared_across_callback_conventions() {
    let source = r#"
      import { defineFeature } from 'jest-cucumber';
      import { Given } from 'vitest-cucumber-plugin';
      defineFeature(feature, test => test('one scenario', ({ given }) => given('jest action', () => work())));
      Given('vitest action', () => work());
    "#;
    let (frameworks, rules, diagnostics) = outcomes(source, SourceLanguage::TypeScript);

    assert_eq!(frameworks.len(), 2);
    assert!(frameworks.contains(&Framework::JestCucumber));
    assert!(frameworks.contains(&Framework::VitestCucumber));
    assert_no_handler_findings(&rules);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
}

#[test]
fn jest_callback_shapes_extract_only_when_synchronous_and_statically_resolved() {
    let supported = [
        (
            "arrow callback",
            r#"import { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));"#,
        ),
        (
            "function callback",
            r#"import { defineFeature } from 'jest-cucumber'; defineFeature(feature, function(test) { test('one', function({ given }) { given('action', () => work()); }); });"#,
        ),
        (
            "static computed scenario method",
            r#"import { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => test['only']('one', ({ given }) => given('action', () => work())));"#,
        ),
        (
            "direct local const alias",
            r#"import { defineFeature } from 'jest-cucumber'; const suite = defineFeature; suite(feature, test => test('one', ({ given: register }) => register('action', () => work())));"#,
        ),
        (
            "options namespace member",
            r#"import { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => test('one', steps => steps.given('action', () => work())));"#,
        ),
        (
            "local destructured alias",
            r#"import { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => test('one', steps => { const { given: register } = steps; register('action', () => work()); }));"#,
        ),
        (
            "type-only name does not shadow runtime callback",
            r#"import { defineFeature } from 'jest-cucumber'; type test = (title: string, callback: unknown) => void; defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));"#,
        ),
    ];
    for (label, source) in supported {
        let (frameworks, _, diagnostics) = outcomes(source, SourceLanguage::TypeScript);
        assert_eq!(frameworks.len(), 1, "{label}: {diagnostics:?}");
        assert_eq!(frameworks[0], Framework::JestCucumber, "{label}");
        assert!(diagnostics.is_empty(), "{label}: {diagnostics:?}");
    }

    let unsupported = [
        (
            "generator callback",
            r#"import { defineFeature } from 'jest-cucumber'; defineFeature(feature, function*(test) { test('one', ({ given }) => given('action', () => work())); });"#,
        ),
        (
            "escaped helper callback",
            r#"import { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => registerScenarios(test)); function registerScenarios(test) { test('one', ({ given }) => given('action', () => work())); }"#,
        ),
        (
            "dynamic scenario member",
            r#"import { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => test[scenarioName](({ given }) => given('action', () => work())));"#,
        ),
        (
            "async setup callback",
            r#"import { defineFeature } from 'jest-cucumber'; defineFeature(feature, async test => { test('one', ({ given }) => given('action', () => work())); });"#,
        ),
    ];
    for (label, source) in unsupported {
        let (frameworks, _, diagnostics) = outcomes(source, SourceLanguage::TypeScript);
        assert!(
            frameworks.is_empty(),
            "{label} unexpectedly yielded {frameworks:?}"
        );
        assert!(
            !diagnostics.is_empty(),
            "{label} must mark extraction incomplete"
        );
    }
}

#[test]
fn jest_resolution_fails_closed_for_type_only_unknown_and_shadowed_bindings() {
    // Each negative row has a real-looking call in an untrusted or shadowed binding position.
    // The supported baseline proves that the zero-definition rows are not caused by disabling
    // Jest extraction altogether.
    let positive = r#"
      import { defineFeature } from 'jest-cucumber';
      defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));
    "#;
    let (positive_frameworks, _, _) = outcomes(positive, SourceLanguage::TypeScript);
    assert_eq!(positive_frameworks, [Framework::JestCucumber]);

    let negative = [
        (
            "type-only import",
            r#"import type { defineFeature } from 'jest-cucumber'; defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));"#,
        ),
        (
            "unknown package origin",
            r#"import { defineFeature } from './unknown-jest-cucumber'; defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));"#,
        ),
        (
            "function parameter shadow",
            r#"import { defineFeature } from 'jest-cucumber'; function local(defineFeature) { defineFeature(feature, test => test('one', ({ given }) => given('action', () => work()))); }"#,
        ),
        (
            "block shadow",
            r#"import { defineFeature } from 'jest-cucumber'; { const defineFeature = fake; defineFeature(feature, test => test('one', ({ given }) => given('action', () => work()))); }"#,
        ),
        (
            "hoisted var shadow",
            r#"import { defineFeature } from 'jest-cucumber'; function local() { defineFeature(feature, test => test('one', ({ given }) => given('action', () => work()))); var defineFeature = fake; }"#,
        ),
        (
            "catch binding shadow",
            r#"import { defineFeature } from 'jest-cucumber'; try { throw 0; } catch (defineFeature) { defineFeature(feature, test => test('one', ({ given }) => given('action', () => work()))); }"#,
        ),
        (
            "reassigned local alias",
            r#"import { defineFeature } from 'jest-cucumber'; let suite = defineFeature; suite = fake; suite(feature, test => test('one', ({ given }) => given('action', () => work())));"#,
        ),
    ];
    for (label, source) in negative {
        let (frameworks, _, _) = outcomes(source, SourceLanguage::TypeScript);
        assert!(
            frameworks.is_empty(),
            "{label} unexpectedly yielded {frameworks:?}"
        );
    }
}

#[test]
fn vitest_plugin_esm_bindings_have_positive_and_fail_closed_controls() {
    let positive = [
        (
            "named import alias",
            "import { Given as given } from 'vitest-cucumber-plugin';",
            "given",
        ),
        (
            "namespace member",
            "import * as plugin from 'vitest-cucumber-plugin';",
            "plugin.Given",
        ),
        (
            "static computed namespace member",
            "import * as plugin from 'vitest-cucumber-plugin';",
            "plugin['Given']",
        ),
        (
            "destructured namespace alias",
            "import * as plugin from 'vitest-cucumber-plugin'; const { Given: given } = plugin;",
            "given",
        ),
        (
            "static computed destructuring",
            "import * as plugin from 'vitest-cucumber-plugin'; const { ['Given']: given } = plugin;",
            "given",
        ),
    ];
    for (label, setup, registration) in positive {
        let source = format!(
            "{setup}\n{registration}('first action', () => work()); {registration}('second action', () => work());"
        );
        let (frameworks, rules, _) = outcomes(&source, SourceLanguage::TypeScript);
        assert_eq!(frameworks, [Framework::VitestCucumber; 2], "{label}");
        assert!(has(&rules, Rule::DuplicateHandler), "{label}: {rules:?}");
    }

    let negative = [
        "import type { Given as register } from 'vitest-cucumber-plugin'; register('action', () => work());",
        "import plugin = require('vitest-cucumber-plugin'); plugin.Given('action', () => work());",
        "import Given from 'vitest-cucumber-plugin'; Given('action', () => work());",
        "import { Given } from 'vitest-cucumber-plugin-extra'; Given('action', () => work());",
        "import { Given } from 'vitest-cucumber-plugin'; function local(Given) { Given('action', () => work()); }",
        "import { Given } from 'vitest-cucumber-plugin'; { const Given = fake; Given('action', () => work()); }",
        "import { Given } from 'vitest-cucumber-plugin'; try { throw 0; } catch (Given) { Given('action', () => work()); }",
        "import { Given } from 'vitest-cucumber-plugin'; function local() { Given('action', () => work()); var Given = fake; }",
        "import * as plugin from 'vitest-cucumber-plugin'; plugin.Given = fake; plugin.Given('action', () => work());",
        "import * as plugin from 'vitest-cucumber-plugin'; const steps = plugin; steps.Given = fake; steps.Given('action', () => work());",
        "import { Given as given } from 'vitest-cucumber-plugin'; given = fake; given('action', () => work());",
        "import { Given } from 'vitest-cucumber-plugin'; let given = Given; given('action', () => work());",
        "import { Given } from './unknown-setup'; Given('action', () => work());",
    ];
    for source in negative {
        let (frameworks, _, _) = outcomes(source, SourceLanguage::TypeScript);
        assert!(
            frameworks.is_empty(),
            "unexpected Vitest definition for {source}: {frameworks:?}"
        );
    }

    let ambient = r#"
      import type { Given } from 'vitest-cucumber-plugin';
      Given('ambient first', () => work());
      Given('ambient second', () => work());
    "#;
    let (frameworks, rules, _) = outcomes(ambient, SourceLanguage::TypeScript);
    assert_eq!(frameworks.len(), 2);
    assert!(frameworks
        .iter()
        .all(|framework| *framework != Framework::VitestCucumber));
    assert!(
        has(&rules, Rule::DuplicateHandler),
        "ambient fallback changed: {rules:?}"
    );
}

#[test]
fn jest_requires_and_scenario_callback_aliases_respect_shadowing_and_escape() {
    let common_js_shadows = [
        "const require = fake; import jc = require('jest-cucumber'); jc.defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));",
        "require = fake; const { defineFeature } = require('jest-cucumber'); defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));",
        "function require(name) { return fake; } const { defineFeature } = require('jest-cucumber'); defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));",
        "const require = fake; const { defineFeature } = require('jest-cucumber'); defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));",
        "class require {} const { defineFeature } = require('jest-cucumber'); defineFeature(feature, test => test('one', ({ given }) => given('action', () => work())));",
    ];
    for source in common_js_shadows {
        let (frameworks, _, _) = outcomes(source, SourceLanguage::TypeScript);
        assert!(
            frameworks.is_empty(),
            "shadowed require yielded {frameworks:?}: {source}"
        );
    }

    let aliases = r#"
      import { defineFeature } from 'jest-cucumber';
      defineFeature(feature, test => {
        test('first', ({ given: register }) => register('first action', () => work()));
        test('second', steps => steps.given('second action', () => work()));
      });
    "#;
    let (frameworks, rules, _) = outcomes(aliases, SourceLanguage::TypeScript);
    assert_eq!(frameworks, [Framework::JestCucumber; 2]);
    assert!(has(&rules, Rule::DuplicateHandler), "{rules:?}");

    let writes = r#"
      import { defineFeature } from 'jest-cucumber';
      defineFeature(feature, test => test('one', steps => {
        const alias = steps;
        alias.given = fake;
        steps.given('must not register', () => work());
      }));
    "#;
    let (frameworks, _, _) = outcomes(writes, SourceLanguage::TypeScript);
    assert!(
        frameworks.is_empty(),
        "written callback property yielded {frameworks:?}"
    );

    let escaped = r#"
      import { defineFeature } from 'jest-cucumber';
      let escapedTest;
      defineFeature(feature, test => {
        escapedTest = test;
        test('first', ({ given }) => given('first action', () => work()));
        test('second', ({ given }) => given('second action', () => work()));
      });
      escapedTest('outside setup', ({ given }) => given('escaped action', () => work()));
    "#;
    let (frameworks, rules, _) = outcomes(escaped, SourceLanguage::TypeScript);
    assert_eq!(frameworks, [Framework::JestCucumber; 2]);
    assert!(has(&rules, Rule::DuplicateHandler), "{rules:?}");
}

#[test]
fn jest_uncertain_setup_argument_positions_and_mixed_scope_never_register() {
    for source in [
        "import { defineFeature } from 'jest-cucumber'; defineFeature(...features, test => test('one', ({given}) => given('action', () => work())));",
        "import { Given } from '@cucumber/cucumber'; import { defineFeature } from 'jest-cucumber'; defineFeature(feature, function*(test) { test('one', ({ Given }) => Given('action', () => work())); });",
        "import * as jc from 'jest-cucumber'; const alias = jc; alias.defineFeature = fake; jc.defineFeature(feature, test => test('one', ({given}) => given('action', () => work())));",
    ] {
        let (frameworks, rules, diagnostics) = outcomes(source, SourceLanguage::TypeScript);
        assert!(frameworks.is_empty(), "{source}: {frameworks:?}");
        assert_no_handler_findings(&rules);
        assert!(!diagnostics.is_empty(), "{source} must remain incomplete");
    }
}

#[test]
fn unmodeled_aliases_keep_framework_evidence_without_registering_steps() {
    for source in [
        "import {defineFeature as suite} from 'jest-cucumber'; let setup=suite; setup(feature, s=>s('one',({given:g})=>g('action',()=>work())));",
        "import {defineFeature} from 'jest-cucumber'; defineFeature(feature,t=>t('one',steps=>{mutate(steps);steps.given('action',()=>work());}));",
        "import * as jc from 'jest-cucumber'; const setup=jc[entry]; setup(feature,s=>s('one',({given:g})=>g('action',()=>work())));",
        "import * as plugin from 'vitest-cucumber-plugin'; const register=plugin[key]; register('action',()=>work());",
        "import * as plugin from 'vitest-cucumber-plugin'; const {Given: register = fallback}=plugin; register('action',()=>work());",
    ] {
        let (frameworks, _, diagnostics) = outcomes(source, SourceLanguage::TypeScript);
        assert!(frameworks.is_empty(), "{source}");
        assert!(!diagnostics.is_empty(), "silent unsupported alias: {source}");
    }
}

#[test]
fn known_helpers_preserve_complete_handler_outcomes() {
    for helper in [
        "loadFeature",
        "loadFeatures",
        "parseFeature",
        "setJestCucumberConfiguration",
        "generateCodeFromFeature",
        "generateCodeWithSeparateFunctionsFromFeature",
    ] {
        for setup in [
            format!("const {{defineFeature, {helper}: help}} = require('jest-cucumber');"),
            format!("import {{defineFeature, {helper} as help}} from 'jest-cucumber';"),
            format!("import * as jc from 'jest-cucumber'; const {{defineFeature}}=jc; const help=jc.{helper};"),
            format!("import jc = require('jest-cucumber'); const {{defineFeature, ['{helper}']: help}}=jc;"),
            format!("const jc=require('jest-cucumber'); const {{defineFeature}}=jc; const help=jc['{helper}'];"),
        ] {
            for (callback, pending) in [("steps =>", "steps.pending"), ("function(steps)", "steps['pending']")] {
                for equal in [true, false] {
                    let right = if equal { 1 } else { 2 };
                    let source = format!("import {{expect}} from '@jest/globals'; {setup} help('example'); consume({{help}}); defineFeature(feature, test => test('one', {callback} {{ const {{given, pending: wait}}=steps; {pending}(); wait(); consume({{wait}}); given('first',()=>expect(value).toBe(1)); given('second',()=>expect(value).toBe({right})); }}));");
                    let (definitions, rules, diagnostics) = outcomes(&source, SourceLanguage::TypeScript);
                    assert_eq!(definitions.len(), 2, "{source}: {diagnostics:?}");
                    assert!(diagnostics.is_empty(), "{source}: {diagnostics:?}");
                    assert_eq!(has(&rules, Rule::DuplicateHandler), equal, "{source}: {rules:?}");
                    if !equal {
                        assert!(!has(&rules, Rule::NearDuplicateStep));
                        assert!(!has(&rules, Rule::ParameterizationCandidate));
                    }
                }
            }
        }
    }
}

#[test]
fn helper_exemptions_do_not_hide_uncertain_or_escaped_registrations() {
    for (setup, count) in [
        ("jc.missing();", 2),
        ("jc[key]();", 2),
        ("const {loadFeature: help = fallback}=jc; help();", 2),
        ("const {[key]: help}=jc; help();", 2),
        ("let help=jc.loadFeature; help();", 2),
        ("const help=jc.loadFeature; help=other; help();", 0),
        ("jc.loadFeature=other; jc.loadFeature();", 0),
        ("consume(jc); jc.loadFeature();", 0),
        ("jc.loadFeature(jc);", 0),
        ("jc.loadFeature({jc});", 0),
        ("for (jc of values) {}", 0),
        ("({value: jc}=source);", 0),
    ] {
        let source = format!("import * as jc from 'jest-cucumber'; {setup} jc.defineFeature(feature,t=>t('one',({{given}})=>{{given('first',()=>work());given('second',()=>work());}}));");
        let (definitions, rules, diagnostics) = outcomes(&source, SourceLanguage::TypeScript);
        assert_eq!(definitions.len(), count, "{source}: {diagnostics:?}");
        assert_eq!(has(&rules, Rule::DuplicateHandler), count == 2, "{source}");
        assert!(!diagnostics.is_empty(), "{source}");
    }
    for callback in ["async steps =>", "function* (steps)"] {
        let source = format!("import {{defineFeature}} from 'jest-cucumber'; defineFeature(feature,t=>t('one',{callback} {{steps.pending();steps.given('first',()=>work());}}));");
        let (definitions, _, diagnostics) = outcomes(&source, SourceLanguage::TypeScript);
        assert!(definitions.is_empty());
        assert!(!diagnostics.is_empty());
    }
}

#[test]
fn helper_barrels_require_export_provenance_and_keep_strict_cli_complete() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("sample.feature"),
        "Feature: helpers\n Scenario: one\n  Given first\n",
    )
    .unwrap();
    for (barrel, complete) in [
        ("export {defineFeature, loadFeature} from 'jest-cucumber';", true),
        ("module.exports=require('jest-cucumber');", true),
        ("const jc=require('jest-cucumber'); exports.defineFeature=jc.defineFeature; exports.loadFeature=jc.loadFeature;", false),
        ("export {defineFeature} from 'jest-cucumber'; export const loadFeature=unrelated;", false),
    ] {
        std::fs::write(directory.path().join("barrel.ts"), barrel).unwrap();
        std::fs::write(directory.path().join("steps.ts"), "const {defineFeature, loadFeature}=require('./barrel'); loadFeature('sample.feature'); defineFeature(feature,t=>t('one',({given})=>given('first',()=>work())));").unwrap();
        let (output, value) = strict_report(directory.path());
        assert_eq!(output.status.code(), Some(if complete { 0 } else { 2 }), "{barrel}: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(value["corpus"]["incomplete"], !complete);
    }
}

#[test]
fn callback_discovery_preserves_outcomes_through_adjacent_syntax() {
    for setup in [
        "export default function() {};",
        "function unrelated({value}, handler) { target(value, handler); }",
        "let unrelated; class Other {} enum Choice { One }",
        "declare class Other {} declare const erased: unknown;",
        "import Alias = Other.Member; require(dynamicModule);",
        "function local(loadFeature) { loadFeature(); }",
        "function local() { class loadFeature {} }",
        "const {loadFeature: help}=jc; consume(help); jc.loadFeature('sample');",
        "for (const jc of objects) { consume(jc); }",
        "factory().value = 1;",
    ] {
        let source = format!("import * as jc from 'jest-cucumber'; {setup} jc.defineFeature(feature,t=>t('one',({{given}})=>{{given('first',()=>work());given('second',()=>work());}}));");
        let (definitions, rules, diagnostics) = outcomes(&source, SourceLanguage::TypeScript);
        assert_eq!(definitions.len(), 2, "{source}: {diagnostics:?}");
        assert!(has(&rules, Rule::DuplicateHandler), "{source}");
        assert!(diagnostics.is_empty(), "{source}: {diagnostics:?}");
    }
    for setup in [
        "const {...rest}=jc; rest.missing();",
        "const {loadFeature: help, ...rest}=jc; rest.missing();",
    ] {
        let source = format!("import * as jc from 'jest-cucumber'; {setup}");
        let (definitions, _, diagnostics) = outcomes(&source, SourceLanguage::TypeScript);
        assert!(definitions.is_empty());
        assert!(!diagnostics.is_empty(), "{source}");
    }
    let mut source = "import * as jc from 'jest-cucumber'; const a0=jc;".to_owned();
    for n in 1..40 {
        source.push_str(&format!("const a{n}=a{};", n - 1));
    }
    for mutate in ["", "a39=unknown;"] {
        let (definitions, rules, diagnostics) = outcomes(
            &format!("{source} {mutate} a39.defineFeature(feature, setup);"),
            SourceLanguage::TypeScript,
        );
        assert!(definitions.is_empty());
        assert!(rules.is_empty());
        assert!(!diagnostics.is_empty());
    }
}

#[test]
fn deep_alias_writes_and_escapes_invalidate_roots_without_losing_uncertainty() {
    for (package, control, member) in [
        ("jest-cucumber", "root.defineFeature(feature,t=>t('one',({given})=>{given('first',()=>work());given('second',()=>work());}));", "defineFeature"),
        ("vitest-cucumber-plugin", "root.Given('first',()=>work());root.Given('second',()=>work());", "Given"),
    ] {
        for length in [1, 30, 32, 33, 40, 96] {
            let mut source = format!("import * as root from '{package}'; const a0=root;");
            for n in 1..length {
                source.push_str(&format!("const a{n}=a{};", n - 1));
            }
            let alias = format!("a{}", length - 1);
            for (effect, count, incomplete) in [
                (String::new(), 2, false),
                (format!("{alias}.{member}=other;"), 0, true),
                (format!("{alias}['{member}']=other;"), 0, true),
                (format!("{alias}=other;"), 0, true),
                (format!("consume({alias});"), 0, true),
                (format!("consume({{{alias}}});"), 0, true),
                (format!("for ({alias} of values) {{}}"), 0, true),
                (format!("{alias}.missing();"), 2, true),
                ("const x=y; const y=x; x.property=other;".into(), 2, false),
                ("function x(p,h){y(p,h);} function y(p,h){x(p,h);} x('unknown',()=>work());".into(), 2, true),
            ] {
                let input = format!("{source} {effect} {control}");
                let (definitions, rules, diagnostics) = outcomes(&input, SourceLanguage::TypeScript);
                assert_eq!(definitions.len(), count, "{input}: {diagnostics:?}");
                assert_eq!(has(&rules, Rule::DuplicateHandler), count == 2, "{input}");
                assert_eq!(!diagnostics.is_empty(), incomplete, "{input}: {diagnostics:?}");
            }
        }
    }
}

#[test]
fn deep_alias_cli_rejects_incomplete_setup_and_mutated_root_findings() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("sample.feature"),
        "Feature: aliases\n Scenario: one\n  Given first\n",
    )
    .unwrap();
    let mut prefix = "import * as root from 'jest-cucumber'; const a0=root;".to_owned();
    for n in 1..40 {
        prefix.push_str(&format!("const a{n}=a{};", n - 1));
    }
    for (effect, count) in [
        ("a39.defineFeature(feature, setup);", 2),
        ("a39.defineFeature=unknown;", 0),
    ] {
        std::fs::write(directory.path().join("steps.ts"), format!("{prefix} {effect} root.defineFeature(feature,t=>t('one',({{given}})=>{{given('first',()=>work());given('second',()=>work());}}));")).unwrap();
        let (output, value) = strict_report(directory.path());
        assert_eq!(output.status.code(), Some(2), "{effect}");
        assert_eq!(value["corpus"]["incomplete"], true);
        assert_eq!(value["corpus"]["definitionsExtracted"], count);
    }
}
