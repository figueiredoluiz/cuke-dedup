use super::pairs::{analyze_definition_pairs, definition_pair_candidates};
use super::similarity::{
    handler_similarity, is_near_matcher, matcher_similarity, ordered_common_subsequence_len,
};
use super::*;
use crate::config::{ConfigOverrides, SuppressionConfig};
use crate::discovery::{SourceFile, SourceLanguage};
use crate::gherkin;
use crate::model::{DuplicationThreshold, Rule};
use crate::typescript;
use std::path::{Path, PathBuf};

fn definitions(source: &str) -> Vec<StepDefinition> {
    typescript::extract(
        source,
        &SourceFile {
            path: PathBuf::from("steps.ts"),
            language: SourceLanguage::TypeScript,
        },
    )
    .unwrap()
}

fn config() -> (tempfile::TempDir, Config) {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    (directory, config)
}

#[test]
fn public_analysis_rejects_mutated_configs_above_hard_safety_ceilings() {
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 2_000_001;
    let error = analyze(Vec::new(), Vec::new(), &config).unwrap_err();
    assert!(error
        .to_string()
        .contains("maxCandidateComparisons must not exceed the hard safety limit of 2000000"));

    config.max_candidate_comparisons = 2_000_000;
    config.max_structural_class_comparisons = 250_001;
    let error = analyze_with_diagnostics(Vec::new(), Vec::new(), &config).unwrap_err();
    assert!(error
        .to_string()
        .contains("maxStructuralClassComparisons must not exceed the hard safety limit of 250000"));
}

#[test]
fn equivalent_assertion_factory_syntax_preserves_conflicting_values() {
    for (import, factory, trusted) in [
        ("import {default as check} from 'expect';", "check", true),
        ("const check = require('expect');", "(check)", true),
        (
            "import {expect as check} from '@playwright/test';",
            "(check as any)",
            true,
        ),
        (
            "import {expect as check} from '@playwright/test';",
            "check!",
            true,
        ),
        (
            "import {expect as check} from '@playwright/test';",
            "(check satisfies Function)",
            true,
        ),
        (
            "import * as api from '@playwright/test';",
            "((api as PW).expect)",
            true,
        ),
        (
            "import * as api from '@playwright/test';",
            "api['expect']",
            true,
        ),
        (
            "const api = require('@playwright/test');",
            "(api as PW)[\"expect\"]",
            true,
        ),
        (
            "import * as api from '@playwright/test';",
            r"api['ex\u0070ect']",
            true,
        ),
        (
            "import * as api from '@playwright/test';",
            "api[key]",
            false,
        ),
        ("import * as api from 'unrelated';", "api['expect']", false),
        (
            "const {expect: check = fallback} = require('@playwright/test');",
            "check",
            false,
        ),
        (
            "const {expect = fallback} = require('@playwright/test');",
            "expect",
            false,
        ),
        (
            "import {default as check} from '@playwright/test';",
            "check",
            false,
        ),
        (
            "import {default as check} from 'unrelated';",
            "(check)",
            false,
        ),
        (
            "import type {default as check} from 'expect';",
            "check",
            false,
        ),
    ] {
        let defs = definitions(&format!("{import} Then('parcel is ready', ({{state}}) => {factory}(state).toBe('ready')); Then('parcel is idle', ({{state}}) => {factory}(state).toBe('idle'));"));
        assert_eq!(defs.len(), 2, "{import} {factory}");
        assert_eq!(
            defs[0]
                .handler
                .behavior_signature
                .iter()
                .any(|e| e.starts_with("assert:")),
            trusted,
            "{import} {factory}"
        );
        if trusted {
            assert_ne!(
                defs[0].handler.behavior_signature,
                defs[1].handler.behavior_signature
            );
        }
    }
}

#[test]
fn future_local_declarations_do_not_expose_outer_assertion_values() {
    for (body, comparable) in [
        (
            "expect(state).toBe(expected); var expected = 'ready';",
            true,
        ),
        ("var expected; expect(state).toBe(expected);", true),
        (
            "var expected = external; expect(state).toBe(expected);",
            false,
        ),
        (
            "{ expect(state).toBe(expected); let expected = 'ready'; }",
            false,
        ),
        (
            "const expected = 'ready'; { expect(state).toBe(expected); class expected {} }",
            false,
        ),
        (
            "const expected = 'ready'; { expect(state).toBe(expected); function expected() {} }",
            false,
        ),
        (
            "const expected = 'ready'; { expect(state).toBe(expected); enum expected { value } }",
            false,
        ),
        (
            "const expected = 'ready'; try {} catch (expected) { expect(state).toBe(expected); }",
            false,
        ),
        (
            "const expected = 'ready'; { type expected = string; expect(state).toBe(expected); }",
            true,
        ),
    ] {
        let defs = definitions(&format!(
            "Then('state', ({{state, expected}}) => {{ {body} }});"
        ));
        assert_eq!(defs[0].handler.comparable, comparable, "{body}");
    }
    for (body, comparable) in [
        ("const expected = 'ready'; { expect(state).toBe(expected); const expected = 'idle'; }", false),
        ("const expected = 'ready'; { const copy = expected; const expected = 'idle'; expect(state).toBe(copy); }", false),
        ("const expected = 'ready'; { expect(state).toBe(expected); let expected; }", false),
        ("const expected = 'ready'; { expect(state).toBe(expected); const {expected} = external; }", false),
        ("const expected = 'ready'; { const [expected] = external; expect(state).toBe(expected); }", false),
        ("const {x} = (() => { const expected = 'ready'; expect(state).toBe(expected); return {}; })();", true),
        ("const expected = 'ready'; { const expected = 'idle'; expect(state).toBe(expected); }", true),
        ("const expected = 'ready'; { const copy = expected; expect(state).toBe(copy); }", true),
        ("{ expect(state).toBe(expected); const expected = 'idle'; }", false),
    ] {
        let defs = definitions(&format!("Then('parcel is ready', ({{state}}) => {{ {body} }});"));
        assert_eq!(defs[0].handler.comparable, comparable, "{body}");
    }
    for callback in ["function expected()", "function* expected()"] {
        let defs = definitions(&format!(
            "Then('state', ({{state}}) => {{ const expected = 'ready'; register({callback} {{ expect(state).toBe(expected); }}); }});"
        ));
        assert!(!defs[0].handler.comparable, "{callback}");
    }
    let defs = definitions("Then('state', ({state, expected}) => { const Box = class expected { static check = expect(state).toBe(expected); }; use(Box); });");
    assert_eq!(defs.len(), 1);
    assert!(!defs[0].handler.comparable);
}

#[test]
fn nested_assertion_builders_validate_their_origin_and_arguments() {
    for (prefix, expected, comparable) in [
        ("", "expect.objectContaining({role: 'admin'})", true),
        ("", "expect.not.objectContaining({role: 'admin'})", true),
        (
            "",
            "expect.arrayContaining([expect.stringContaining('admin')])",
            true,
        ),
        ("", "expect.objectContaining({role: external})", false),
        ("", "expect.unknownBuilder({role: 'admin'})", false),
        (
            "import {expect as check} from '@playwright/test';",
            "check.objectContaining({role: 'admin'})",
            true,
        ),
        (
            "import {expect as check} from 'unrelated';",
            "check.objectContaining({role: 'admin'})",
            false,
        ),
        (
            "const check = custom;",
            "check.objectContaining({role: 'admin'})",
            false,
        ),
    ] {
        let defs = definitions(&format!("{prefix} Then('parcel is ready', ({{state}}) => expect(state).toEqual({expected})); Then('shipment readiness confirmed', ({{state}}) => expect(state).toEqual({expected}));"));
        assert_eq!(
            defs[0].handler.comparable, comparable,
            "{prefix} {expected}"
        );
        let (_dir, cfg) = config();
        let result = analyze(defs, Vec::new(), &cfg).unwrap();
        assert_eq!(
            result
                .findings
                .iter()
                .any(|f| f.rule == Rule::DuplicateHandler),
            comparable,
            "{prefix} {expected}"
        );
    }
    let defs = definitions("Then('parcel is ready', ({state}) => expect(state).toEqual(expect.objectContaining({role: 'admin'}))); Then('parcel is now ready', ({state}) => expect(state).toEqual(expect.objectContaining({role: 'guest'})));");
    assert_ne!(
        defs[0].handler.behavior_signature,
        defs[1].handler.behavior_signature
    );

    let defs = definitions("import {expect as check} from '@playwright/test'; Then('parcel is ready', ({state}) => expect(state).toEqual(expect.objectContaining({role: 'admin'}))); Then('shipment readiness confirmed', ({state}) => check(state).toEqual((check as any).objectContaining({role: 'admin'})));");
    assert_eq!(
        defs[0].handler.behavior_signature,
        defs[1].handler.behavior_signature
    );
    let defs = definitions("Then('parcel is ready', ({state}) => expect(state).toEqual(expect.objectContaining({role: 'admin'}))); Then('shipment readiness confirmed', ({state}) => expect(state).toEqual(expect.not.objectContaining({role: 'admin'})));");
    assert_ne!(
        defs[0].handler.behavior_signature,
        defs[1].handler.behavior_signature
    );
}

#[test]
fn computed_nested_matchers_preserve_dotted_semantics() {
    for prefix in [
        "import * as api from '@playwright/test';",
        "const api = require('@playwright/test');",
    ] {
        for (dotted, computed) in [
            ("expect.objectContaining", "expect['objectContaining']"),
            (
                "expect.not.objectContaining",
                "expect['not']['objectContaining']",
            ),
            (
                "api.expect.objectContaining",
                "api['expect']['objectContaining']",
            ),
            (
                "api.expect.not.objectContaining",
                "(api['expect'] as any)['not']['objectContaining']",
            ),
            (
                "api.expect.objectContaining",
                r"api['expect']['object\u0043ontaining']",
            ),
        ] {
            let defs = definitions(&format!("{prefix} Then('parcel is ready', ({{state}}) => expect(state).toEqual({dotted}({{role: 'admin'}}))); Then('parcel is now ready', ({{state}}) => expect(state).toEqual({computed}({{role: 'admin'}})));"));
            assert!(defs.iter().all(|d| d.handler.comparable), "{computed}");
            assert_eq!(
                defs[0].handler.behavior_signature, defs[1].handler.behavior_signature,
                "{computed}"
            );
            let (_dir, cfg) = config();
            assert!(
                analyze(defs, Vec::new(), &cfg)
                    .unwrap()
                    .findings
                    .iter()
                    .any(|f| f.rule == Rule::NearDuplicateStep),
                "{computed}"
            );
        }
        for builder in [
            "api['expect'][key]",
            "api['expect']['unknownBuilder']",
            "local['objectContaining']",
        ] {
            let defs = definitions(&format!("{prefix} const local = custom; Then('state', ({{state}}) => expect(state).toEqual({builder}({{role: 'admin'}})));"));
            assert!(!defs[0].handler.comparable, "{builder}");
        }
    }
}

#[test]
fn unresolved_subject_shadows_do_not_collapse_to_empty_values() {
    for declarations in [
        "class First {} class Second {}",
        "function First() {} function Second() {}",
        "enum First { Ready } enum Second { Idle }",
    ] {
        let defs = definitions(&format!(
            "Then('subject', () => {{ {declarations} expect(First).toBe('ready'); expect(Second).toBe('ready'); }});"
        ));
        let events: Vec<_> = defs[0]
            .handler
            .behavior_signature
            .iter()
            .filter(|event| event.starts_with("assert:"))
            .collect();
        assert_eq!(events.len(), 2, "{declarations}");
        assert_ne!(events[0], events[1], "{declarations}");
    }
}

#[test]
fn reassigned_asymmetric_matchers_lose_trust_without_changing_outer_factory() {
    for (prefix, factory) in [
        ("", "expect"),
        (
            "import { expect as check } from '@playwright/test';",
            "check",
        ),
        ("const api = require('@playwright/test');", "api.expect"),
        ("const api = require('@playwright/test');", "api['expect']"),
    ] {
        for (mutation, trusted) in [
            ("FACTORY.objectContaining = replacement;", false),
            ("FACTORY.not = replacement;", false),
            ("FACTORY.not.objectContaining = replacement;", false),
            ("FACTORY['not'].objectContaining = replacement;", false),
            ("FACTORY['objectContaining'] = replacement;", false),
            ("delete FACTORY.objectContaining;", false),
            (
                "const alias = FACTORY; alias.objectContaining = replacement;",
                false,
            ),
            ("FACTORY.unrelated = replacement;", true),
            (
                "function local(expect) { expect.objectContaining = replacement; }",
                true,
            ),
            ("", true),
        ] {
            let mutation = mutation.replace("FACTORY", factory);
            let defs = definitions(&format!(
                "{prefix} {mutation} Then('nested', ({{state}}) => {factory}(state).toEqual({factory}.objectContaining({{role: 'admin'}}))); Then('direct', ({{state}}) => {factory}(state).toBe('ready'));"
            ));
            assert_eq!(defs.len(), 2, "{factory} {mutation}");
            assert_eq!(defs[0].handler.comparable, trusted, "{factory} {mutation}");
            assert!(defs[1].handler.comparable, "{factory} {mutation}");
            assert!(
                defs[1]
                    .handler
                    .behavior_signature
                    .iter()
                    .any(|event| event.starts_with("assert:")),
                "{factory} {mutation}"
            );
        }
    }
}

#[test]
fn namespace_matcher_mutations_follow_computed_and_destructured_aliases() {
    for prefix in [
        "import * as api from '@playwright/test';",
        "const api = require('@playwright/test');",
    ] {
        for (alias, trusted) in [
            ("const check = api['expect'];", false),
            ("const check = (api as any)[\"expect\"];", false),
            ("const { expect: check } = api;", false),
            ("const { expect: check = fallback } = api;", false),
            ("let check; ({ expect: check = fallback } = api);", false),
            (
                "const {expect = fallback} = api; const check = expect;",
                false,
            ),
            ("const { 'expect': check } = api;", false),
            ("const { ['expect']: check } = api;", false),
            (r"const { 'ex\u0070ect': check } = api;", false),
            ("let check; ({ expect: check } = api);", false),
            ("const check = api.other;", true),
            ("const check = make(api.expect);", true),
            ("const check = (make(api['expect']) as any);", true),
            ("const check = { factory: api.expect };", true),
            ("const check = api[key];", false),
            ("const { other: check } = api;", true),
        ] {
            let defs = definitions(&format!("{prefix} {alias} check.objectContaining = replacement; Then('state', ({{state}}) => api.expect(state).toEqual(api.expect.objectContaining({{role: 'admin'}})));"));
            assert_eq!(defs[0].handler.comparable, trusted, "{prefix} {alias}");
        }
        let defs = definitions(&format!("{prefix} function local(api) {{ const {{ expect: check }} = api; check.objectContaining = replacement; }} Then('state', ({{state}}) => api.expect(state).toEqual(api.expect.objectContaining({{role: 'admin'}})));"));
        assert!(defs[0].handler.comparable, "{prefix}");
        let defs = definitions(&format!("{prefix} function local(api) {{ const {{expect: check = fallback}} = api; check.objectContaining = replacement; }} Then('state', ({{state}}) => api['expect'](state).toEqual(api.expect.objectContaining({{role: 'admin'}})));"));
        assert!(defs[0].handler.comparable, "{prefix}");
    }
}

#[test]
fn computed_destructuring_distinguishes_trust_from_possible_mutation() {
    let (_dir, cfg) = config();
    for (key, grants_trust, may_mutate) in [
        ("expect", true, true),
        ("'expect'", true, true),
        ("['expect']", true, true),
        (r"['ex\u0070ect']", true, true),
        ("[key]", false, true),
        ("[expect]", false, true),
        ("other", false, false),
        ("['other']", false, false),
    ] {
        for mutation in [false, true] {
            for prefix in [
                "import * as api from '@playwright/test';",
                "const api = require('@playwright/test');",
            ] {
                let setup = if mutation {
                    format!("{prefix} const {{{key}: check}} = api; check.objectContaining = replacement;")
                } else {
                    format!("const {{{key}: check}} = require('@playwright/test');")
                };
                let builder = if mutation { "api.expect" } else { "check" };
                let expected = if mutation { !may_mutate } else { grants_trust };
                let defs = definitions(&format!("{setup} Then('parcel is ready', ({{state}}) => expect(state).toEqual({builder}.objectContaining({{role:'admin'}}))); Then('parcel is now ready', ({{state}}) => expect(state).toEqual({builder}.objectContaining({{role:'admin'}})));"));
                assert!(
                    defs.iter().all(|d| d.handler.comparable == expected),
                    "{setup}"
                );
                let findings = analyze(defs, Vec::new(), &cfg).unwrap().findings;
                assert_eq!(
                    findings.iter().any(|f| matches!(
                        f.rule,
                        Rule::DuplicateHandler
                            | Rule::NearDuplicateStep
                            | Rule::ParameterizationCandidate
                    )),
                    expected,
                    "{setup}"
                );
            }
        }
    }
}

