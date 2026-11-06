use cuke_dedup::analysis::analyze;
use cuke_dedup::config::{Config, ConfigOverrides};
use cuke_dedup::model::Rule;
use cuke_dedup::source_adapter::SourceFile;
use cuke_dedup::typescript::{extract_detailed, TYPESCRIPT};

/// Exercises final handler findings across callable forms, bindings and conflicting effects.
#[test]
fn identifier_collection_preserves_handler_findings() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::load(directory.path(), ConfigOverrides::default()).unwrap();
    let file = SourceFile {
        path: directory.path().join("steps.ts"),
        language: TYPESCRIPT,
    };
    let cases = [
        (
            "arrow",
            "(z) => { const a = work(z); consume(a); }",
            "(x) => { const y = work(x); consume(y); }",
            true,
        ),
        (
            "function",
            "function(z) { var z; consume(z); }",
            "function(x) { var x; consume(x); }",
            true,
        ),
        (
            "generator",
            "function*(z) { yield consume(z); }",
            "function*(x) { yield consume(x); }",
            true,
        ),
        (
            "destructured",
            "({key: z = seed()}, [a, ...b]) => consume(z, a, b)",
            "({key: x = seed()}, [y, ...w]) => consume(x, y, w)",
            true,
        ),
        (
            "typed",
            "(z: string) => consume(z)",
            "(x: string) => consume(x)",
            true,
        ),
        (
            "single parameter",
            "z => consume(z)",
            "x => consume(x)",
            true,
        ),
        (
            "local pattern",
            "(z) => { const {key: a, ...b} = z; consume(a, b); }",
            "(x) => { const {key: y, ...w} = x; consume(y, w); }",
            true,
        ),
        (
            "shadowed assertion",
            "(expect, z) => expect(z).toBeVisible()",
            "(check, x) => check(x).toBeVisible()",
            true,
        ),
        (
            "nested",
            "(z) => { let a = work(z); { let z = work(a); consume(z); } }",
            "(x) => { let y = work(x); { let x = work(y); consume(x); } }",
            true,
        ),
        (
            "external defaults",
            "(z = first()) => consume(z)",
            "(x = second()) => consume(x)",
            false,
        ),
        (
            "external locals",
            "() => { const z = first(); consume(z); }",
            "() => { const x = second(); consume(x); }",
            false,
        ),
        (
            "trusted assertion",
            "(z) => expect(z).toBeVisible()",
            "(x) => expect(x).toBeVisible()",
            true,
        ),
        (
            "opposite assertions",
            "(z) => expect(z).toBeVisible()",
            "(x) => expect(x).not.toBeVisible()",
            false,
        ),
    ];
    for import in [
        "import { Given } from '@cucumber/cucumber';",
        "const { Given } = require('@cucumber/cucumber');",
    ] {
        for (name, left, right, duplicate) in cases {
            let source = format!(
                "{import}\nGiven('first operation', {left});\nGiven('second operation', {right});"
            );
            let extracted = extract_detailed(&source, &file).unwrap();
            assert_eq!(extracted.definitions.len(), 2, "{name}: {import}");
            assert!(
                extracted.diagnostics.is_empty(),
                "{name}: {:?}",
                extracted.diagnostics
            );
            if name == "shadowed assertion" {
                assert!(extracted.definitions.iter().all(|d| d
                    .handler
                    .behavior_signature
                    .iter()
                    .all(|e| !e.starts_with("assert:"))));
            }
            if matches!(name, "trusted assertion" | "opposite assertions") {
                assert!(extracted.definitions.iter().all(|d| d
                    .handler
                    .behavior_signature
                    .iter()
                    .any(|e| e.starts_with("assert:"))));
            }
            let result = analyze(extracted.definitions, Vec::new(), &config).unwrap();
            assert_eq!(
                result
                    .findings
                    .iter()
                    .any(|f| f.rule == Rule::DuplicateHandler),
                duplicate,
                "{name}: {import}: {:?}",
                result.findings
            );
            if !duplicate {
                assert!(
                    !result.findings.iter().any(|f| matches!(
                        f.rule,
                        Rule::NearDuplicateStep | Rule::ParameterizationCandidate
                    )),
                    "{name}: {:?}",
                    result.findings
                );
            }
        }
    }
    let source = "import { Given } from 'playwright-bdd/decorators'; class Steps { @Given('first operation') first(z) { const a = work(z); consume(a); } @Given('second operation') second(x) { const y = work(x); consume(y); } }";
    let extracted = extract_detailed(source, &file).unwrap();
    assert_eq!(extracted.definitions.len(), 2);
    assert!(extracted.diagnostics.is_empty());
    let result = analyze(extracted.definitions, Vec::new(), &config).unwrap();
    assert!(result
        .findings
        .iter()
        .any(|f| f.rule == Rule::DuplicateHandler));
}
