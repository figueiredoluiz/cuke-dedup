use super::*;
use crate::source_adapter::{SourceExtractionSession, SourceFile};
use crate::tests::property_runner;
use crate::typescript::{TypeScriptExtractionSession, TYPESCRIPT};
use proptest::prelude::*;

/// Creates an actual adapter session with test-local module-byte and resolution-state allowances.
fn session(root: &Path, bytes: usize, states: usize) -> SourceExtractionSession {
    let mut session = SourceExtractionSession::new(root);
    let resolver = &mut session
        .state::<TypeScriptExtractionSession>()
        .unwrap()
        .resolver;
    resolver.state.module_bytes = WorkBudget::new(bytes);
    resolver.state.resolutions_started = WorkBudget::new(states);
    session
}

type Input = (usize, usize, Vec<usize>);

/// Replays immutable roots and resolver operations without any input generation.
fn check((units, allowance, ops): &Input) -> proptest::test_runner::TestCaseResult {
    let project = tempfile::tempdir().unwrap();
    let roots = [project.path().join("first"), project.path().join("second")];
    let names = ["Given", "Then"];
    let mut sources = Vec::new();
    for (root, name) in roots.iter().zip(names) {
        fs::create_dir(root).unwrap();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::write(root.join("steps.ts"), "").unwrap();
        let modules: Vec<_> = (0..3)
            .map(|i| {
                format!(
                    "export {{ {name} as register }} from '@cucumber/cucumber';\n//{}",
                    "x".repeat(i * 7)
                )
            })
            .collect();
        for (i, source) in modules.iter().enumerate() {
            fs::write(root.join(format!("m{i}.ts")), source).unwrap();
        }
        sources.push(modules);
    }
    let limits = [units * sources[0][0].len(), units * sources[1][0].len()];
    let mut sessions = [
        session(&roots[0], limits[0], *allowance),
        session(&roots[1], limits[1], *allowance),
    ];
    let mut cached = [BTreeSet::new(), BTreeSet::new()];
    let mut spent = [[0usize; 2]; 2];
    for op in ops {
        let root = op % 2;
        let module = (op / 2) % 3;
        if *op >= 12 {
            sessions[root] = session(&roots[root], limits[root], *allowance);
            cached[root].clear();
            spent[root] = [0, 0];
        }
        let mut fits = cached[root].contains(&module);
        if !fits && spent[root][1] < *allowance {
            spent[root][1] += 1;
            if spent[root][0] + sources[root][module].len() <= limits[root] {
                spent[root][0] += sources[root][module].len();
                cached[root].insert(module);
                fits = true;
            }
        }
        let resolver = &mut sessions[root]
            .state::<TypeScriptExtractionSession>()
            .unwrap()
            .resolver;
        let outcome = resolver
            .registration_exports(&roots[root].join("steps.ts"), &format!("./m{module}"))
            .unwrap();
        prop_assert_eq!(outcome.resolution.is_some(), fits);
        prop_assert_eq!(outcome.reason.is_some(), !fits);
        if let Some(exports) = outcome.resolution {
            prop_assert_eq!(
                exports
                    .exports
                    .get("register")
                    .map(|e| e.canonical.as_str()),
                Some(names[root])
            );
        }
        prop_assert_eq!(resolver.state.module_bytes.spent(), spent[root][0]);
        prop_assert_eq!(resolver.state.resolutions_started.spent(), spent[root][1]);
        prop_assert_eq!(resolver.state.modules.len(), cached[root].len());
        prop_assert!(resolver.state.active.is_empty());
    }
    // Fresh real sessions must resolve both roots even when every bounded attempt refused.
    for root in 0..2 {
        let mut fresh = SourceExtractionSession::new(&roots[root]);
        let resolver = &mut fresh
            .state::<TypeScriptExtractionSession>()
            .unwrap()
            .resolver;
        let outcome = resolver
            .registration_exports(&roots[root].join("steps.ts"), "./m0")
            .unwrap()
            .resolution
            .unwrap();
        prop_assert_eq!(
            outcome
                .exports
                .get("register")
                .map(|e| e.canonical.as_str()),
            Some(names[root])
        );
    }
    Ok(())
}

/// Models immutable per-root module snapshots, repeated cache hits and independent session resets.
#[test]
fn generated_source_sessions_isolate_cached_exports_and_consumed_work() {
    property_runner::generated(
        (
            0usize..=4,
            0usize..=4,
            proptest::collection::vec(0usize..=15, 0..=16),
        ),
        check,
    );
}

/// The public adapter must honor stored limits, retain cached successes and preserve independent positives.
#[test]
fn adapter_session_callers_preserve_limits_and_fresh_positive_controls() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("package.json"), "{}").unwrap();
    let module = "export { Given as register } from '@cucumber/cucumber';";
    for i in 0..2 {
        fs::write(root.path().join(format!("m{i}.ts")), module).unwrap();
    }
    let extract = |session: &mut SourceExtractionSession, i| {
        let path = root.path().join(format!("steps{i}.ts"));
        let source = format!("import {{ register }} from './m{i}'; import {{ Given }} from '@cucumber/cucumber'; register('target',()=>work()); Given('control',()=>work()); Given('control',()=>work());");
        crate::source_adapter::adapter_for_language(TYPESCRIPT)
            .unwrap()
            .extract_with_session(
                &source,
                &SourceFile {
                    path,
                    language: TYPESCRIPT,
                },
                session,
            )
            .unwrap()
    };
    let mut shared = session(root.path(), 2 * module.len(), 1);
    for i in [0, 0, 1] {
        let extraction = extract(&mut shared, i);
        assert_eq!(extraction.definitions.len(), if i == 0 { 3 } else { 2 });
        assert_eq!(
            extraction
                .definitions
                .iter()
                .filter(|d| d.matcher == "control")
                .count(),
            2
        );
        assert_eq!(extraction.diagnostics.is_empty(), i == 0);
        if i == 1 {
            assert!(extraction
                .diagnostics
                .iter()
                .any(|d| d.message.contains("state resolution limit")));
        }
    }
    assert_eq!(
        shared
            .state::<TypeScriptExtractionSession>()
            .unwrap()
            .resolver
            .state
            .resolutions_started
            .spent(),
        1
    );
    let mut refused = session(root.path(), 0, 4);
    let result = extract(&mut refused, 0);
    assert_eq!(result.definitions.len(), 2);
    assert!(!result.diagnostics.is_empty());
    assert!(result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("byte aggregate resolution limit")));
    assert_eq!(
        refused
            .state::<TypeScriptExtractionSession>()
            .unwrap()
            .resolver
            .state
            .module_bytes
            .spent(),
        0
    );
    let mut fresh = session(root.path(), module.len(), 1);
    let result = extract(&mut fresh, 1);
    assert_eq!(result.definitions.len(), 3);
    assert!(result.diagnostics.is_empty());
}