#[test]
fn parameterized_inline_calls_keep_execution_context_without_handler_findings() {
    let (_dir, cfg) = config();
    for function in [
        "(x) => save(x)",
        "function(x) { save(x); }",
        "function*(x) { yield x; }",
        "async function*(x) { yield x; }",
    ] {
        for deferred in [false, true] {
            let call = format!("({function})(prepare(), finish());");
            let body = if deferred {
                format!("register(() => {{ {call} }});")
            } else {
                call
            };
            let defs = definitions(&format!("Then('parcel is ready', () => {{ {body} }}); Then('parcel is now ready', () => {{ {body} }});"));
            let marker = if deferred {
                "deferred-assert:unresolved"
            } else {
                "assert:unresolved"
            };
            assert!(
                defs.iter().all(|d| !d.handler.comparable
                    && d.handler.behavior_signature.iter().any(|e| e == marker)),
                "{body}"
            );
            let mut expected_events = if deferred {
                vec!["call:register"]
            } else {
                Vec::new()
            };
            expected_events.extend([marker, "call:prepare", "call:finish"]);
            assert_eq!(
                defs[0].handler.behavior_signature, expected_events,
                "{body}"
            );
            let findings = analyze(defs, Vec::new(), &cfg).unwrap().findings;
            assert!(
                !findings.iter().any(|f| matches!(
                    f.rule,
                    Rule::DuplicateHandler
                        | Rule::NearDuplicateStep
                        | Rule::ParameterizationCandidate
                )),
                "{body}"
            );
        }
    }
}

#[test]
fn deferred_assertion_disagreement_cannot_be_outweighed_by_shared_calls() {
    let (_dir, cfg) = config();
    let mut limited_cfg = cfg.clone();
    limited_cfg.max_candidate_comparisons = 1;
    for callback in ["() =>", "function()", "async () =>"] {
        for (right_wrapper, shared_calls) in [
            ("register", ""),
            ("otherWrapper", ""),
            ("register", "load(); save(); render();"),
        ] {
            for (right_assertion, expected) in [
                ("expect(state).toBe('ready');", true),
                ("(expect as any)(state).toBe('ready');", true),
                ("expect(state).toBe('idle');", false),
                ("expect(state).not.toBe('ready');", false),
                ("expect(state).toBe(external);", false),
                (
                    "expect(state).toBe('ready'); expect(state).toBe('idle');",
                    false,
                ),
            ] {
                let defs = definitions(&format!("Then('parcel is ready', ({{state}}) => register({callback} {{ {shared_calls} expect(state).toBe('ready'); }})); Then('parcel is now ready', ({{state}}) => {right_wrapper}({callback} {{ {shared_calls} {right_assertion} }}));"));
                for pair in [defs.clone(), defs.into_iter().rev().collect()] {
                    if !expected {
                        assert!(
                            definition_pair_candidates(&pair, &limited_cfg).is_empty(),
                            "{right_assertion}"
                        );
                    }
                    let findings = analyze(pair, Vec::new(), &cfg).unwrap().findings;
                    assert_eq!(
                        findings.iter().any(|f| matches!(
                            f.rule,
                            Rule::DuplicateHandler
                                | Rule::NearDuplicateStep
                                | Rule::ParameterizationCandidate
                        )),
                        expected,
                        "{callback} {shared_calls} {right_assertion}"
                    );
                }
            }
        }
    }
}

#[test]
fn repeated_commonjs_loads_share_only_guarded_mutation_provenance() {
    let (_dir, cfg) = config();
    for (binding, trusted) in [
        ("const {[key]: check} = require('@playwright/test'); check.objectContaining = replacement;", false),
        ("const {expect: check} = require('@playwright/test'); delete check.objectContaining;", false),
        ("let check; ({['expect']: check} = require('@playwright/test')); check.objectContaining = replacement;", false),
        ("const {expect: check = fallback} = require('@playwright/test'); check.objectContaining = replacement;", false),
        ("const other = require('@playwright/test'); other.expect.objectContaining = replacement;", false),
        ("const check = require('@playwright/test').expect; check.objectContaining = replacement;", false),
        ("function mutate() { const {expect: check} = require('@playwright/test'); check.objectContaining = replacement; }", false),
        ("const {[key]: check} = require('@jest/globals'); check.objectContaining = replacement;", true),
        ("function mutate(require) { const {expect: check} = require('@playwright/test'); check.objectContaining = replacement; }", true),
        ("{ const require = localLoader; const {expect: check} = require('@playwright/test'); check.objectContaining = replacement; }", true),
        ("const {[key]: check} = require('@playwright/test');", true),
        ("let {expect: check} = require('@playwright/test'); check = replacement;", true),
        ("const {other: check} = require('@playwright/test'); check.objectContaining = replacement;", true),
    ] {
        for before in [false, true] {
            let namespace = "const api = require('@playwright/test');";
            let setup = if before { format!("{binding} {namespace}") } else { format!("{namespace} {binding}") };
            let defs = definitions(&format!("{setup} Then('parcel is ready', ({{state}}) => api.expect(state).toEqual(api.expect.objectContaining({{role:'admin'}}))); Then('shipment readiness confirmed', ({{state}}) => api.expect(state).toEqual(api.expect.objectContaining({{role:'admin'}})));") );
            assert!(defs.iter().all(|d| d.handler.comparable == trusted), "{setup}");
            let result = analyze(defs, Vec::new(), &cfg).unwrap();
            assert_eq!(result.findings.iter().any(|f| matches!(f.rule, Rule::DuplicateHandler | Rule::NearDuplicateStep | Rule::ParameterizationCandidate)), trusted, "{setup}");
        }
    }
    // A module-level custom loader must not gain assertion provenance or contaminate imports.
    let defs = definitions("import * as api from '@playwright/test'; const require = loader; const {[key]: check} = require('@playwright/test'); check.objectContaining = replacement; Then('ready', ({state}) => api.expect(state).toEqual(api.expect.objectContaining({role:'admin'})));");
    assert!(defs[0].handler.comparable);
    let defs = definitions("const {[key]: check} = require('@playwright/test'); Then('ready', ({state}) => check(state).toBe('ready'));");
    assert!(defs[0]
        .handler
        .behavior_signature
        .iter()
        .all(|e| !e.starts_with("assert:")));
}

#[test]
fn destructured_negation_mutations_do_not_establish_handler_equivalence() {
    let (_dir, cfg) = config();
    for factory in ["expect", "api.expect", "api['expect']"] {
        for (alias, trusted) in [
            ("const {not: negated} = FACTORY; negated.objectContaining = replacement;", false),
            ("const {'not': negated} = FACTORY; negated.objectContaining = replacement;", false),
            ("const {['not']: negated} = FACTORY; negated.objectContaining = replacement;", false),
            (r"const {['n\u006ft']: negated} = FACTORY; delete negated.objectContaining;", false),
            ("const {not: negated = fallback} = FACTORY; negated.objectContaining = replacement;", false),
            ("const {not} = FACTORY; not.objectContaining = replacement;", false),
            ("const {not = fallback} = FACTORY; not.objectContaining = replacement;", false),
            ("let negated; ({not: negated} = FACTORY); negated.objectContaining = replacement;", false),
            ("const {other: negated} = FACTORY; negated.objectContaining = replacement;", true),
            ("function local(FACTORY) { const {not} = FACTORY; not.objectContaining = replacement; }", true),
        ] {
            // The shadowing control applies to a lexical identifier, not a namespace expression.
            if alias.starts_with("function") && factory != "expect" { continue; }
            let alias = alias.replace("FACTORY", factory);
            let source = format!("import {{expect}} from '@playwright/test'; import * as api from '@playwright/test'; {alias} Then('parcel is ready', ({{state}}) => {factory}(state).toEqual({factory}.not.objectContaining({{role:'admin'}}))); Then('shipment readiness confirmed', ({{state}}) => {factory}(state).toEqual({factory}.not.objectContaining({{role:'admin'}})));");
            let defs = definitions(&source);
            assert!(defs.iter().all(|d| d.handler.comparable == trusted), "{source}");
            let result = analyze(defs, Vec::new(), &cfg).unwrap();
            assert_eq!(result.findings.iter().any(|f| matches!(f.rule, Rule::DuplicateHandler | Rule::NearDuplicateStep | Rule::ParameterizationCandidate)), trusted, "{source}");
        }
    }
    let defs = definitions("const {not: check} = require('@playwright/test'); Then('step', ({state}) => check(state).toBe('ready'));");
    assert!(defs[0]
        .handler
        .behavior_signature
        .iter()
        .all(|e| !e.starts_with("assert:")));
}

#[test]
fn direct_generator_calls_do_not_contribute_suspended_body_events() {
    for generator in ["function*", "async function*"] {
        for wrapped in [
            format!("(<any>{generator} () {{ expect(state).toBe('ready'); }})"),
            format!("(({generator} <T>() {{ expect(state).toBe('ready'); }})<string>)"),
        ] {
            let defs = definitions(&format!(
                "Then('state', ({{state}}) => {{ {wrapped}(); }});"
            ));
            assert!(defs[0].handler.behavior_signature.is_empty(), "{wrapped}");
        }
        for body in ["expect(state).toBe('ready');", "save(state); yield state;"] {
            let defs = definitions(&format!(
                "Then('state', ({{state}}) => {{ ({generator} () {{ {body} }})(); }});"
            ));
            assert!(
                defs[0].handler.behavior_signature.is_empty(),
                "{generator} {body}"
            );
        }
        let defs = definitions(&format!("Then('state', () => {{ ({generator} (value = initialize()) {{ save(value); }})(prepare()); }});"));
        assert_eq!(
            defs[0].handler.behavior_signature,
            ["assert:unresolved", "call:prepare"]
        );
        assert!(!defs[0].handler.comparable);
        let defs = definitions(&format!("Then('state', () => register(() => {{ ({generator} (value = initialize()) {{ save(value); }})(prepare()); }}));"));
        assert_eq!(
            defs[0].handler.behavior_signature,
            [
                "call:register",
                "deferred-assert:unresolved",
                "call:prepare"
            ]
        );
        assert!(!defs[0].handler.comparable);
        let defs = definitions(&format!("Then('state', ({{state}}) => register({generator} () {{ expect(state).toBe('ready'); }}));"));
        assert!(
            defs[0]
                .handler
                .behavior_signature
                .iter()
                .any(|event| event.starts_with("deferred-assert:")),
            "{generator}"
        );
    }
}

#[test]
fn reports_exact_normalized_and_duplicate_handlers_even_when_unused() {
    let definitions = definitions(
        r#"
Given('a user {string}', async (name) => { await save(name); });
Given('a user {string}', async (name) => { await other(name); });
Given('a  user {string}', async (value) => { await third(value); });
Then('the user exists', async (name) => { await save(name); });
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let rules: Vec<_> = result.findings.iter().map(|finding| finding.rule).collect();
    assert!(rules.contains(&Rule::DuplicateMatcher));
    assert!(rules.contains(&Rule::NormalizedMatcher));
    assert!(rules.contains(&Rule::DuplicateHandler));
}

#[test]
fn exact_handlers_require_compatible_assertion_provenance_across_files() {
    let (_directory, config) = config();
    let mut extracted = Vec::new();
    for (index, module) in [
        "@playwright/test",
        "unrelated-assertions",
        "@playwright/test",
    ]
    .into_iter()
    .enumerate()
    {
        let source = format!("import {{ expect }} from '{module}'; Then('the current state is verified {index}', () => expect(state).toBe('ready'));");
        extracted.extend(
            typescript::extract(
                &source,
                &SourceFile {
                    path: PathBuf::from(format!("steps-{index}.ts")),
                    language: SourceLanguage::TypeScript,
                },
            )
            .unwrap(),
        );
    }
    for definitions in [extracted.clone(), extracted.into_iter().rev().collect()] {
        let result = analyze(definitions, Vec::new(), &config).unwrap();
        let findings: Vec<_> = result
            .findings
            .iter()
            .filter(|f| matches!(f.rule, Rule::DuplicateHandler | Rule::NearDuplicateStep))
            .collect();
        assert_eq!(
            findings
                .iter()
                .filter(|f| f.rule == Rule::DuplicateHandler)
                .count(),
            1
        );
        for finding in findings {
            assert_ne!(finding.primary.path, Path::new("steps-1.ts"));
            assert!(finding
                .related
                .iter()
                .all(|location| location.path != Path::new("steps-1.ts")));
        }
    }
}

#[test]
fn unresolved_external_assertion_values_cannot_establish_handler_equivalence() {
    let (_directory, config) = config();
    for (prefix, body, expected_duplicates) in [
        ("function getExpected() { return VALUE; }", "() => { const expected = getExpected(); expect(state).toBe(expected); }", 0),
        ("const external = VALUE;", "() => { const expected = external; expect(state).toBe(expected); }", 0),
        ("const expected = VALUE;", "() => { { const expected = 'local'; } expect(state).toBe(expected); }", 0),
        ("function getExpected() { return VALUE; }", "(expected) => { { var expected = getExpected(); } expect(state).toBe(expected); }", 0),
        ("", "() => expect(state).toSatisfy(actual => actual > 0)", 1),
        ("const external = VALUE;", "() => expect(state).toSatisfy(actual => actual === external)", 0),
        ("", "() => { const expected = 'ready'; const alias = expected; expect(state).toBe(alias); }", 1),
        (
            "const expected = VALUE;",
            "() => expect(state).toBe(expected)",
            0,
        ),
        (
            "",
            "() => { const expected = 'ready'; expect(state).toBe(expected); }",
            1,
        ),
        ("", "(expected) => expect(state).toBe(expected)", 1),
    ] {
        let mut extracted = Vec::new();
        for (index, value) in ["'ready'", "'idle'"].into_iter().enumerate() {
            let source = format!(
                "{} Then('the current state is verified {index}', {body});",
                prefix.replace("VALUE", value)
            );
            extracted.extend(
                typescript::extract(
                    &source,
                    &SourceFile {
                        path: PathBuf::from(format!("steps-{index}.ts")),
                        language: SourceLanguage::TypeScript,
                    },
                )
                .unwrap(),
            );
        }
        let result = analyze(extracted.clone(), Vec::new(), &config).unwrap();
        assert_eq!(
            result
                .findings
                .iter()
                .filter(|f| f.rule == Rule::DuplicateHandler)
                .count(),
            expected_duplicates,
            "{prefix} {body}"
        );
        if expected_duplicates == 0 {
            assert!(!result
                .findings
                .iter()
                .any(|f| f.rule == Rule::NearDuplicateStep));
        }
        extracted[1].matcher = extracted[0].matcher.clone();
        extracted[1].normalized_matcher = extracted[0].normalized_matcher.clone();
        assert!(analyze(extracted, Vec::new(), &config)
            .unwrap()
            .findings
            .iter()
            .any(|f| f.rule == Rule::DuplicateMatcher));
    }
}

#[test]
fn inline_and_local_assertion_values_share_behavior_without_losing_value_precision() {
    let (_directory, config) = config();
    for (declaration, inline, local, expected_match) in [
        ("const expected = 'ready';", "'ready'", "expected", true),
        ("const expected = 'idle';", "'ready'", "expected", false),
        (
            "const first = 'ready'; const expected = first;",
            "'ready'",
            "expected",
            true,
        ),
        ("const expected = 12;", "12", "expected", true),
        ("const expected = true;", "true", "expected", true),
        ("const expected = null;", "null", "expected", true),
        (
            "const expected = 'ready';",
            "{ status: 'ready' }",
            "{ status: expected }",
            true,
        ),
        (
            "const expected = 'ready';",
            "['ready', 'idle']",
            "['idle', expected]",
            false,
        ),
        (
            "let expected = 'ready'; expected = 'idle';",
            "'ready'",
            "expected",
            false,
        ),
        ("const expected = external;", "'ready'", "expected", false),
        (
            "{ const expected = 'ready'; }",
            "'ready'",
            "expected",
            false,
        ),
    ] {
        let source = format!("Then('the parcel status is verified', ({{ state }}) => {{ expect(state).toEqual({inline}); }}); Then('the parcel status is now verified', ({{ state }}) => {{ {declaration} expect(state).toEqual({local}); }});");
        let extracted = definitions(&source);
        assert_eq!(extracted.len(), 2);
        if expected_match {
            assert_eq!(
                extracted[0].handler.behavior_signature, extracted[1].handler.behavior_signature,
                "{source}"
            );
        }
        for definitions in [extracted.clone(), extracted.into_iter().rev().collect()] {
            let result = analyze(definitions, Vec::new(), &config).unwrap();
            assert_eq!(
                result
                    .findings
                    .iter()
                    .any(|f| f.rule == Rule::NearDuplicateStep),
                expected_match,
                "{source}"
            );
        }
    }
}

#[test]
fn assertion_trust_requires_real_facades_and_unmodified_namespace_factories() {
    let (_directory, config) = config();
    let valid = "import {Then} from '@cucumber/cucumber'; import * as api from '@playwright/test';";
    let cjs =
        "const {Then} = require('@cucumber/cucumber'); const api = require('@playwright/test');";
    for (prefix, mutation, trusted) in [
        (valid, "const other = api; delete other.expect;", false),
        (cjs, "delete api['expect'];", false),
        (valid, "const other = api; delete other.other;", true),
        (
            valid,
            "function local(api) { const other = api; delete other.expect; }",
            true,
        ),
        (
            valid,
            "const other = api; other.expect = replacement;",
            false,
        ),
        (cjs, "const other = api; other.expect = replacement;", false),
        (
            valid,
            "const other = (api as PW); const last = other; [last.expect] = replacements;",
            false,
        ),
        (valid, "let other; other = api; other['expect']++;", false),
        (
            valid,
            "function mutate() { const other = api; other.expect = replacement; }",
            false,
        ),
        (
            valid,
            "function mutate(other = api) { other.expect = replacement; }",
            false,
        ),
        (
            valid,
            "function local(api, other = api) { other.expect = replacement; }",
            true,
        ),
        (
            valid,
            "let other; other ||= api; other.expect = replacement;",
            false,
        ),
        (
            valid,
            "let other; other &&= api; other.expect = replacement;",
            false,
        ),
        (
            valid,
            "let other; other ??= api; other.expect = replacement;",
            false,
        ),
        (
            valid,
            "let other = ''; other += api; other.expect = replacement;",
            true,
        ),
        (
            valid,
            "const other = api; function mutate() { other.expect = replacement; }",
            false,
        ),
        (
            valid,
            "const other = api; function local(other) { other.expect = replacement; }",
            true,
        ),
        (
            valid,
            "function local(api) { const other = api; other.expect = replacement; }",
            true,
        ),
        (
            valid,
            "const other = api; { const other = local; other.expect = replacement; }",
            true,
        ),
        (
            valid,
            "let other = api; other = local; other.expect = replacement;",
            false,
        ),
        (
            valid,
            "let other = api; let last = other; other = last; last.expect = replacement;",
            false,
        ),
        (
            valid,
            "const other = api; other.unrelated = replacement;",
            true,
        ),
        (
            valid,
            "type other = unknown; const other = api; other.expect = replacement;",
            false,
        ),
        (valid, "", true),
        (valid, "(api).expect = replacement;", false),
        (valid, "(api as PW).expect = replacement;", false),
        (valid, "api!.expect = replacement;", false),
        (valid, "(api satisfies PW).expect = replacement;", false),
        (valid, "(<PW>api).expect = replacement;", false),
        (valid, "((api as PW)!).expect = replacement;", false),
        (valid, "(api as PW)['expect'] = replacement;", false),
        (valid, "[api!.expect] = replacement;", false),
        (valid, "(api as PW).expect++;", false),
        (valid, "(api as PW).other = replacement;", true),
        (
            valid,
            "function local(api) { (api as PW).expect = replacement; }",
            true,
        ),
        (cjs, "", true),
        (cjs, "api.expect = replacement;", false),
        (cjs, r#"api.ex\u0070ect = replacement;"#, false),
        (cjs, r#"api.ex\u{70}ect = replacement;"#, false),
        (cjs, r#"api["\145xpect"] = replacement;"#, false),
        (cjs, r#"api["\x65xpect"] = replacement;"#, false),
        (cjs, r#"api["\u0065xpect"] = replacement;"#, false),
        (cjs, r#"api["\u{65}xpect"] = replacement;"#, false),
        (cjs, r#"api["\x6fther"] = replacement;"#, true),
        (cjs, "api.expect++;", false),
        (cjs, "for (api.expect of replacements) {}", false),
        (cjs, "for (api.expect in replacements) {}", false),
        (cjs, "api.other++;", true),
        (cjs, "for (api.other of replacements) {}", true),
        (
            cjs,
            "function local(api) { api.expect++; for (api.expect of replacements) {} }",
            true,
        ),
        (valid, "api['expect'] = replacement;", false),
        (valid, "({ factory: api.expect } = replacement);", false),
        (valid, "[api.expect] = replacement;", false),
        (valid, "api.expect ||= replacement;", false),
        (valid, "api[key] = replacement;", false),
        (valid, "api.other = replacement;", true),
        (valid, "api['other'] = replacement;", true),
        (
            valid,
            "function unrelated(api) { api.expect = replacement; }",
            true,
        ),
        (
            valid,
            "function mutate() { api.expect = replacement; }",
            false,
        ),
    ] {
        let mut extracted = Vec::new();
        for (index, header) in [format!("{prefix} {mutation}"), valid.to_owned()]
            .iter()
            .enumerate()
        {
            extracted.extend(typescript::extract(
                &format!("{header} Then('the parcel status is verified {index}', ({{state}}) => api.expect(state).toBe('ready'));"),
                &SourceFile { path: PathBuf::from(format!("steps-{index}.ts")), language: SourceLanguage::TypeScript },
            ).unwrap());
        }
        assert_eq!(
            extracted[0]
                .handler
                .behavior_signature
                .iter()
                .any(|e| e.starts_with("assert:")),
            trusted,
            "{prefix} {mutation}"
        );
        for definitions in [extracted.clone(), extracted.into_iter().rev().collect()] {
            let result = analyze(definitions, Vec::new(), &config).unwrap();
            assert_eq!(
                result
                    .findings
                    .iter()
                    .any(|f| f.rule == Rule::DuplicateHandler),
                trusted,
                "{prefix} {mutation}"
            );
            if !trusted {
                assert!(!result
                    .findings
                    .iter()
                    .any(|f| f.rule == Rule::NearDuplicateStep));
            }
        }
    }
    for mutation in [
        "function mutate(other = api) { other.expect = replacement; }",
        "let other; other ||= api; other.expect = replacement;",
    ] {
        let source = format!(
            "const {{Then}} = require('@cucumber/cucumber'); const api = require('@playwright/test'); {mutation} Then('the parcel status is verified', ({{state}}) => api.expect(state).toBe('ready'));"
        );
        let extracted = typescript::extract(
            &source,
            &SourceFile {
                path: PathBuf::from("steps.js"),
                language: SourceLanguage::JavaScript,
            },
        )
        .unwrap();
        assert!(
            extracted[0]
                .handler
                .behavior_signature
                .iter()
                .all(|event| !event.starts_with("assert:")),
            "{mutation}"
        );
    }
    for write in [
        "require &&= replacement;",
        "require += replacement;",
        "require++;",
        "for (require of replacements) {}",
    ] {
        let source =
            format!("{write} {cjs} Then('state', ({{state}}) => api.expect(state).toBe('ready'));");
        assert!(
            definitions(&source)[0]
                .handler
                .behavior_signature
                .iter()
                .all(|e| !e.starts_with("assert:")),
            "{write}"
        );
        let source = format!("function local(require) {{ {write} }} {cjs} Then('state', ({{state}}) => api.expect(state).toBe('ready'));");
        assert!(
            definitions(&source)[0]
                .handler
                .behavior_signature
                .iter()
                .any(|e| e.starts_with("assert:")),
            "local {write}"
        );
    }
    for module in [
        "@cucumber/cucumber",
        "cucumber",
        "playwright-bdd",
        "@badeball/cypress-cucumber-preprocessor",
        "cypress-cucumber-preprocessor/steps",
    ] {
        let source = format!("import {{Then, expect as check}} from '{module}'; Then('state', ({{state}}) => check(state).toBe('ready'));");
        let extracted = definitions(&source);
        assert_eq!(extracted.len(), 1, "{module}");
        assert!(
            extracted[0]
                .handler
                .behavior_signature
                .iter()
                .all(|e| !e.starts_with("assert:")),
            "{module}"
        );
    }
    let extracted = definitions("import * as expect from '@playwright/test'; expect.expect = replacement; Then('state', ({state}) => expect(state).toBe('ready'));");
    assert!(extracted[0]
        .handler
        .behavior_signature
        .iter()
        .all(|e| !e.starts_with("assert:")));
}

#[test]
fn shorthand_assertions_preserve_property_keys_and_local_values() {
    let (_directory, config) = config();
    for (left, right, expected_duplicate) in [
        (
            "const expected = 'ready';",
            "const expected = 'idle';",
            false,
        ),
        (
            "const expected = 'ready';",
            "const expected = 'ready';",
            true,
        ),
        ("const expected = 'ready';", "const other = 'ready';", false),
        (
            "const expected = external;",
            "const expected = external;",
            false,
        ),
        ("let expected = 'ready';", "let expected = 'idle';", false),
        (
            "{ const expected = 'ready'; }",
            "{ const expected = 'idle'; }",
            false,
        ),
    ] {
        let right_key = if right.contains("other") {
            "other"
        } else {
            "expected"
        };
        let source = format!("Then('the parcel status is verified', ({{ state }}) => {{ {left} expect(state).toEqual({{ expected }}); }}); Then('the parcel status is now verified', ({{ state }}) => {{ {right} expect(state).toEqual({{ {right_key} }}); }});");
        let extracted = definitions(&source);
        for definitions in [extracted.clone(), extracted.into_iter().rev().collect()] {
            let result = analyze(definitions, Vec::new(), &config).unwrap();
            let findings: Vec<_> = result
                .findings
                .iter()
                .filter(|f| {
                    matches!(
                        f.rule,
                        Rule::DuplicateHandler
                            | Rule::NearDuplicateStep
                            | Rule::ParameterizationCandidate
                    )
                })
                .collect();
            assert_eq!(
                findings.len(),
                if expected_duplicate { 2 } else { 0 },
                "{source}: {findings:?}"
            );
            if expected_duplicate {
                assert_eq!(findings[0].rule, Rule::DuplicateHandler);
            }
        }
    }
}

#[test]
fn assertion_precision_covers_member_require_iifes_and_factory_options() {
    let (_directory, config) = config();
    for (prefix, body) in [
        (
            "const check = require('@playwright/test').expect;",
            "() => check(state).toBe(VALUE)",
        ),
        (
            "const check = require('@jest/globals').expect;",
            "() => check(state).toBe(VALUE)",
        ),
        (
            "const check = require('expect').expect;",
            "() => check(state).toBe(VALUE)",
        ),
        ("", "() => { (() => expect(state).toBe(VALUE))(); }"),
        (
            "",
            "() => { (function () { expect(state).toBe(VALUE); })(); }",
        ),
        (
            "",
            "async () => { await (async () => expect(state).toBe(VALUE))(); }",
        ),
        (
            "",
            "() => { ((() => expect(state).toBe(VALUE)) as (() => void))(); }",
        ),
        (
            "",
            "() => expect.poll(() => state, { timeout: VALUE }).toBe('ready')",
        ),
        (
            "",
            "() => expect.poll(() => state, { intervals: [VALUE] }).toBe('ready')",
        ),
    ] {
        for same in [false, true] {
            let source = format!(
                "{prefix} Then('the current state is verified', {}); Then('the current state is verified now', {});",
                body.replace("VALUE", "100"), body.replace("VALUE", if same { "100" } else { "5000" })
            );
            let extracted = definitions(&source);
            assert_eq!(extracted.len(), 2, "{source}");
            assert_eq!(
                extracted[0].handler.behavior_signature == extracted[1].handler.behavior_signature,
                same,
                "{source}"
            );
            let result = analyze(extracted, Vec::new(), &config).unwrap();
            assert_eq!(
                result
                    .findings
                    .iter()
                    .any(|f| f.rule == Rule::DuplicateHandler),
                same,
                "{source}"
            );
            if !same {
                assert!(
                    !result.findings.iter().any(|f| matches!(
                        f.rule,
                        Rule::NearDuplicateStep | Rule::ParameterizationCandidate
                    )),
                    "{source}: {:?}",
                    result.findings
                );
            }
        }
    }
}

#[test]
fn assertion_precision_keeps_untrusted_bindings_and_callbacks_separate() {
    for (prefix, body) in [
        ("const check = require('unrelated').expect;", "() => check(state).toBe('ready')"),
        ("const check = require('@playwright/test').other;", "() => check(state).toBe('ready')"),
        ("const { expect: check } = require('@playwright/test').expect;", "() => check(state).toBe('ready')"),
        ("function require(name) { return custom; } const check = require('@playwright/test').expect;", "() => check(state).toBe('ready')"),
        ("const check = require('@playwright/test').expect;", "(check) => check(state).toBe('ready')"),
        ("", "() => { register(() => expect(state).toBe('ready')); }"),
        ("", "() => { const callback = () => expect(state).toBe('ready'); register(callback); }"),
        ("", "() => { (function* () { expect(state).toBe('ready'); })(); }"),
    ] {
        let source = format!("{prefix} Then('state', {body});");
        let extracted = definitions(&source);
        assert_eq!(extracted.len(), 1, "{source}");
        assert!(extracted[0].handler.behavior_signature.iter().all(|event| !event.starts_with("assert:")), "{source}");
    }
    for options in [
        "external",
        "{ timeout: external }",
        "{ intervals: [external] }",
    ] {
        let extracted = definitions(&format!(
            "Then('state', () => expect.poll(() => state, {options}).toBe('ready'));"
        ));
        assert!(!extracted[0].handler.comparable, "{options}");
    }
    for invocation in [
        "((value) => expect(state).toBe(value))(100)",
        "(value => expect(state).toBe(value))(100)",
        "((value = 100) => expect(state).toBe(value))()",
    ] {
        let extracted = definitions(&format!("Then('state', () => {{ {invocation}; }});"));
        assert!(!extracted[0].handler.comparable, "{invocation}");
    }
    let named = definitions("Then('one', () => { (function first() { expect(state).toBe('ready'); })(); }); Then('two', () => { (function second() { expect(state).toBe('ready'); })(); });");
    assert_eq!(
        named[0].handler.behavior_signature,
        named[1].handler.behavior_signature
    );
    let arguments =
        definitions("Then('state', () => { (() => expect(state).toBe('ready'))(prepare()); });");
    assert_eq!(arguments[0].handler.behavior_signature.len(), 2);
    assert_eq!(arguments[0].handler.behavior_signature[0], "call:prepare");
    assert!(arguments[0].handler.behavior_signature[1].starts_with("assert:"));
}

#[test]
fn nested_argument_callbacks_preserve_behavior_and_scope_boundaries() {
    let (_directory, config) = config();
    for container in [
        "CALLBACK",
        "{ callback: CALLBACK }",
        "[{ nested: [CALLBACK] }]",
        "enabled ? CALLBACK : undefined",
        "enabled && CALLBACK",
        "(undefined, CALLBACK)",
        "({ callback: CALLBACK } as Options)",
    ] {
        for callback in ["() =>", "async () =>", "function ()", "function* ()"] {
            for (left_body, right_body, expected) in [
                (
                    "load(); save(); render();",
                    "load(); save(); render();",
                    true,
                ),
                (
                    "load(); save(); render();",
                    "erase(); reset(); remove();",
                    false,
                ),
                (
                    "expect(state).toBe('ready');",
                    "expect(state).toBe('ready');",
                    true,
                ),
                (
                    "expect(state).toBe('ready');",
                    "expect(state).toBe('idle');",
                    false,
                ),
                (
                    "expect(state).toBe('ready');",
                    "expect(state).toBe(external);",
                    false,
                ),
            ] {
                let argument =
                    |body| container.replace("CALLBACK", &format!("{callback} {{ {body} }}"));
                let defs = definitions(&format!(
                    "Then('parcel is ready', ({{state}}) => register({})); Then('parcel is now ready', ({{state}}) => register({}));",
                    argument(left_body), argument(right_body)
                ));
                assert!(
                    defs[0].handler.behavior_signature.len() > 1,
                    "{container} {callback}"
                );
                assert!(defs.iter().all(|d| d
                    .handler
                    .behavior_signature
                    .iter()
                    .all(|e| !e.starts_with("assert:"))));
                for pair in [defs.clone(), defs.into_iter().rev().collect()] {
                    let findings = analyze(pair, Vec::new(), &config).unwrap().findings;
                    assert_eq!(
                        findings.iter().any(|f| matches!(
                            f.rule,
                            Rule::DuplicateHandler
                                | Rule::NearDuplicateStep
                                | Rule::ParameterizationCandidate
                        )),
                        expected,
                        "{container} {callback} {right_body}"
                    );
                }
            }
        }
        for callback in ["value => save(value)", "function (value) { save(value); }"] {
            let argument = container.replace("CALLBACK", callback);
            let defs = definitions(&format!("Then('parcel is ready', () => register({argument})); Then('parcel is now ready', () => register({argument}));"));
            assert!(
                defs.iter().all(|d| !d.handler.comparable),
                "{container} {callback}"
            );
            let findings = analyze(defs, Vec::new(), &config).unwrap().findings;
            assert!(!findings.iter().any(|f| matches!(
                f.rule,
                Rule::DuplicateHandler | Rule::NearDuplicateStep | Rule::ParameterizationCandidate
            )));
        }
    }
    for body in [
        "register({callback: () => { const unused = () => erase(); load(); }});",
        "register({callback: () => { function unused() { erase(); } load(); }});",
        "register({callback: () => () => erase()});",
        "register(class { method() { erase(); } });",
        "const unused = {callback: () => erase()}; load();",
    ] {
        let defs = definitions(&format!("Then('scope control', () => {{ {body} }});"));
        assert!(
            !defs[0]
                .handler
                .behavior_signature
                .iter()
                .any(|e| e == "call:erase"),
            "{body}"
        );
    }
}

#[test]
fn callback_calls_retain_discriminating_behavior_without_assertion_promotion() {
    let (_directory, config) = config();
    for callback in ["async () =>", "async function ()", "function* ()"] {
        for (left_wrapper, right_wrapper, right_calls, expected) in [
            (
                "withTransaction",
                "withTransaction",
                "loadOrder(); applyShipping(); commit();",
                false,
            ),
            (
                "alpha",
                "beta",
                "loadCart(); applyDiscount(); save();",
                true,
            ),
        ] {
            let defs = definitions(&format!(
                "When('the admin applies the discount rule', () => {{ {left_wrapper}({callback} {{ loadCart(); applyDiscount(); save(); }}); }});\nWhen('the admin applies the shipping rule', () => {{ {right_wrapper}({callback} {{ {right_calls} }}); }});"
            ));
            assert_eq!(defs[0].handler.behavior_signature.len(), 4);
            assert_eq!(defs[1].handler.behavior_signature.len(), 4);
            for pair in [defs.clone(), defs.into_iter().rev().collect()] {
                let result = analyze(pair, Vec::new(), &config).unwrap();
                assert_eq!(
                    result
                        .findings
                        .iter()
                        .any(|f| f.rule == Rule::NearDuplicateStep),
                    expected,
                    "{callback} {left_wrapper} {right_wrapper} {right_calls}"
                );
                assert!(!result
                    .findings
                    .iter()
                    .any(|f| f.rule == Rule::DuplicateHandler));
            }
        }
    }
    for body in [
        "register(() => { (() => expect(state).toBe('ready'))(); });",
        "register(((function () { load(); render(); }) as Callback));",
        "register(() => nested(() => load()));",
    ] {
        let defs = definitions(&format!("Then('callback example', () => {{ {body} }});"));
        assert!(defs[0].handler.behavior_signature.len() > 1, "{body}");
        assert!(
            defs[0]
                .handler
                .behavior_signature
                .iter()
                .all(|e| !e.starts_with("assert:")),
            "{body}"
        );
    }
    let conflicting = definitions(
        "Then('callback expects ready', ({state}) => register(() => expect(state).toBe('ready'))); Then('callback expects idle', ({state}) => register(() => expect(state).toBe('idle')));",
    );
    assert_ne!(
        conflicting[0].handler.behavior_signature,
        conflicting[1].handler.behavior_signature
    );
    assert!(conflicting.iter().all(|definition| definition
        .handler
        .behavior_signature
        .iter()
        .any(|event| event.starts_with("deferred-assert:"))));
    let unresolved = definitions(
        "Then('callback unresolved', ({state}) => register(() => expect(state).toBe(external)));",
    );
    assert!(!unresolved[0].handler.comparable);
    for callback in ["value =>", "function (value)", "function* (value)"] {
        let unresolved = definitions(&format!(
            "Then('parameterized callback', ({{state}}) => register({callback} {{ expect(state).toBe(value); }}, externalValue));"
        ));
        assert!(!unresolved[0].handler.comparable, "{callback}");
        assert!(
            unresolved[0]
                .handler
                .behavior_signature
                .iter()
                .any(|event| event == "deferred-assert:unresolved"),
            "{callback}"
        );
    }
    let unresolved_without_assertion = definitions(
        "Then('parameterized callback', () => register(value => save(value), externalValue));",
    );
    assert!(!unresolved_without_assertion[0].handler.comparable);
    assert!(unresolved_without_assertion[0]
        .handler
        .behavior_signature
        .iter()
        .any(|event| event == "deferred-assert:unresolved"));
}

#[test]
fn candidate_buckets_skip_unrelated_pairs_and_keep_exact_groups() {
    let mut unrelated_source = String::new();
    for index in 0..100 {
        unrelated_source.push_str(&format!(
            "Given('unique step {index}', () => action_{index}());\n"
        ));
    }
    let unrelated = definitions(&unrelated_source);
    let (_directory, config) = config();
    assert!(definition_pair_candidates(&unrelated, &config).is_empty());

    let exact = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());",
    );
    assert_eq!(definition_pair_candidates(&exact, &config).len(), 2);
}

#[test]
fn candidate_limits_preserve_partial_analysis_and_report_skipped_work() {
    let mut source = String::new();
    for index in 0..143 {
        source.push_str(&format!(
            "Given('operation label {index}', () => perform({index}));\n"
        ));
    }
    let class_definitions = definitions(&source);
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 100;
    config.max_structural_class_comparisons = 20;

    let generated = definition_pair_candidates(&class_definitions, &config);
    assert!(generated.census.truncated);
    assert!(generated.len() <= 100);
    assert_eq!(generated.census.truncated_structural_classes, 1);
    assert!(generated.census.skipped_candidate_comparisons > 0);
    assert!(generated.census.candidate_sources["structuralHandler"].skipped > 0);

    let (outcome, census, _) = analyze_for_cli(class_definitions, Vec::new(), &config).unwrap();
    assert!(!outcome.result.findings.is_empty());
    assert_eq!(outcome.incomplete.len(), 1);
    assert!(outcome.incomplete[0].contains("partial findings are available"));
    let mut expected_census = generated.census;
    expected_census.candidate_sources.insert(
        "matcherOverlap".to_owned(),
        CandidateSourceCensus::default(),
    );
    assert_eq!(census, expected_census);
}

#[test]
fn similarity_work_limits_fail_closed_before_quadratic_pair_verification() {
    let mut long_matchers = definitions(
        "Given('left', () => sharedImplementation());\nGiven('right', () => sharedImplementation());",
    );
    // Just past the matrix budget: `matrix_work` charges `l * r + l + r`, so this many characters
    // costs slightly more than the ceiling. Sizing to the boundary rather than far beyond it keeps
    // the computation finite when the budget is removed, so a regression fails these assertions
    // instead of running long enough to look like a hang.
    let over_budget = "a".repeat(1_010);
    long_matchers[0].matcher = format!("{over_budget}b");
    long_matchers[0].normalized_matcher = long_matchers[0].matcher.clone();
    long_matchers[1].matcher = format!("{over_budget}c");
    long_matchers[1].normalized_matcher = long_matchers[1].matcher.clone();

    let (_directory, config) = config();
    let mut findings = Vec::new();
    let suppressions = super::suppression::SuppressionIndex::new(&config, &long_matchers);
    let pair_analysis =
        analyze_definition_pairs(&long_matchers, &config, &suppressions, &mut findings);
    assert!(pair_analysis.census.truncated);
    assert_eq!(pair_analysis.census.candidate_comparisons_evaluated, 0);
    assert_eq!(pair_analysis.census.skipped_candidate_comparisons, 1);
    assert_eq!(
        pair_analysis.census.candidate_sources["identicalHandler"].evaluated,
        0
    );
    assert_eq!(
        pair_analysis.census.candidate_sources["identicalHandler"].skipped,
        1
    );
    assert!(pair_analysis
        .incomplete
        .as_deref()
        .is_some_and(|error| error.contains("after safety limits")));
    assert!(!findings.iter().any(|finding| matches!(
        finding.rule,
        Rule::DuplicateHandler | Rule::NearDuplicateStep
    )));

    let mut long_handlers = definitions(
        "Given('the account is enabled', () => first());\nGiven('the accounts are enabled', () => second());",
    );
    for (index, definition) in long_handlers.iter_mut().enumerate() {
        definition.handler.alpha_normalized = format!("alpha-{index}");
        definition.handler.structural = format!("structure-{index}");
        definition.handler.behavior_signature = vec!["call:shared-event".to_owned(); 10_000];
    }
    let mut findings = Vec::new();
    let suppressions = super::suppression::SuppressionIndex::new(&config, &long_handlers);
    let pair_analysis =
        analyze_definition_pairs(&long_handlers, &config, &suppressions, &mut findings);
    assert!(pair_analysis.census.truncated);
    assert_eq!(pair_analysis.census.candidate_comparisons_evaluated, 0);
    assert_eq!(pair_analysis.census.skipped_candidate_comparisons, 1);
    assert_eq!(
        pair_analysis.census.candidate_sources["matcherBlocking"].evaluated,
        0
    );
    assert_eq!(
        pair_analysis.census.candidate_sources["matcherBlocking"].skipped,
        1
    );
    assert!(pair_analysis.incomplete.is_some());
}

#[test]
fn suppression_globs_compile_once_and_pair_lookups_share_the_work_budget() {
    let base = definitions("Given('base', () => sharedImplementation());").remove(0);
    let (directory, mut config) = config();
    let long_directory = "nested/".repeat(30);
    let definitions = (0..700)
        .map(|index| {
            let mut definition = base.clone();
            definition.matcher = format!("operation label {index:04}");
            definition.normalized_matcher = definition.matcher.clone();
            definition.location.path = directory
                .path()
                .join(format!("{long_directory}steps-{index}.ts"));
            definition.location.line = index + 1;
            definition
        })
        .collect::<Vec<_>>();
    config.suppressions = (0..1_000)
        .map(|index| SuppressionConfig {
            rule: Rule::DuplicateHandler,
            reason: format!("legacy exception {index}"),
            path: Some(format!("missing-{index}/**")),
            matcher: None,
        })
        .collect();

    let suppressions = super::suppression::SuppressionIndex::new(&config, &definitions);
    assert_eq!(suppressions.compiled_path_count(), 1_000);
    for _ in 0..10 {
        assert!(suppressions
            .find_reason(Rule::DuplicateHandler, &[0, 1])
            .is_none());
    }
    assert_eq!(suppressions.compiled_path_count(), 1_000);
    let unmatched = suppressions.unmatched();
    assert!(unmatched.truncated);
    assert!(unmatched.indices.len() < config.suppressions.len());

    let mut findings = Vec::new();
    let pair_analysis =
        analyze_definition_pairs(&definitions, &config, &suppressions, &mut findings);
    assert!(pair_analysis.census.truncated);
    assert!(pair_analysis.census.candidate_comparisons_evaluated < definitions.len() - 1);
    assert!(pair_analysis
        .incomplete
        .as_deref()
        .is_some_and(|error| error.contains("after safety limits")));
}

#[test]
fn suppression_lookup_borrows_and_bounds_large_reasons_until_a_finding_is_retained() {
    let definitions = definitions("Given('left', () => work()); Given('right', () => work());");
    let (_directory, mut config) = config();
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "accepted because migration is in progress ".repeat(10_000),
        path: Some("**".to_owned()),
        matcher: None,
    });

    let suppressions = super::suppression::SuppressionIndex::new(&config, &definitions);
    let reason = suppressions
        .find_reason(Rule::DuplicateHandler, &[0, 1])
        .expect("matching suppression");
    assert!(std::ptr::eq(
        reason.as_ptr(),
        config.suppressions[0].reason.as_ptr()
    ));
    assert_eq!(
        reason.chars().count(),
        crate::resource_limits::MAX_SUPPRESSION_REASON_CHARS
    );

    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .expect("duplicate handler finding");
    assert_eq!(
        finding.suppression.as_ref().unwrap().reason.chars().count(),
        crate::resource_limits::MAX_SUPPRESSION_REASON_CHARS
    );
}

#[test]
fn structural_class_limit_preserves_work_from_later_classes() {
    let mut source = String::new();
    for prefix in ["account", "invoice"] {
        for index in 0..6 {
            source.push_str(&format!(
                "Given('{prefix} operation {index}', () => {prefix}Action({index}));\n"
            ));
        }
    }
    let class_definitions = definitions(&source);
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 100;
    config.max_structural_class_comparisons = 2;

    let generated = definition_pair_candidates(&class_definitions, &config);
    assert_eq!(generated.census.truncated_structural_classes, 2);
    assert_eq!(
        generated.census.candidate_sources["structuralHandler"].evaluated,
        4
    );
    assert!((0..6).any(|left| (left + 1..6).any(|right| generated.contains_pair(left, right))));
    assert!((6..12).any(|left| (left + 1..12).any(|right| generated.contains_pair(left, right))));
}

#[test]
fn global_candidate_limit_reports_the_source_that_was_truncated() {
    let definitions = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());\nGiven('same', () => four());\nGiven('same', () => five());",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 2;
    config.max_structural_class_comparisons = 100;

    let generated = definition_pair_candidates(&definitions, &config);
    assert_eq!(generated.len(), 2);
    assert!(generated.census.truncated);
    assert_eq!(
        generated.census.candidate_sources["normalizedMatcher"].skipped,
        2
    );
    assert_eq!(
        generated
            .census
            .candidate_sources
            .values()
            .map(|source| source.evaluated)
            .sum::<usize>(),
        generated.len()
    );
}

#[test]
fn candidate_limits_are_inclusive_at_the_exact_boundary() {
    let structural = definitions(
        "Given('operation one', () => perform(1));\nGiven('operation two', () => perform(2));\nGiven('operation three', () => perform(3));",
    );
    let exact = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 3;
    config.max_structural_class_comparisons = 3;

    let structural = definition_pair_candidates(&structural, &config);
    assert_eq!(structural.len(), 3);
    assert!(!structural.census.truncated);
    assert_eq!(structural.census.skipped_candidate_comparisons, 0);
    assert_eq!(structural.census.truncated_structural_classes, 0);

    config.max_candidate_comparisons = 2;
    let exact = definition_pair_candidates(&exact, &config);
    assert_eq!(exact.len(), 2);
    assert!(!exact.census.truncated);
    assert_eq!(exact.census.skipped_candidate_comparisons, 0);
}

#[test]
fn candidate_limits_do_not_count_structural_pairs_that_cannot_reach_a_rule() {
    let definitions = definitions(
        "Given('same', () => action(1));\nGiven('same', () => action(2));\nGiven('same', () => action(3));",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 1;
    config.max_structural_class_comparisons = 10;

    let generated = definition_pair_candidates(&definitions, &config);
    assert!(generated.census.truncated);
    assert_eq!(generated.len(), 1);
    assert_eq!(generated.census.skipped_candidate_comparisons, 1);
    assert_eq!(
        generated.census.candidate_sources["normalizedMatcher"].skipped,
        1
    );
    assert_eq!(
        generated.census.candidate_sources["structuralHandler"].skipped,
        0
    );
}

#[test]
fn same_normalized_pairs_do_not_consume_the_structural_limit() {
    let definitions = definitions(
        "Given('same', () => action(1));\nGiven('same', () => action(2));\nGiven('same', () => action(3));",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 3;
    config.max_structural_class_comparisons = 1;

    let generated = definition_pair_candidates(&definitions, &config);
    assert_eq!(generated.len(), 2);
    assert!(!generated.census.truncated);
    assert_eq!(generated.census.skipped_candidate_comparisons, 0);
    assert_eq!(generated.census.truncated_structural_classes, 0);
    assert_eq!(
        generated.census.candidate_sources["normalizedMatcher"].evaluated,
        2
    );
    assert_eq!(
        generated.census.candidate_sources["structuralHandler"].evaluated,
        0
    );
}

#[test]
fn same_normalized_class_does_not_report_false_structural_truncation() {
    let definitions = definitions(
        "Given('same', () => action(1));\nGiven('same', () => action(2));\nGiven('same', () => action(3));\nGiven('same', () => action(4));",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 10;
    config.max_structural_class_comparisons = 1;

    let generated = definition_pair_candidates(&definitions, &config);
    assert!(!generated.census.truncated);
    assert_eq!(generated.census.skipped_candidate_comparisons, 0);
    assert_eq!(
        generated.census.candidate_sources["normalizedMatcher"].evaluated,
        3
    );
    assert_eq!(
        generated.census.candidate_sources["structuralHandler"].evaluated,
        0
    );
}

#[test]
fn structural_overlap_ignores_existing_pairs_within_one_handler_group() {
    let definitions = definitions(
        "Given('first', () => action('same'));\nGiven('second', () => action('same'));\nGiven('third', () => action('third'));\nGiven('fourth', () => action('fourth'));",
    );
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 10;
    config.max_structural_class_comparisons = 1;

    let generated = definition_pair_candidates(&definitions, &config);
    assert!(generated.census.truncated);
    assert_eq!(generated.census.skipped_candidate_comparisons, 4);
    assert_eq!(
        generated.census.candidate_sources["identicalHandler"].evaluated,
        1
    );
}

#[test]
fn trivial_and_unresolved_handlers_do_not_consume_candidate_budgets() {
    for definitions in [
        definitions("Given('one', () => {});\nGiven('two', () => {});"),
        definitions("Given('one', importedHandler);\nGiven('two', anotherImportedHandler);"),
    ] {
        let (_directory, config) = config();
        let generated = definition_pair_candidates(&definitions, &config);
        assert!(generated.is_empty());
        assert_eq!(generated.census.candidate_comparisons_evaluated, 0);
        assert!(!generated.census.truncated);
    }
}

#[test]
fn candidate_limit_diagnostic_bounds_affected_class_locations() {
    let mut source = String::new();
    for class in 0..5 {
        for index in 0..3 {
            source.push_str(&format!(
                "Given('class {class} operation {index}', () => action{class}({index}));\n"
            ));
        }
    }
    let class_definitions = definitions(&source);
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 100;
    config.max_structural_class_comparisons = 1;

    let (outcome, census, _) = analyze_for_cli(class_definitions, Vec::new(), &config).unwrap();
    assert_eq!(census.truncated_structural_classes, 5);
    assert_eq!(outcome.incomplete.len(), 1);
    assert!(outcome.incomplete[0].contains("and 2 more"));

    let definitions = definitions(
        "Given('one', () => action(1));\nGiven('two', () => action(2));\nGiven('three', () => action(3));",
    );
    let (outcome, census, _) = analyze_for_cli(definitions, Vec::new(), &config).unwrap();
    assert_eq!(census.truncated_structural_classes, 1);
    assert!(!outcome.incomplete[0].contains("and 0 more"));
}

#[test]
fn regex_resource_errors_are_preserved_for_cli_reporting_and_library_callers() {
    let definitions = definitions("Given(/a{1000000}/, () => work());");
    let (_directory, config) = config();

    let error = analyze(definitions.clone(), Vec::new(), &config).unwrap_err();
    assert!(error.to_string().contains("regex resource limit"));

    let outcome = analyze_with_diagnostics(definitions, Vec::new(), &config).unwrap();
    assert_eq!(outcome.result.definitions.len(), 1);
    assert_eq!(outcome.incomplete.len(), 1);
}

#[test]
fn equivalence_classes_report_a_spanning_set_instead_of_every_pair() {
    let definitions = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());\nGiven('same', () => four());",
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert_eq!(
        result
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateMatcher)
            .count(),
        3
    );
}

#[test]
fn large_equivalence_classes_report_one_stable_cluster() {
    let definitions = definitions(
        "Given('same', () => one());\nGiven('same', () => two());\nGiven('same', () => three());\nGiven('same', () => four());\nGiven('same', () => five());",
    );
    let (_directory, config) = config();
    let result = analyze(definitions.clone(), Vec::new(), &config).unwrap();
    let duplicate_matcher = result
        .findings
        .iter()
        .filter(|finding| finding.rule == Rule::DuplicateMatcher)
        .collect::<Vec<_>>();

    assert_eq!(duplicate_matcher.len(), 1);
    let finding = duplicate_matcher[0];
    assert_eq!(finding.related.len(), 4);
    assert!(finding.evidence.comparison.is_none());
    let cluster = finding.evidence.cluster.as_ref().unwrap();
    assert_eq!(cluster.member_count, 5);
    assert_eq!(cluster.definition_fingerprints.len(), 5);
    assert_eq!(cluster.pair_findings_collapsed, 4);
    assert!(!cluster.members_truncated);

    let duplication = DuplicationThreshold::from_result(&result, 100.0);
    assert_eq!(duplication.duplicated_definitions, 5);
    assert_eq!(duplication.percentage, 100.0);

    let mut reversed = definitions;
    reversed.reverse();
    let reordered = analyze(reversed, Vec::new(), &config).unwrap();
    let reordered_finding = reordered
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateMatcher)
        .unwrap();
    assert_eq!(
        crate::modes::finding_fingerprint(finding),
        crate::modes::finding_fingerprint(reordered_finding)
    );
}

#[test]
fn inline_suppression_applies_to_pair_findings() {
    let definitions = definitions(
        r#"
// cuke-dedup:ignore duplicate-matcher -- intentionally split setup paths
Given('same step', () => first());
Given('same step', () => second());
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let duplicate = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateMatcher)
        .unwrap();
    assert_eq!(
        duplicate
            .suppression
            .as_ref()
            .map(|value| value.reason.as_str()),
        Some("intentionally split setup paths")
    );
}

#[test]
fn wording_drift_with_same_handler_is_near_duplicate() {
    let definitions = definitions(
        r#"
Then('the item is visible', async () => { await expect(item).toBeVisible(); });
Then('the items are visible', async () => { await expect(item).toBeVisible(); });
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn matcher_blocking_finds_near_wording_across_related_handler_shapes() {
    let definitions = definitions(
        r##"
Given("I am on login page", async function () {
  await this.page.goto("/login");
  await this.page.fill("#email", "user@example.test");
});
Given("I am on the login page", async function () {
  await this.page.goto("/login");
  await this.page.fill("#email", "user@example.test");
  await this.page.waitForLoadState();
});
"##,
    );
    assert_ne!(
        definitions[0].handler.structural,
        definitions[1].handler.structural
    );
    assert!(handler_similarity(&definitions[0], &definitions[1]) >= 0.6);
    let (_directory, config) = config();
    let candidates = definition_pair_candidates(&definitions, &config);
    assert!(candidates.contains_pair(0, 1));
    assert_eq!(
        candidates.census.candidate_sources["matcherBlocking"].evaluated,
        1
    );

    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn matcher_blocking_rejects_similar_wording_without_handler_agreement() {
    let definitions = definitions(
        r##"
Given("I am on login page", async function () {
  await this.page.goto("/login");
  await this.page.fill("#email", "user@example.test");
});
Given("I am on the login page", async function () {
  await unrelatedAudit();
  await unrelatedNotification();
});
"##,
    );
    let (_directory, config) = config();
    assert!(definition_pair_candidates(&definitions, &config).is_empty());
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn matcher_blocking_semantic_boundary_is_table_driven() {
    struct Case {
        name: &'static str,
        left_handler: &'static str,
        right_handler: &'static str,
        expected_near: bool,
        expected_similarity: f64,
    }

    let cases = [
        Case {
            name: "one shared action out of two reaches the boundary",
            left_handler: "async function () { await this.page.goto('/login'); }",
            right_handler: "async function () { await this.page.goto('/login'); await this.page.waitForLoadState(); }",
            expected_near: true,
            expected_similarity: 0.5,
        },
        Case {
            name: "direct and fluent calls share their terminal action",
            left_handler: "async ({ page }) => { await page.click('#save'); }",
            right_handler: "async ({ page }) => { await page.locator('#save').click(); }",
            expected_near: true,
            expected_similarity: 0.5,
        },
        Case {
            name: "unrelated awaited calls do not share synthetic behavior",
            left_handler: "async () => { await loadAccount(); }",
            right_handler: "async () => { await writeAudit(); }",
            expected_near: false,
            expected_similarity: 0.0,
        },
        Case {
            name: "receiver names remain semantically distinct",
            left_handler: "async () => { await page.click('#save'); }",
            right_handler: "async () => { await audit.click('#save'); }",
            expected_near: false,
            expected_similarity: 0.0,
        },
        Case {
            name: "control flow alone cannot establish handler agreement",
            left_handler: "() => { if (ready) return first; return second; }",
            right_handler: "() => { if (active) return third; }",
            expected_near: false,
            expected_similarity: 2.0 / 3.0,
        },
        Case {
            name: "one shared action out of three remains below the boundary",
            left_handler: "async ({ page }) => { await page.goto('/'); await page.fill('#x', 'x'); await page.click('#save'); }",
            right_handler: "async ({ page }) => { await page.goto('/'); await page.reload(); await page.screenshot(); }",
            expected_near: false,
            expected_similarity: 1.0 / 3.0,
        },
        Case {
            name: "different terminal operations do not collide",
            left_handler: "async ({ page }) => { await page.click('#save'); }",
            right_handler: "async ({ page }) => { await page.fill('#save', 'value'); }",
            expected_near: false,
            expected_similarity: 0.0,
        },
    ];

    let (_directory, config) = config();
    for case in cases {
        let definitions = definitions(&format!(
            "Given('I am on login page', {}); Given('I am on the login page', {});",
            case.left_handler, case.right_handler
        ));
        let similarity = handler_similarity(&definitions[0], &definitions[1]);
        assert!(
            (similarity - case.expected_similarity).abs() < f64::EPSILON,
            "{}: {similarity}",
            case.name
        );
        let result = analyze(definitions, Vec::new(), &config).unwrap();
        assert_eq!(
            result
                .findings
                .iter()
                .any(|finding| finding.rule == Rule::NearDuplicateStep),
            case.expected_near,
            "{}",
            case.name
        );
    }
}

#[test]
fn matcher_blocking_finds_near_members_outside_an_identical_handler_spanning_tree() {
    let definitions = definitions(
        r#"
Given('completely unrelated setup', () => sharedImplementation());
Given('the account is enabled', () => sharedImplementation());
Given('the accounts are enabled', () => sharedImplementation());
"#,
    );
    let (_directory, config) = config();
    let candidates = definition_pair_candidates(&definitions, &config);
    assert!(candidates.contains_pair(1, 2));
    assert!(candidates.census.candidate_sources["matcherBlocking"].evaluated >= 1);

    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let near = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::NearDuplicateStep)
        .expect("near wording pair");
    let comparison = near.evidence.comparison.as_ref().unwrap();
    assert_eq!(
        [
            comparison.left_matcher.as_str(),
            comparison.right_matcher.as_str()
        ],
        ["the account is enabled", "the accounts are enabled"]
    );
}

#[test]
fn matcher_blocking_keeps_alpha_identical_handlers_with_empty_behavior_signatures() {
    let definitions = definitions(
        r#"
Given('completely unrelated setup', () => world.value);
Given('the account is enabled', () => world.value);
Given('the accounts are enabled', () => world.value);
"#,
    );
    assert!(definitions
        .iter()
        .all(|definition| definition.handler.behavior_signature.is_empty()));
    assert!(definitions
        .iter()
        .all(|definition| definition.handler.comparable && !definition.handler.trivial));

    let (_directory, config) = config();
    let candidates = definition_pair_candidates(&definitions, &config);
    assert!(candidates.contains_pair(1, 2));
    assert!(candidates.census.candidate_sources["matcherBlocking"].evaluated >= 1);

    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn matcher_blocking_respects_the_global_candidate_limit() {
    let mut definitions = definitions(
        r#"
Given('account operation one', () => first());
Given('account operation two', () => second());
Given('account operation three', () => third());
Given('account operation four', () => fourth());
"#,
    );
    for (index, definition) in definitions.iter_mut().enumerate() {
        definition.handler.alpha_normalized = format!("alpha-{index}");
        definition.handler.structural = format!("structure-{index}");
        definition.handler.behavior_signature =
            vec!["call:open".to_owned(), "call:fill".to_owned()];
    }
    let (_directory, mut config) = config();
    config.max_candidate_comparisons = 2;
    let generated = definition_pair_candidates(&definitions, &config);
    assert_eq!(generated.len(), 2);
    assert!(generated.census.truncated);
    assert_eq!(
        generated.census.candidate_sources["matcherBlocking"].evaluated,
        2
    );
    assert_eq!(
        generated.census.candidate_sources["matcherBlocking"].skipped,
        1
    );
}

#[test]
fn homogeneous_handler_groups_generate_linear_candidates() {
    let mut source = String::new();
    for index in 0..2_000 {
        source.push_str(&format!(
            "Given('homogeneous handler wording {index}', () => sharedImplementation());\n"
        ));
    }
    let definitions = definitions(&source);
    let (_directory, config) = config();
    assert!(definition_pair_candidates(&definitions, &config).len() <= definitions.len() * 2);
}

#[test]
fn literal_handler_differences_prefer_parameterization_over_near_wording() {
    let definitions = definitions(
        r#"
Then('the save button is shown', async ({ page }) => { await expect(page.locator('#save')).toBeVisible(); });
Then('the save buttons are shown', async ({ page }) => { await expect(page.locator('#saves')).toBeVisible(); });
When('再生ボタンを押す', async ({ page }) => { await page.click('#play'); });
When('停止ボタンを押す', async ({ page }) => { await page.click('#stop'); });
"#,
    );
    assert_eq!(
        definitions[0].handler.structural,
        definitions[1].handler.structural
    );
    assert_eq!(
        definitions[0].handler.behavior_signature,
        definitions[1].handler.behavior_signature
    );
    let similarity = matcher_similarity(&definitions[0], &definitions[1]);
    assert!(is_near_matcher(
        &definitions[0],
        &definitions[1],
        similarity
    ));
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::ParameterizationCandidate));
}

#[test]
fn empty_handlers_and_disjoint_placeholder_types_do_not_fail_analysis() {
    let definitions = definitions(
        r#"
Given('an integer {int}', () => {});
Given('a string {string}', () => {});
Given('another empty step', () => {});
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result.findings.iter().any(|finding| matches!(
        finding.rule,
        Rule::DuplicateHandler | Rule::NormalizedMatcher
    )));
}

#[test]
fn semantically_distinct_matchers_do_not_become_error_equivalent() {
    let definitions = definitions(
        r#"
Given(/^I wait (\d+) seconds$/, () => first());
Given(/^I wait (a few|several) seconds$/, () => second());
Given('custom {int}', () => third());
Given('custom {INT}', () => fourth());
Given(/^literal collision$/i, () => fifth());
Given('literal collision /i', () => sixth());
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NormalizedMatcher));
}

#[test]
fn awaited_stub_handlers_do_not_create_duplicate_handler_storms() {
    let definitions = definitions(
        r#"
async function notImplemented() { await pending(); }
Given('stub one', notImplemented);
Given('stub two', notImplemented);
Given('stub three', notImplemented);
Given('stub four', notImplemented);
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn similarity_rejects_opposite_or_different_actions() {
    for (left, right) in [
        ("the account is active", "the account is inactive"),
        ("the user can log in", "the user cannot log in"),
    ] {
        let definitions = definitions(&format!(
            "Then('{left}', () => work()); Then('{right}', () => work());"
        ));
        let similarity = matcher_similarity(&definitions[0], &definitions[1]);
        assert!(!is_near_matcher(
            &definitions[0],
            &definitions[1],
            similarity
        ));
    }
}

#[test]
fn matcher_similarity_gates_are_pinned_at_short_and_long_boundaries() {
    struct Case {
        name: &'static str,
        left: &'static str,
        right: &'static str,
        similarity: f64,
        expected: bool,
    }

    for case in [
        Case {
            name: "twelve characters accept the short boundary",
            left: "abcdefghijkl",
            right: "abcdefghijkx",
            similarity: 0.92,
            expected: true,
        },
        Case {
            name: "twelve characters reject below the short boundary",
            left: "abcdefghijkl",
            right: "abcdefghijkx",
            similarity: 0.919,
            expected: false,
        },
        Case {
            name: "thirteen characters accept the long boundary",
            left: "abcdefghijklm",
            right: "abcdefghijklz",
            similarity: 0.90,
            expected: true,
        },
        Case {
            name: "thirteen characters reject below the long boundary",
            left: "abcdefghijklm",
            right: "abcdefghijklz",
            similarity: 0.899,
            expected: false,
        },
        Case {
            name: "negation wins over a high similarity score",
            left: "user is active",
            right: "user is not active",
            similarity: 0.99,
            expected: false,
        },
    ] {
        let definitions = definitions(&format!(
            "Then('{}', () => work()); Then('{}', () => work());",
            case.left, case.right
        ));
        assert_eq!(
            is_near_matcher(&definitions[0], &definitions[1], case.similarity),
            case.expected,
            "{}",
            case.name
        );
    }
}

#[test]
fn matcher_similarity_reports_the_stronger_normalized_score() {
    let mut pair = definitions("Given('left', () => first()); Given('right', () => second());");
    for (left, right) in [("", ""), ("kitten", "sitting"), ("account", "accounts")] {
        pair[0].normalized_matcher = left.to_owned();
        pair[1].normalized_matcher = right.to_owned();
        let expected = if left.is_empty() && right.is_empty() {
            1.0
        } else {
            let longest = left.chars().count().max(right.chars().count());
            let edit = 1.0 - strsim::levenshtein(left, right) as f64 / longest as f64;
            edit.max(strsim::jaro_winkler(left, right))
        };
        assert!(
            (matcher_similarity(&pair[0], &pair[1]) - expected).abs() < f64::EPSILON,
            "{left:?} and {right:?}"
        );
    }
}

#[test]
fn handler_similarity_contract_is_table_driven() {
    let mut base = definitions("Given('left', () => first()); Given('right', () => second());");
    base[0].handler.alpha_normalized = "alpha-left".to_owned();
    base[1].handler.alpha_normalized = "alpha-right".to_owned();
    base[0].handler.structural = "structure-left".to_owned();
    base[1].handler.structural = "structure-right".to_owned();

    let cases = [
        (
            "alpha equivalence",
            "same",
            "same",
            "left",
            "right",
            vec!["open"],
            vec!["open"],
            1.0,
        ),
        (
            "alpha equivalence with conflicting behavior",
            "same",
            "same",
            "left",
            "right",
            vec!["open"],
            vec!["close"],
            0.0,
        ),
        (
            "structural and behavioral equivalence",
            "left",
            "right",
            "same",
            "same",
            vec!["open"],
            vec!["open"],
            0.95,
        ),
        (
            "structural equivalence with conflicting behavior",
            "left",
            "right",
            "same",
            "same",
            vec!["open"],
            vec!["close"],
            0.0,
        ),
        (
            "ordered behavior overlap",
            "left",
            "right",
            "left",
            "right",
            vec!["open", "fill", "save"],
            vec!["open", "save", "notify"],
            2.0 / 3.0,
        ),
        (
            "no behavior",
            "left",
            "right",
            "left",
            "right",
            vec![],
            vec![],
            0.0,
        ),
    ];

    for (
        name,
        left_alpha,
        right_alpha,
        left_structure,
        right_structure,
        left_events,
        right_events,
        expected,
    ) in cases
    {
        let mut pair = base.clone();
        pair[0].handler.alpha_normalized = left_alpha.to_owned();
        pair[1].handler.alpha_normalized = right_alpha.to_owned();
        pair[0].handler.structural = left_structure.to_owned();
        pair[1].handler.structural = right_structure.to_owned();
        pair[0].handler.behavior_signature = left_events.into_iter().map(str::to_owned).collect();
        pair[1].handler.behavior_signature = right_events.into_iter().map(str::to_owned).collect();
        assert!(
            (handler_similarity(&pair[0], &pair[1]) - expected).abs() < f64::EPSILON,
            "{name}"
        );
    }
}

#[test]
fn duplicate_matcher_components_are_invariant_to_definition_input_order() {
    let mut ordered = definitions(
        "Given('same', () => first());\nGiven('same', () => second());\nGiven('same', () => third());",
    );
    for (index, definition) in ordered.iter_mut().enumerate() {
        definition.location.path = PathBuf::from(format!("definition-{index}.ts"));
    }
    let (_directory, config) = config();

    fn component_members(result: &AnalysisResult) -> Vec<String> {
        let mut paths = result
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::DuplicateMatcher)
            .flat_map(|finding| std::iter::once(&finding.primary).chain(&finding.related))
            .map(|location| location.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        paths
    }

    let expected = [
        "definition-0.ts".to_owned(),
        "definition-1.ts".to_owned(),
        "definition-2.ts".to_owned(),
    ];
    for permutation in [[0, 1, 2], [2, 1, 0], [0, 2, 1], [1, 0, 2]] {
        let definitions = permutation
            .into_iter()
            .map(|index| ordered[index].clone())
            .collect();
        let result = analyze(definitions, Vec::new(), &config).unwrap();
        assert_eq!(component_members(&result), expected);
        assert_eq!(
            result
                .findings
                .iter()
                .filter(|finding| finding.rule == Rule::DuplicateMatcher)
                .count(),
            2
        );
    }
}

#[test]
fn shared_context_does_not_override_parameterization_evidence() {
    for (left, right) in [
        (
            "I click the save button on the checkout summary page",
            "I click the cancel button on the checkout summary page",
        ),
        (
            "I select the first row of the table",
            "I select the last row of the table",
        ),
        (
            "the advanced reporting feature is enabled for this account",
            "the advanced reporting feature is disabled for this account",
        ),
    ] {
        let definitions = definitions(&format!(
            "Then('{left}', () => choose('left')); Then('{right}', () => choose('right'));"
        ));
        let (_directory, config) = config();
        let result = analyze(definitions, Vec::new(), &config).unwrap();
        assert!(result
            .findings
            .iter()
            .any(|finding| finding.rule == Rule::ParameterizationCandidate));
        assert!(!result
            .findings
            .iter()
            .any(|finding| finding.rule == Rule::NearDuplicateStep));
    }
}

#[test]
fn path_and_matcher_suppression_must_select_the_same_definition() {
    let mut definitions =
        definitions("Given('left matcher', () => work()); Given('right matcher', () => work());");
    let (directory, mut config) = config();
    definitions[0].location.path = directory.path().join("left.ts");
    definitions[1].location.path = directory.path().join("right.ts");
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "must not span definitions".to_owned(),
        path: Some("left.ts".to_owned()),
        matcher: Some("right matcher".to_owned()),
    });
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap();
    assert!(finding.suppression.is_none());

    config.suppressions[0].path = Some("*.ts".to_owned());
    config.suppressions[0].matcher = Some("missing matcher".to_owned());
    let result = analyze(result.definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap();
    assert!(finding.suppression.is_none());
}

#[test]
fn suppression_path_globs_remain_root_relative() {
    let mut definitions =
        definitions("Given('left matcher', () => work()); Given('right matcher', () => work());");
    let (_directory, mut config) = config();
    definitions[0].location.path = config.root.join("root.ts");
    definitions[1].location.path = config.root.join("nested/steps.ts");
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "root files only".to_owned(),
        path: Some("*.ts".to_owned()),
        matcher: None,
    });
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap();
    assert!(finding.suppression.is_some());

    config.suppressions[0].path = Some("steps.ts".to_owned());
    let result = analyze(result.definitions, Vec::new(), &config).unwrap();
    let finding = result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap();
    assert!(finding.suppression.is_none());
}

#[test]
fn path_suppression_requires_every_definition_in_a_pair_to_be_in_scope() {
    let mut definitions =
        definitions("Given('left matcher', () => work()); Given('right matcher', () => work());");
    let (_directory, mut config) = config();
    definitions[0].location.path = config.root.join("legacy/left.ts");
    definitions[1].location.path = config.root.join("current/right.ts");
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "legacy directory only".to_owned(),
        path: Some("legacy/**".to_owned()),
        matcher: None,
    });

    let result = analyze(definitions.clone(), Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap()
        .suppression
        .is_none());

    definitions[1].location.path = config.root.join("legacy/right.ts");
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .find(|finding| finding.rule == Rule::DuplicateHandler)
        .unwrap()
        .suppression
        .is_some());
}

#[test]
fn suppression_directory_patterns_and_unmatched_detection_share_semantics() {
    let mut definitions = definitions("Given('legacy', () => work());");
    let (_directory, mut config) = config();
    definitions[0].location.path = config.root.join("legacy/nested/steps.ts");
    config.suppressions.push(SuppressionConfig {
        rule: Rule::DuplicateHandler,
        reason: "legacy tree".to_owned(),
        path: Some("legacy/".to_owned()),
        matcher: None,
    });
    assert!(
        super::suppression::SuppressionIndex::new(&config, &definitions)
            .unmatched()
            .indices
            .is_empty()
    );

    config.suppressions[0].path = Some("missing/**".to_owned());
    assert_eq!(
        super::suppression::SuppressionIndex::new(&config, &definitions)
            .unmatched()
            .indices,
        [0]
    );
}

#[test]
fn opposite_assertions_are_not_duplicate_handlers() {
    let definitions = definitions(
        r#"
Then('the item is visible', async () => { await expect(item).toBeVisible(); });
Then('the item is hidden', async () => { await expect(item).not.toBeVisible(); });
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn similar_matchers_on_different_assertion_subjects_are_not_near_duplicates() {
    let definitions = definitions(
        r#"
Then('the settings panel shows the primary account field', async ({ page }, expected) => {
  await expect(new AccountForm(page).primaryInput).toHaveValue(expected);
});
Then('the settings panel shows the secondary account field', async ({ page }, expected) => {
  await expect(new AccountForm(page).secondaryInput).toHaveValue(expected);
});
"#,
    );
    assert!(matcher_similarity(&definitions[0], &definitions[1]) >= 0.9);
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn similar_matchers_with_opposite_assertion_chains_are_not_near_duplicates() {
    let definitions = definitions(
        r#"
Then('the navigation drawer state indicator is collapsed', async ({ page }) => {
  await expect(new Navigation(page).drawer).toHaveClass(/collapsed/);
});
Then('the navigation drawer state indicator is expanded', async ({ page }) => {
  await expect(new Navigation(page).drawer).not.toHaveClass(/collapsed/);
});
"#,
    );
    assert!(matcher_similarity(&definitions[0], &definitions[1]) >= 0.9);
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn assertion_only_handlers_reach_near_duplicate_verification() {
    let definitions = definitions(
        r#"
Then('the account badge is visible', async ({ page }) => {
  await expect(new AccountPage(page).badge).toBeVisible();
});
Then('the account badge is now visible', async ({ page }) =>
  expect(new AccountPage(page).badge).toBeVisible()
);
"#,
    );
    assert_ne!(
        definitions[0].handler.structural,
        definitions[1].handler.structural
    );
    let (_directory, config) = config();
    let candidates = definition_pair_candidates(&definitions, &config);
    assert_eq!(
        candidates.census.candidate_sources["matcherBlocking"].evaluated,
        1
    );
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn conflicting_assertion_values_are_not_near_duplicates() {
    let definitions = definitions(
        r#"
Then('the account status indicator shows the first condition', async ({ page }) => {
  await expect(new AccountPage(page).status).toBe('ready');
});
Then('the account status indicator shows the final condition', async ({ page }) => {
  await expect(new AccountPage(page).status).toBe('idle');
});
"#,
    );
    assert!(matcher_similarity(&definitions[0], &definitions[1]) >= 0.9);
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn mostly_matching_ordered_behavior_remains_a_near_duplicate_candidate() {
    let assertions = |last| {
        (0..10)
            .map(|index| {
                let expected = if index == 9 { last } else { "shared" };
                format!("expect(state.field{index}).toBe('{expected}');")
            })
            .collect::<String>()
    };
    let definitions = definitions(&format!(
        "Then('the account workflow is ready', ({{state}}) => {{ {} }}); Then('the account workflow is nearly ready', ({{state}}) => {{ {} }});",
        assertions("first"),
        assertions("second")
    ));
    assert_eq!(
        definitions[0].handler.structural,
        definitions[1].handler.structural
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn decorated_method_metadata_does_not_create_near_duplicate_findings() {
    let definitions = definitions(
        r#"
import { Then } from 'playwright-bdd/decorators';
import { expect } from '@playwright/test';
class StatusSteps {
  @Then('the account status indicator shows the first condition')
  first({ state }) { expect(state).toBe('ready'); }

  @Then('the account status indicator shows the final condition')
  second({ state }) { expect(state).toBe('idle'); }
}
"#,
    );
    assert_eq!(definitions.len(), 2);
    assert!(matcher_similarity(&definitions[0], &definitions[1]) >= 0.9);
    assert_eq!(handler_similarity(&definitions[0], &definitions[1]), 0.0);

    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn incompatible_decorated_method_semantics_veto_near_duplicate_findings() {
    let definitions = definitions(
        r#"
import { Then } from 'playwright-bdd/decorators';
import { expect } from '@playwright/test';
class StatusSteps {
  @Then('the account status indicator is visible')
  first({ state }) { expect(state).toBeVisible(); }

  @Then('the account status indicator is now visible')
  async second({ state }) { expect(state).toBeVisible(); }
}
"#,
    );
    assert_eq!(definitions.len(), 2);
    assert!(matcher_similarity(&definitions[0], &definitions[1]) >= 0.9);
    assert_eq!(handler_similarity(&definitions[0], &definitions[1]), 0.0);

    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::NearDuplicateStep));
}

#[test]
fn pair_evidence_retains_source_and_unicode_safe_matcher_delta() {
    let definitions = definitions(
        r#"
Then('the item is visible', async () => { await expect(item).toBeVisible(); });
Then('the items are visible', async () => { await expect(item).toBeVisible(); });
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    let comparison = result
        .findings
        .iter()
        .find_map(|finding| finding.evidence.comparison.as_ref())
        .expect("pair comparison");
    assert_eq!(comparison.matcher_diff.prefix, "the item");
    assert_eq!(comparison.matcher_diff.left_change, " is");
    assert_eq!(comparison.matcher_diff.right_change, "s are");
    assert_eq!(comparison.matcher_diff.suffix, " visible");
    assert!(comparison.left_handler.contains("toBeVisible"));
}

#[test]
fn concrete_feature_step_detects_ambiguity_and_usage() {
    let definitions = definitions(
        r#"
Given('a user named {word}', () => { makeUser(); });
Given(/^a user named .+$/, () => { makeOtherUser(); });
"#,
    );
    let steps = gherkin::extract(
        "Feature: Users\n  Scenario: Create\n    Given a user named Ada",
        Path::new("test.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::AmbiguousStep));
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::UnusedDefinition));
}

#[test]
fn scenario_outline_rows_and_case_insensitive_regexes_count_as_usage() {
    let definitions = definitions(
        r#"
Given('I have {int} items', () => { count(); });
Then(/^THE USER EXISTS$/i, () => { verify(); });
"#,
    );
    let steps = gherkin::extract(
        r#"
Feature: Outline
  Scenario Outline: Count
Given I have <count> items
Then the user exists
Examples:
  | count |
  | 2     |
"#,
        Path::new("outline.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::UnusedDefinition));
}

#[test]
fn supports_optional_and_alternative_cucumber_expression_syntax() {
    let definitions = definitions(
        r#"
Given('I have {int} cucumber(s)', () => { count(); });
Then('they are in my belly/stomach', () => { verify(); });
"#,
    );
    let steps = gherkin::extract(
        "Feature: Food\n  Scenario: Eat\n    Given I have 2 cucumbers\n    Then they are in my stomach",
        Path::new("food.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::UnusedDefinition));
}

#[test]
fn fallback_for_custom_parameters_preserves_optional_and_alternative_syntax() {
    let definitions = definitions(
        r#"
Given('I have {quantity} cucumber(s)', () => { count(); });
Then('they are in my belly/stomach with {mood}', () => { verify(); });
"#,
    );
    let steps = gherkin::extract(
        "Feature: Food\n  Scenario: Eat\n    Given I have several cucumbers\n    Then they are in my belly with joy",
        Path::new("food.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::UnusedDefinition));
}

#[test]
fn named_handlers_with_different_bodies_and_stub_handlers_do_not_duplicate() {
    let mut extracted = Vec::new();
    for (path, source) in [
        (
            "steps/a.ts",
            "function handler() { doAlpha(); } Given('alpha step', handler);",
        ),
        (
            "steps/b.ts",
            "function handler() { doBeta(); } Given('bravo step', handler);",
        ),
        (
            "steps/pending.ts",
            "function pendingStep() { throw new Error('pending'); } Given('pending one', pendingStep); Given('pending two', pendingStep);",
        ),
    ] {
        extracted.extend(
            typescript::extract(
                source,
                &SourceFile {
                    path: PathBuf::from(path),
                    language: SourceLanguage::TypeScript,
                },
            )
            .unwrap(),
        );
    }
    let (_directory, config) = config();
    let result = analyze(extracted, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn differently_bound_arguments_do_not_report_duplicate_handlers() {
    let definitions = definitions(
        r#"
function handler(value) { process(value); }
Given('first binding', handler.bind(null, 1));
Given('second binding', handler.bind(null, 2));
"#,
    );
    let (_directory, config) = config();
    let result = analyze(definitions, Vec::new(), &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| finding.rule == Rule::DuplicateHandler));
}

#[test]
fn outline_ambiguities_are_deduplicated_by_template_location_and_matches() {
    let definitions = definitions(
        r#"
Given('I have {int} items', () => { first(); });
Given(/^I have \d+ items$/, () => { second(); });
"#,
    );
    let steps = gherkin::extract(
        r#"
Feature: Repeated rows
  Scenario Outline: Counts
Given I have <count> items
Examples:
  | count |
  | 1     |
  | 2     |
  | 3     |
"#,
        Path::new("outline.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert_eq!(
        result
            .findings
            .iter()
            .filter(|finding| finding.rule == Rule::AmbiguousStep)
            .count(),
        1
    );
}

#[test]
fn outline_ambiguities_keep_disjoint_match_sets_separate() {
    let definitions = definitions(
        r#"
Given('value {int}', () => first());
Given(/^value \d+$/, () => second());
Given(/^value 1$/, () => third());
"#,
    );
    let steps = gherkin::extract(
        r#"
Feature: Different sets
  Scenario Outline: Values
    Given value <value>
    Examples:
      | value |
      | 1     |
      | 2     |
"#,
        Path::new("outline.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    let ambiguities = result
        .findings
        .iter()
        .filter(|finding| finding.rule == Rule::AmbiguousStep)
        .collect::<Vec<_>>();
    assert_eq!(ambiguities.len(), 2);
    assert!(ambiguities.iter().all(|finding| finding.related.len() >= 2));
}

#[test]
fn custom_parameter_fallback_marks_usage_but_cannot_prove_ambiguity() {
    let definitions = definitions(
        r#"
Given('I wait {duration}', () => waitDuration());
Given('I wait for the page', () => waitForPage());
"#,
    );
    let steps = gherkin::extract(
        "Feature: Wait\nScenario: custom\nGiven I wait for the page\nGiven I wait 3 seconds\n",
        Path::new("wait.feature"),
    )
    .unwrap();
    let (_directory, config) = config();
    let result = analyze(definitions, steps, &config).unwrap();
    assert!(!result
        .findings
        .iter()
        .any(|finding| { matches!(finding.rule, Rule::AmbiguousStep | Rule::UnusedDefinition) }));
}

#[test]
fn ordered_behavior_similarity_uses_longest_common_subsequence() {
    for (left, right, expected) in [
        (
            vec!["open", "fill", "save"],
            vec!["open", "save", "notify"],
            2,
        ),
        (vec!["open"], vec!["close"], 0),
        (vec!["open", "save"], vec!["open", "save"], 2),
        (vec![], vec!["open"], 0),
    ] {
        let left = left.into_iter().map(str::to_owned).collect::<Vec<_>>();
        let right = right.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(ordered_common_subsequence_len(&left, &right), expected);
        assert_eq!(ordered_common_subsequence_len(&right, &left), expected);
    }
}

#[test]
fn reported_similarity_scores_round_to_three_decimal_places() {
    for (input, expected) in [
        (0.0, 0.0),
        (0.123_4, 0.123),
        (0.123_5, 0.124),
        (0.999_9, 1.0),
    ] {
        assert_eq!(super::similarity::round_score(input), expected);
    }
}

/// Supporting invariants for OPEN-2 / R5190030268-S2, stated on dot access where an assertion is a
/// single atomic event.
///
/// An assertion is identified by its subject, its modifier chain and its expected value, and a
/// change that dropped any one of them would still emit one plausible-looking event. Each part is
/// pinned separately here: erase the subject, the modifier, or the value and this test fails.
#[test]
fn dotted_assertion_events_distinguish_subject_modifier_and_value() {
    let (_directory, config) = config();

    let events = |subject: &str, access: &str, value: &str| {
        let source =
            format!("Then('alpha holds', () => {{ expect({subject}){access}.toBe({value}); }});");
        let extracted = definitions(&source);
        assert_eq!(extracted.len(), 1, "{source}");
        extracted[0].handler.behavior_signature.clone()
    };

    let reference = events("alpha", ".not", "'ready'");
    assert!(
        reference
            .iter()
            .any(|event| event.starts_with("assert:expect.not#toBe:")),
        "dot access must record the negation modifier: {reference:?}"
    );
    assert_eq!(
        reference.len(),
        1,
        "an assertion is one atomic event: {reference:?}"
    );

    assert_ne!(
        reference,
        events("beta", ".not", "'ready'"),
        "assertions on different subjects must not share an assertion event"
    );
    assert_ne!(
        reference,
        events("alpha", "", "'ready'"),
        "a negated assertion must not match the same assertion without the modifier"
    );
    assert_ne!(
        reference,
        events("alpha", ".not", "'idle'"),
        "assertions expecting different values must not share an assertion event"
    );

    // Final outcome, not only event inequality: close wording must not equate assertions that
    // differ only in their subject. A change erasing the subject would report a near duplicate.
    let extracted = definitions(
        "Then('the alpha row is aligned', () => { expect(alpha).not.toBe('ready'); }); \
         Then('the beta row is aligned', () => { expect(beta).not.toBe('ready'); });",
    );
    assert_eq!(extracted.len(), 2);
    let result = analyze(extracted, Vec::new(), &config).unwrap();
    assert!(
        !result.findings.iter().any(|finding| matches!(
            finding.rule,
            Rule::DuplicateHandler | Rule::NearDuplicateStep | Rule::ParameterizationCandidate
        )),
        "close wording must not equate assertions on different subjects: {:?}",
        result.findings
    );
}

/// Shared by the OPEN-1 tests below. Returns, per handler, whether it stayed comparable and its
/// behavior signature, plus the handler rules the pair produced. `write` is inserted between the
/// alias and the two identical step definitions.
///
/// Per handler rather than aggregated: revoking trust for only one of a pair must not be able to
/// match a fully revoked baseline.
fn aliased_matcher_outcome(
    alias: &str,
    write: &str,
    config: &Config,
) -> (Vec<bool>, Vec<Vec<String>>, Vec<Rule>) {
    let usage = "api.expect(state).toEqual(api.expect.not.objectContaining({ role: 'admin' }));";
    let source = format!(
        "const api = require('@playwright/test'); {alias} {write} \
         Then('the parcel is ready', async ({{ state }}) => {{ {usage} }}); \
         Then('the parcel is now ready', async ({{ state }}) => {{ {usage} }});"
    );
    let extracted = definitions(&source);
    assert_eq!(extracted.len(), 2, "{source}");
    let comparable: Vec<bool> = extracted
        .iter()
        .map(|definition| definition.handler.comparable)
        .collect();
    let events: Vec<Vec<String>> = extracted
        .iter()
        .map(|definition| definition.handler.behavior_signature.clone())
        .collect();
    let result = analyze(extracted, Vec::new(), config).unwrap();
    let mut rules: Vec<Rule> = result
        .findings
        .iter()
        .map(|finding| finding.rule)
        .filter(|rule| {
            matches!(
                rule,
                Rule::DuplicateHandler | Rule::NearDuplicateStep | Rule::ParameterizationCandidate
            )
        })
        .collect();
    rules.sort();
    rules.dedup();
    (comparable, events, rules)
}

const FLAT_MATCHER_ALIAS: &str = "const negated = api.expect.not;";
const MATCHER_WRITE: &str = "negated.objectContaining = replacement;";

/// Reference behaviour for OPEN-1 / R5190030268-S1, stated on the flat alias.
///
/// A write through an alias of a matcher builder revokes assertion trust, and the handler becomes
/// non-comparable rather than silently keeping a stale result. Both sides of that decision are
/// pinned: a write under a path that is not a matcher path must leave trust intact, so a collector
/// that followed any property would fail here.
#[test]
fn flat_matcher_aliases_revoke_trust_only_for_matcher_paths() {
    let (_directory, config) = config();

    let mutated = aliased_matcher_outcome(FLAT_MATCHER_ALIAS, MATCHER_WRITE, &config);
    assert!(
        mutated.0.iter().all(|comparable| !comparable),
        "a write through a flat alias must revoke trust for every handler: {mutated:?}"
    );
    assert!(
        mutated
            .1
            .iter()
            .all(|events| events.iter().any(|event| event == "assert:unresolved")),
        "revoked trust is reported as the unresolved sentinel: {:?}",
        mutated.1
    );
    assert!(
        mutated.2.is_empty(),
        "a non-comparable handler yields no handler finding: {:?}",
        mutated.2
    );

    // Without the write the same alias stays trusted and still reports, so a change that simply
    // suppressed every aliased assertion would fail here.
    let clean = aliased_matcher_outcome(FLAT_MATCHER_ALIAS, "", &config);
    assert!(
        clean.0.iter().all(|comparable| *comparable),
        "an unmutated alias must leave every handler comparable"
    );
    assert!(
        clean.2.contains(&Rule::DuplicateHandler),
        "an unmutated alias must still report identical handlers: {:?}",
        clean.2
    );

    // The other side of the provenance decision: a write under a path that is not a matcher path
    // must not revoke trust, so a collector that followed *any* property would fail here.
    for alias in [
        // A sibling of the trusted factory: the walk stops before the matcher allowlist.
        "const negated = api.other.not;",
        // A member *under* the trusted factory that is not a matcher: this is what pins the
        // allowlist itself, since a change treating every `api.expect.<property>` as a matcher
        // path would revoke trust here while leaving the sibling case untouched.
        "const negated = api.expect.unknownProperty;",
    ] {
        let other = aliased_matcher_outcome(alias, MATCHER_WRITE, &config);
        assert!(
            other.2.contains(&Rule::DuplicateHandler),
            "a write under `{alias}` is not a matcher path and must not revoke trust: {other:?}"
        );
    }
}

/// Every position of an assertion chain, as `label, dotted, computed, dynamic, event prefix`.
///
/// `VALUE` is substituted per definition. `computed` names the property with a static string and
/// must behave exactly like `dotted`. `dynamic` selects the property at runtime and must
/// never gain assertion trust. The prefix is the trusted event dot access produces, and pins the
/// factory, modifier and matcher identity so a row cannot be satisfied by an event that lost one
/// of them.
const ASSERTION_CHAIN_POSITIONS: [(&str, &str, &str, &str, &str); 6] = [
    (
        "factory",
        "api.expect(state).not.toBe(VALUE)",
        "api['expect'](state).not.toBe(VALUE)",
        "api[factoryKey](state).not.toBe(VALUE)",
        "assert:expect.not#toBe:",
    ),
    (
        "modifier",
        "api.expect(state).not.toBe(VALUE)",
        "api.expect(state)['not'].toBe(VALUE)",
        "api.expect(state)[modifierKey].toBe(VALUE)",
        "assert:expect.not#toBe:",
    ),
    (
        "terminal matcher",
        "api.expect(state).not.toBe(VALUE)",
        "api.expect(state).not['toBe'](VALUE)",
        "api.expect(state).not[matcherKey](VALUE)",
        "assert:expect.not#toBe:",
    ),
    (
        "factory option",
        "api.expect.soft(state).toBe(VALUE)",
        "api.expect['soft'](state).toBe(VALUE)",
        "api.expect[optionKey](state).toBe(VALUE)",
        "assert:expect.soft#toBe:",
    ),
    (
        "nested builder",
        "api.expect(state).toEqual(api.expect.not.objectContaining({ r: VALUE }))",
        "api.expect(state).toEqual(api.expect['not'].objectContaining({ r: VALUE }))",
        "api.expect(state).toEqual(api.expect[modifierKey].objectContaining({ r: VALUE }))",
        "assert:expect#toEqual:",
    ),
    (
        "wrapped modifier",
        "(api.expect(state) as any).not.toBe(VALUE)",
        "(api.expect(state) as any)['not'].toBe(VALUE)",
        "(api.expect(state) as any)[modifierKey].toBe(VALUE)",
        "assert:expect.not#toBe:",
    ),
];

/// Two definitions asserting `expression` with `left` and `right` expected values. Returns each
/// handler's behavior signature and the handler rules the pair produced.
///
/// Per handler rather than first-only: resolving one handler correctly while losing trust on the
/// other must not be able to match a fully resolved baseline.
fn assertion_position_outcome(
    expression: &str,
    (left, right): (&str, &str),
    config: &Config,
) -> (Vec<Vec<String>>, Vec<Rule>) {
    let source = format!(
        "const api = require('@playwright/test'); \
         Then('alpha holds', async ({{ state }}) => {{ {}; }}); \
         Then('alpha stands', async ({{ state }}) => {{ {}; }});",
        expression.replace("VALUE", left),
        expression.replace("VALUE", right)
    );
    let extracted = definitions(&source);
    assert_eq!(extracted.len(), 2, "{source}");
    let events: Vec<Vec<String>> = extracted
        .iter()
        .map(|definition| definition.handler.behavior_signature.clone())
        .collect();
    let result = analyze(extracted, Vec::new(), config).unwrap();
    let mut rules: Vec<Rule> = result
        .findings
        .iter()
        .map(|finding| finding.rule)
        .filter(|rule| {
            matches!(
                rule,
                Rule::DuplicateHandler | Rule::NearDuplicateStep | Rule::ParameterizationCandidate
            )
        })
        .collect();
    rules.sort();
    rules.dedup();
    (events, rules)
}

const CONFLICTING: (&str, &str) = ("'ready'", "'idle'");
const EQUAL: (&str, &str) = ("'ready'", "'ready'");

/// Dot access must resolve every position of an assertion chain to a trusted event that names its
/// factory, modifier and matcher, keep conflicting expected values apart, and still report equal
/// ones. Each position is pinned separately, since resolving one says nothing about the others.
#[test]
fn dotted_access_resolves_every_assertion_position() {
    let (_directory, config) = config();
    for (label, dotted, _, _, prefix) in ASSERTION_CHAIN_POSITIONS {
        let (events, rules) = assertion_position_outcome(dotted, CONFLICTING, &config);
        for handler in &events {
            assert_eq!(
                handler.len(),
                1,
                "dot access at the {label} position is one atomic event: {handler:?}"
            );
            assert!(
                handler[0].starts_with(prefix),
                "dot access at the {label} position must produce `{prefix}…`: {handler:?}"
            );
        }
        assert!(
            rules.is_empty(),
            "dot access at the {label} position must keep conflicting values apart: {rules:?}"
        );

        // Equal values are a genuine duplicate, so a change that merely stopped comparing this
        // position would fail here rather than satisfy the contract test.
        let (_, equal_rules) = assertion_position_outcome(dotted, EQUAL, &config);
        assert!(
            equal_rules.contains(&Rule::DuplicateHandler),
            "dot access at the {label} position must still report equal values: {equal_rules:?}"
        );

        // The subject is part of the assertion at every position.
        let (other_subject, _) =
            assertion_position_outcome(&dotted.replace("state", "other"), CONFLICTING, &config);
        assert_ne!(
            other_subject[0], events[0],
            "the {label} position must distinguish the asserted subject"
        );
    }
}

/// A property selected at runtime is not provably the factory, modifier, option or matcher it
/// happens to hold, so it must not gain assertion trust at any position. Returning the existing
/// `assert:unresolved` sentinel is an acceptable conservative answer; inventing a trusted event is
/// not, so a resolver that trusted whatever a bracket expression happened to name would fail here.
#[test]
fn dynamic_access_gains_no_assertion_trust_at_any_position() {
    let (_directory, config) = config();
    for (label, _, _, dynamic, _) in ASSERTION_CHAIN_POSITIONS {
        let (events, _) = assertion_position_outcome(dynamic, CONFLICTING, &config);
        for handler in &events {
            assert!(
                !handler
                    .iter()
                    .any(|event| event.starts_with("assert:") && event != "assert:unresolved"),
                "a runtime-selected property at the {label} position must not gain trust: {handler:?}"
            );
        }
    }
}

/// A modifier chain is ordered and semantic: `not`, the promise modifiers, and the supported
/// factory options each change what an assertion means, and `not.resolves` is not `resolves.not`.
/// A change that sorted, deduplicated or folded the chain would leave every existing test passing.
///
/// Transparent TypeScript wrappers and `await` are the opposite: they carry no meaning and must
/// leave the assertion unchanged.
#[test]
fn modifier_chains_are_ordered_and_semantic() {
    let (_directory, config) = config();

    let events = |expression: &str| {
        let (events, _) = assertion_position_outcome(expression, EQUAL, &config);
        events[0].clone()
    };
    let chain_events = |chain: &str| events(&format!("api.expect(state){chain}.toBe(VALUE)"));

    // Every distinct chain is a distinct trusted assertion. Collected rather than compared
    // pairwise so a collision names both chains.
    let mut seen: Vec<(&str, Vec<String>)> = Vec::new();
    for chain in [
        "",
        ".not",
        ".not.not",
        ".resolves",
        ".rejects",
        ".not.resolves",
        ".resolves.not",
    ] {
        let produced = chain_events(chain);
        assert_eq!(
            produced.len(),
            1,
            "`expect(state){chain}` is one atomic event: {produced:?}"
        );
        assert!(
            produced[0].starts_with("assert:expect"),
            "`expect(state){chain}` must be a trusted assertion: {produced:?}"
        );
        if let Some((other, _)) = seen.iter().find(|(_, events)| *events == produced) {
            panic!(
                "`expect(state){chain}` and `expect(state){other}` share an event: {produced:?}"
            );
        }
        seen.push((chain, produced));
    }

    // A nested asymmetric builder contributes to the expected value, so its own modifier must be
    // part of the assertion. The outer `assert:expect#toEqual:` prefix carries no information about
    // it, so dropping the nested `not` would otherwise go unnoticed.
    assert_ne!(
        events("api.expect(state).toEqual(api.expect.not.objectContaining({ r: VALUE }))"),
        events("api.expect(state).toEqual(api.expect.objectContaining({ r: VALUE }))"),
        "a negated nested builder must not match the same builder without the modifier"
    );

    // `poll` is a supported factory option and names itself in the chain.
    let polled = events("api.expect.poll(() => state, { timeout: 1 }).toBe(VALUE)");
    assert!(
        polled[0].starts_with("assert:expect.poll#"),
        "a polled assertion must record the option: {polled:?}"
    );

    // Final outcome, not only event inequality: near-identical wording must not equate two steps
    // whose only difference is the modifier chain.
    for (left, right) in [
        (".not", ".resolves"),
        (".not.resolves", ".resolves.not"),
        (".not", ".not.not"),
    ] {
        let source = format!(
            "const api = require('@playwright/test'); \
             Then('the alpha row is aligned', async ({{ state }}) => {{ api.expect(state){left}.toBe('ready'); }}); \
             Then('the beta row is aligned', async ({{ state }}) => {{ api.expect(state){right}.toBe('ready'); }});"
        );
        let extracted = definitions(&source);
        assert_eq!(extracted.len(), 2, "{source}");
        let result = analyze(extracted, Vec::new(), &config).unwrap();
        assert!(
            !result.findings.iter().any(|finding| matches!(
                finding.rule,
                Rule::DuplicateHandler | Rule::NearDuplicateStep | Rule::ParameterizationCandidate
            )),
            "`{left}` and `{right}` differ in meaning and must not be equated: {:?}",
            result.findings
        );
    }

    // Transparent wrappers and `await` carry no meaning, so they must not change the assertion.
    let plain = chain_events(".not");
    for equivalent in [
        "(api.expect(state) as any).not.toBe(VALUE)",
        "(api.expect(state))!.not.toBe(VALUE)",
        "((api.expect(state) as any)! satisfies Function).not.toBe(VALUE)",
        "await api.expect(state).not.toBe(VALUE)",
    ] {
        assert_eq!(
            events(equivalent),
            plain,
            "`{equivalent}` must resolve to the same assertion as dot access"
        );
    }
}

/// OPEN-2 / R5190030268-S2. A statically computed modifier names the same modifier as dot access,
/// so `expect(x)['not']` must carry the semantics `expect(x).not` carries: conflicting expected
/// values stay distinguishable, and equal values remain an exact duplicate. A modifier selected at
/// runtime is unknown and must not inherit that trust.
///
/// Currently the computed form yields generic call events whose literals are normalized away, so
/// conflicting values collapse into one `parameterization-candidate`.
///
/// Comparing finding sets alone is not enough: unwrapping `['not']` while discarding the modifier
/// would still distinguish differing values and still equate identical handlers. The event
/// assertions below pin the modifier itself, so dropping negation fails this test.
#[test]
fn computed_modifier_access_matches_dot_access_semantics() {
    let (_directory, config) = config();

    // Behavior events for a single definition asserting on `subject` using `access` before
    // `.toBe(value)`. The subject is a parameter so that erasing it from the event is observable.
    let events = |subject: &str, access: &str, value: &str| {
        let source =
            format!("Then('alpha holds', () => {{ expect({subject}){access}.toBe({value}); }});");
        let extracted = definitions(&source);
        assert_eq!(extracted.len(), 1, "{source}");
        extracted[0].handler.behavior_signature.clone()
    };

    // Handler rules only: `unused-definition` depends on a feature corpus these snippets lack.
    let outcome = |access: &str, left: &str, right: &str| {
        let source = format!(
            "Then('alpha holds', () => {{ expect(state){access}.toBe({left}); }}); \
             Then('alpha stands', () => {{ expect(state){access}.toBe({right}); }});"
        );
        let extracted = definitions(&source);
        assert_eq!(extracted.len(), 2, "{source}");
        let result = analyze(extracted, Vec::new(), &config).unwrap();
        let mut rules: Vec<Rule> = result
            .findings
            .iter()
            .map(|finding| finding.rule)
            .filter(|rule| {
                matches!(
                    rule,
                    Rule::DuplicateHandler
                        | Rule::NearDuplicateStep
                        | Rule::ParameterizationCandidate
                )
            })
            .collect();
        rules.sort();
        rules.dedup();
        rules
    };

    // The dotted form is the reference. Anchor it absolutely, so a change that drops the modifier
    // from both access forms cannot satisfy the equality below by making them agree on nothing.
    let dotted_negated = events("state", ".not", "'ready'");
    assert!(
        dotted_negated
            .iter()
            .any(|event| event.starts_with("assert:expect.not#toBe:")),
        "dot access must record the negation modifier: {dotted_negated:?}"
    );

    // Negation is an execution effect: a negated assertion must never look like the unmodified one.
    assert_ne!(
        dotted_negated,
        events("state", "", "'ready'"),
        "a negated assertion must not match the same assertion without the modifier"
    );
    assert_ne!(
        events("state", "['not']", "'ready'"),
        events("state", "", "'ready'"),
        "a computed negation must not be reduced to the unmodified assertion"
    );

    // The computed modifier must produce the same events as dot access, modifier included.
    assert_eq!(
        events("state", "['not']", "'ready'"),
        dotted_negated,
        "a static computed modifier must produce the same assertion events as dot access"
    );

    for (left, right, expected) in [
        ("'ready'", "'idle'", Vec::new()),
        ("'ready'", "'ready'", vec![Rule::DuplicateHandler]),
    ] {
        let dotted = outcome(".not", left, right);
        assert_eq!(
            dotted, expected,
            "dot access baseline changed for {left}/{right}"
        );
        assert_eq!(
            outcome("['not']", left, right),
            dotted,
            "a static computed modifier must match dot access for {left}/{right}"
        );
    }

    // Control: the modifier is chosen at runtime, so it is not provably `not`. Emitting the
    // unresolved sentinel is a conservative, contract-compliant answer here — it makes the handler
    // non-comparable rather than claiming a modifier — so only a *trusted* assertion event is
    // forbidden.
    let dynamic = events("state", "[modifier]", "'ready'");
    assert!(
        !dynamic
            .iter()
            .any(|event| event.starts_with("assert:") && event != "assert:unresolved"),
        "a runtime-selected modifier must not produce a trusted assertion event: {dynamic:?}"
    );
    // Forbidding only `DuplicateHandler` while supplying differing literals cannot fail: differing
    // literals already preclude that rule whatever the modifier means, so the check passes without
    // observing the modifier at all. Pin the whole rule set for both pairings instead, which holds
    // the runtime form to a stated outcome rather than to a condition it satisfies for free.
    assert_eq!(
        outcome("[modifier]", "'ready'", "'idle'"),
        vec![Rule::ParameterizationCandidate],
        "an unresolved modifier falls back to structural similarity for differing values"
    );
    assert_eq!(
        outcome("[modifier]", "'ready'", "'ready'"),
        vec![Rule::DuplicateHandler],
        "handlers identical in source stay duplicates however the modifier is spelled"
    );
}

/// OPEN-1 / R5190030268-S1. A destructuring pattern that binds a matcher builder through nested
/// levels aliases the same module object as the equivalent flat member access, so a write through
/// either alias must revoke assertion trust identically.
///
/// Today only the flat alias revokes it: the pattern collector records an identifier at the first
/// supported property level, so nested bindings never reach the owner and the write is invisible.
///
/// Each scenario is asserted as nested-equals-flat rather than against an absolute value, because
/// the consistency requirement is the contract and the product has not decided every absolute
/// answer. `flat_matcher_aliases_revoke_trust_only_for_matcher_paths` anchors the flat side, so
/// these equalities cannot be satisfied by both forms degrading together.
#[test]
fn nested_destructuring_aliases_revoke_matcher_trust_like_flat_aliases() {
    let (_directory, config) = config();
    let flat_mutated = aliased_matcher_outcome(FLAT_MATCHER_ALIAS, MATCHER_WRITE, &config);
    let flat_clean = aliased_matcher_outcome(FLAT_MATCHER_ALIAS, "", &config);

    for pattern in [
        "const { expect: { not: negated } } = api;",
        "const { expect: { not: negated } = {} } = api;",
        // A computed key that resolves statically names the same property as the plain key.
        "const { expect: { ['not']: negated } } = api;",
        // A computed key that does not resolve statically may still name it. Unknown provenance
        // must remove trust rather than keep it, so this form cannot be treated as an unrelated
        // property either.
        "const { expect: { ['n' + 'ot']: negated } } = api;",
        // The same applies to escapes the decoder does not read: an escaped identifier and a
        // legacy numeric escape both still name `not`, and the flat path already treats them as
        // unknown rather than unrelated.
        "const { expect: { n\\u006ft: negated } } = api;",
        "const { expect: { ['\\156ot']: negated } } = api;",
    ] {
        assert_eq!(
            aliased_matcher_outcome(pattern, MATCHER_WRITE, &config),
            flat_mutated,
            "a write through `{pattern}` must revoke trust like the flat alias"
        );
        assert_eq!(
            aliased_matcher_outcome(pattern, "", &config),
            flat_clean,
            "`{pattern}` without a write must stay trusted like the flat alias"
        );
    }

    // Binding forms whose local name is not `negated`: a shorthand, a shorthand carrying a default,
    // an escaped shorthand, and a rest binding. Object rest copies the property values, so `copy`
    // holds the very matcher objects the owner holds and a write through it reaches them.
    for (pattern, write) in [
        (
            "const { expect: { not } } = api;",
            "not.objectContaining = replacement;",
        ),
        (
            "const { expect: { not = {} } } = api;",
            "not.objectContaining = replacement;",
        ),
        (
            "const { expect: { n\\u006ft } } = api;",
            "n\\u006ft.objectContaining = replacement;",
        ),
        (
            "const { expect: { ...copy } } = api;",
            "copy.not.objectContaining = replacement;",
        ),
    ] {
        assert_eq!(
            aliased_matcher_outcome(pattern, write, &config),
            flat_mutated,
            "a write through `{pattern}` must revoke trust like the flat alias"
        );
        assert_eq!(
            aliased_matcher_outcome(pattern, "", &config),
            flat_clean,
            "`{pattern}` without a write must stay trusted like the flat alias"
        );
    }

    // Bindings are tracked by spelling: nothing in this analyzer decodes identifier escapes, so a
    // declaration and a write that spell one identifier differently do not match. That is a
    // limitation rather than a judgement about provenance, and normalizing it in the nested
    // collector alone would disagree with registration and shadowing and lose real mutations. Pin
    // it as nested-equals-flat so the two forms cannot drift apart while it stands.
    for (nested, flat, write) in [
        (
            "const { expect: { n\\u006ft } } = api;",
            "const n\\u006ft = api.expect.not;",
            "not.objectContaining = replacement;",
        ),
        (
            "const { expect: { not } } = api;",
            "const not = api.expect.not;",
            "n\\u006ft.objectContaining = replacement;",
        ),
    ] {
        assert_eq!(
            aliased_matcher_outcome(nested, write, &config),
            aliased_matcher_outcome(flat, write, &config),
            "`{nested}` with `{write}` must behave like the equivalent flat alias"
        );
    }

    // The nested collector must reach every property the flat walker reaches, not only `not`.
    assert_eq!(
        aliased_matcher_outcome(
            "const { expect: { not: { objectContaining: negated } } } = api;",
            "negated.any = replacement;",
            &config
        ),
        aliased_matcher_outcome(
            "const negated = api.expect.not.objectContaining;",
            "negated.any = replacement;",
            &config
        ),
        "a matcher-builder path must be treated the same through both alias forms"
    );

    // The recursion bound is a stack guard, not a judgement about provenance. A pattern nested
    // past it must still revoke, or the bound would quietly become a way to keep trust.
    let beyond_bound = {
        let mut pattern = "negated".to_owned();
        for _ in 0..24 {
            pattern = format!("{{ ['n' + 'ot']: {pattern} }}");
        }
        format!("const {{ expect: {pattern} }} = api;")
    };
    assert_eq!(
        aliased_matcher_outcome(&beyond_bound, MATCHER_WRITE, &config),
        flat_mutated,
        "a pattern nested past the recursion bound must still revoke trust"
    );

    // A variable that appears only as a computed key is not a binding, so a write through it must
    // not revoke trust. Nested past the bound so the fallback walker is the one deciding.
    let beyond_bound_key = {
        let mut pattern = "{ [marker]: { deep: leaf } }".to_owned();
        for _ in 0..20 {
            pattern = format!("{{ ['n' + 'ot']: {pattern} }}");
        }
        format!("const marker = 'k'; const {{ expect: {pattern} }} = api;")
    };
    assert_eq!(
        aliased_matcher_outcome(
            &beyond_bound_key,
            "marker.objectContaining = replacement;",
            &config
        ),
        flat_clean,
        "a variable used only as a computed key must not revoke trust"
    );
    assert_eq!(
        aliased_matcher_outcome(
            &beyond_bound_key,
            "leaf.objectContaining = replacement;",
            &config
        ),
        flat_mutated,
        "a binding past the recursion bound must still revoke trust"
    );

    // A write to an unrelated property of the aliased object is the same question on both forms,
    // whatever it is decided to mean, so consistency is asserted without fixing the answer.
    const UNRELATED_WRITE: &str = "negated.unrelatedProperty = replacement;";
    assert_eq!(
        aliased_matcher_outcome(
            "const { expect: { not: negated } } = api;",
            UNRELATED_WRITE,
            &config
        ),
        aliased_matcher_outcome(FLAT_MATCHER_ALIAS, UNRELATED_WRITE, &config),
        "an unrelated write must be treated the same through both alias forms"
    );

    // The same nesting shape under a property that is not a matcher path must also agree, so an
    // over-broad collector that aliased identifiers beneath any property fails rather than passes.
    assert_eq!(
        aliased_matcher_outcome(
            "const { other: { not: negated } } = api;",
            MATCHER_WRITE,
            &config
        ),
        aliased_matcher_outcome("const negated = api.other.not;", MATCHER_WRITE, &config),
        "an alias under an unrelated property must be treated the same through both forms"
    );
}

/// OPEN-2 / R5190030268-S2, generalized across assertion positions. A statically computed property
/// names the same thing as dot access at every position of an assertion chain, so each must carry
/// the same semantics for both conflicting and equal expected values.
///
/// Every divergence is collected before asserting, so the failure names each unsupported position
/// instead of stopping at the first. At the current tree the factory and nested-builder positions
/// already agree.
#[test]
fn computed_access_matches_dot_access_at_every_position() {
    let (_directory, config) = config();
    let mut diverging = Vec::new();
    for (label, dotted, computed, _, _) in ASSERTION_CHAIN_POSITIONS {
        for values in [CONFLICTING, EQUAL] {
            if assertion_position_outcome(computed, values, &config)
                != assertion_position_outcome(dotted, values, &config)
            {
                diverging.push(label);
                break;
            }
        }
    }
    assert!(
        diverging.is_empty(),
        "computed access must match dot access, but diverges at: {diverging:?}"
    );
}

/// A direct property read of a required assertion module is the factory, in either spelling, so
/// `require('...')['expect']` must carry exactly the trust `require('...').expect` carries.
///
/// The dotted spelling is anchored absolutely, so the equalities below cannot be satisfied by both
/// spellings failing to resolve together.
#[test]
fn a_computed_require_property_is_an_assertion_factory() {
    let (_directory, config) = config();
    let outcome = |spelling: &str, right: &str| {
        let source = format!(
            "const check = {spelling}; \
             Then('the panel reads the first value', () => {{ check(state).not.toBe('ready'); }}); \
             Then('the panel reads the final value', () => {{ check(state).not.toBe({right}); }});"
        );
        let extracted = definitions(&source);
        assert_eq!(extracted.len(), 2, "{source}");
        let events = extracted[0].handler.behavior_signature.clone();
        let result = analyze(extracted, Vec::new(), &config).unwrap();
        let mut rules: Vec<Rule> = result
            .findings
            .iter()
            .map(|finding| finding.rule)
            .filter(|rule| {
                matches!(
                    rule,
                    Rule::DuplicateHandler
                        | Rule::NearDuplicateStep
                        | Rule::ParameterizationCandidate
                )
            })
            .collect();
        rules.sort();
        rules.dedup();
        (events, rules)
    };
    let dotted = "require('@playwright/test').expect";
    let computed = "require('@playwright/test')['expect']";

    let (anchored, _) = outcome(dotted, "'idle'");
    assert!(
        anchored
            .iter()
            .any(|event| event.starts_with("assert:expect.not#toBe:")),
        "the dotted spelling must resolve to a trusted assertion: {anchored:?}"
    );

    for right in ["'idle'", "'ready'"] {
        assert_eq!(
            outcome(computed, right),
            outcome(dotted, right),
            "`{computed}` must carry the trust `{dotted}` carries for {right}"
        );
    }
}

/// A subscript index names a property only when it is a static string literal. Deciding that from
/// the index text alone is unsound: any expression whose text merely begins and ends with a
/// matching quote decodes into a key that names nothing, which both invents a modifier and hides
/// whatever the index actually executes.
///
/// The resolved case is anchored absolutely, so the prohibitions below cannot be satisfied by
/// making every subscript unresolved.
#[test]
fn quote_shaped_subscript_indexes_do_not_name_a_property() {
    let (_directory, config) = config();
    let outcome = |index: &str| {
        let handler = format!("expect(state)[{index}].toBe('ready');");
        let source = format!(
            "Then('the alpha gauge is settled', () => {{ {handler} }}); \
             Then('the archive reading has finished', () => {{ {handler} }});"
        );
        let extracted = definitions(&source);
        assert_eq!(extracted.len(), 2, "{source}");
        let events = extracted[0].handler.behavior_signature.clone();
        let comparable = extracted[0].handler.comparable;
        let result = analyze(extracted, Vec::new(), &config).unwrap();
        let mut rules: Vec<Rule> = result
            .findings
            .iter()
            .map(|finding| finding.rule)
            .filter(|rule| {
                matches!(
                    rule,
                    Rule::DuplicateHandler
                        | Rule::NearDuplicateStep
                        | Rule::ParameterizationCandidate
                )
            })
            .collect();
        rules.sort();
        rules.dedup();
        (events, comparable, rules)
    };

    let (resolved, _, _) = outcome("'not'");
    assert!(
        resolved
            .iter()
            .any(|event| event.starts_with("assert:expect.not#toBe:")),
        "a string-literal index must resolve to the modifier it names: {resolved:?}"
    );

    // A backslash before U+2028 or U+2029 is a line continuation, so this key names `not` exactly
    // as `'not'` does. Preserving the separator instead would lose that name and, worse, make the
    // key indistinguishable from one that genuinely contains the separator.
    let (continued, _, _) = outcome("'n\\\u{2028}ot'");
    assert_eq!(
        continued, resolved,
        "a line continuation inside a key must resolve to the name the key spells"
    );
    let (separator, _, _) = outcome("'n\u{2028}ot'");
    assert_ne!(
        separator, resolved,
        "a key that genuinely contains the separator names a different property"
    );

    // An escaped backslash is not the start of an escape: `'foo\\5'` names `foo\5`, which reads
    // fine. Only a digit that an escape actually consumes makes a key unreadable.
    let (escaped_backslash, _, _) = outcome(r"'foo\\5'");
    assert!(
        escaped_backslash
            .iter()
            .any(|event| event.starts_with("assert:expect.")),
        "an escaped backslash must not make a key unreadable: {escaped_backslash:?}"
    );

    // An identifier may carry unicode escapes, and a decoded key may contain a literal backslash.
    // They name different properties, so encoding must keep them apart: `x.foobar` reads
    // `foobar`, while `x['foo\\u0062ar']` reads a property whose name contains a backslash.
    let (computed_backslash, _, _) = outcome(r"'foo\\u0062ar'");
    let escaped_identifier = {
        let source = "Then('alpha holds', () => { expect(state).foo\\u0062ar.toBe('ready'); });";
        let extracted = definitions(source);
        assert_eq!(extracted.len(), 1, "{source}");
        extracted[0].handler.behavior_signature.clone()
    };
    assert!(
        escaped_identifier
            .iter()
            .any(|event| event.starts_with("assert:expect.")),
        "an escaped identifier must still resolve: {escaped_identifier:?}"
    );
    assert_ne!(
        computed_backslash, escaped_identifier,
        "a key containing a backslash must not encode to the same event as an escaped identifier"
    );

    // A decoded key is arbitrary text, while event components are joined with delimiters. A key
    // containing one of them must stay distinguishable from the join, or a single property named
    // `not.resolves` would be recorded as the two-step chain `.not.resolves`.
    let (dotted_key, _, _) = outcome("'not.resolves'");
    let (chain, _, _) = {
        let source = "Then('the alpha gauge is settled', () => { \
                      expect(state).not.resolves.toBe('ready'); }); \
                      Then('the archive reading has finished', () => { \
                      expect(state).not.resolves.toBe('ready'); });";
        let extracted = definitions(source);
        assert_eq!(extracted.len(), 2, "{source}");
        (extracted[0].handler.behavior_signature.clone(), true, ())
    };
    assert!(
        chain
            .iter()
            .any(|event| event.starts_with("assert:expect.not.resolves#toBe:")),
        "the modifier chain must resolve, anchoring the comparison below: {chain:?}"
    );
    assert_ne!(
        dotted_key, chain,
        "a property named `not.resolves` must not record the same event as the chain `.not.resolves`"
    );

    // The sequence index executes an assertion against an external binding. Decoding the index as
    // one opaque key swallows that call, which turns a handler that cannot be compared into one
    // that looks fully resolved. Compare against the same handler written without the index so the
    // control states what the difference must be rather than asserting an absolute.
    const SWALLOWED: &str = "'not', ((x) => expect(state).toBe(x))(external), 'not'";
    let (events, comparable, rules) = outcome(SWALLOWED);
    assert!(
        !comparable,
        "an index that executes an unresolved call must leave the handler non-comparable: \
         {events:?}"
    );
    assert!(
        rules.is_empty(),
        "a non-comparable handler must not be equated with another, but produced: {rules:?}"
    );

    for index in [
        "'n' + 'ot'",
        "`not`",
        "'not', ((x) => expect(state).toBe(x))(external), 'not'",
        // A legacy numeric escape is a string literal the shared decoder cannot read: `'n\\157t'`
        // names `not`, not `n157t`. Answering with the undecoded digits would name a property the
        // source never mentions, so it must stay unreadable rather than resolve to the wrong one.
        "'n\\157t'",
    ] {
        let (events, _, _) = outcome(index);
        // Reject every trusted assertion event, not only a modified one: resolving the index to
        // nothing and emitting the unmodified `assert:expect#toBe:` would still be claiming the
        // chain was understood.
        assert!(
            !events
                .iter()
                .any(|event| event.starts_with("assert:") && event != "assert:unresolved"),
            "`[{index}]` does not name a modifier, but produced: {events:?}"
        );
    }
}
