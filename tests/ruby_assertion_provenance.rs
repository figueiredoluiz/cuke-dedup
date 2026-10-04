use assert_cmd::Command;
use serde_json::Value;
use std::fs;

const PROVIDER: &str = include_str!(
    "../fixtures/ruby-parity/equivalent-handlers/precision-inline-local/providers/assertions.rb"
);

#[test]
fn closed_assertion_origins_preserve_opposing_and_equivalent_outcomes() {
    for (binding, configured, expected_parameterization) in [
        ("check = SyntheticAssertions.method(:expect)", true, false),
        ("check = SyntheticAssertions.method(:expect)", false, true),
        (
            "check = SyntheticAssertions.api.fetch(:expect)",
            true,
            false,
        ),
        (
            "check = SyntheticAssertions.api.fetch(:expect)",
            false,
            true,
        ),
        (
            "api = SyntheticAssertions.api; check = api[:expect]",
            true,
            false,
        ),
        (
            "first = SyntheticAssertions.api[:expect]; check = first",
            true,
            false,
        ),
    ] {
        let root = project(PROVIDER);
        fs::write(
            root.path().join("steps.rb"),
            format!(
                "require_relative 'provider'\n{binding}\nGiven('the gauge reads a first value') {{ check.call(gauge()).to_be(1) }}\nGiven('the gauge reads a second value') {{ check.call(gauge()).to_be(2) }}\nGiven('the badge is visible') {{ check.call(badge()).to_be_visible }}\nGiven('the badge is now visible') {{ check.call(badge()).to_be_visible }}\n"
            ),
        )
        .unwrap();
        fs::write(
            root.path().join(".cuke-dedup.json"),
            serde_json::json!({
                "threshold": 100,
                "rules": {"unused-definition": "off"},
                "assertionModules": if configured { vec!["provider"] } else { vec![] },
            })
            .to_string(),
        )
        .unwrap();
        let rows = analyze(root.path(), "steps.rb");
        let label = format!("binding={binding}; configured={configured}");
        let summary = rows.last().unwrap();
        assert_eq!(summary["summary"]["definitionsAnalyzed"], 4, "{label}");
        assert_eq!(summary["corpus"]["incomplete"], false, "{label}");
        assert_eq!(
            rows.iter()
                .any(|row| row["rule"] == "parameterization-candidate"),
            expected_parameterization,
            "{label}"
        );
        assert!(
            rows.iter().any(|row| row["rule"] == "duplicate-handler"),
            "{label}"
        );
    }
}

#[test]
fn matcher_alias_mutations_preserve_controls_and_withdraw_optional_comparison() {
    for (setup, mutation, expected) in [
        ("api = SyntheticAssertions.api", "", true),
        (
            "api = SyntheticAssertions.api; other = api",
            "other[:expect] = replacement",
            false,
        ),
        (
            "api = SyntheticAssertions.api; check = api[:expect]",
            "check.object_containing = replacement",
            false,
        ),
        (
            "api = SyntheticAssertions.api; check = api[runtime_key]",
            "check.object_containing = replacement",
            false,
        ),
        (
            "api = SyntheticAssertions.api; negated = api[:expect].not",
            "negated.object_containing = replacement",
            false,
        ),
        (
            "api = SyntheticAssertions.api; copy = api[:expect].dup",
            "copy.not = replacement",
            true,
        ),
        (
            "api = SyntheticAssertions.api; copy = api[:expect].dup",
            "copy.not.object_containing = replacement",
            false,
        ),
    ] {
        let root = project(PROVIDER);
        fs::write(root.path().join(".cuke-dedup.json"), r#"{"assertionModules":["provider"],"threshold":100,"rules":{"unused-definition":"off"}}"#).unwrap();
        fs::write(root.path().join("steps.rb"), format!(
            "require_relative 'provider'\n{setup}\n{mutation}\nGiven('the parcel seal is confirmed') {{ |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(tier: 'gold')) }}\nGiven('the parcel seal is now confirmed') {{ |state| api[:expect].call(state).to_equal(api[:expect].not.object_containing(tier: 'gold')) }}\n"
        )).unwrap();
        let rows = analyze(root.path(), "steps.rb");
        let label = format!("{setup}; {mutation}");
        assert_eq!(
            rows.last().unwrap()["corpus"]["incomplete"],
            false,
            "{label}"
        );
        assert_eq!(
            rows.last().unwrap()["summary"]["definitionsAnalyzed"],
            2,
            "{label}"
        );
        assert_eq!(
            rows.iter().any(|row| row["rule"] == "duplicate-handler"),
            expected,
            "{label}"
        );
        if !expected {
            assert!(
                !rows.iter().any(|row| matches!(
                    row["rule"].as_str(),
                    Some("near-duplicate-step" | "parameterization-candidate")
                )),
                "{label}"
            );
        }
    }
}

fn findings(provider: &str, setup: &str, first: &str, second: &str) -> Vec<Value> {
    let root = project(provider);
    fs::write(root.path().join(".cuke-dedup.json"), r#"{"assertionModules":["provider"],"threshold":100,"nearDuplicateHandlerSimilarity":0.5,"rules":{"unused-definition":"off"}}"#).unwrap();
    fs::write(root.path().join("steps.rb"), format!("require_relative 'provider'\n{setup}\nThen('the parcel status is verified') {{ |state| {first} }}\nThen('the parcel status is now verified') {{ |state| {second} }}\n")).unwrap();
    analyze(root.path(), "steps.rb")
}

fn handler_findings(rows: &[Value]) -> bool {
    rows.iter().any(|row| {
        matches!(
            row["rule"].as_str(),
            Some("duplicate-handler" | "near-duplicate-step" | "parameterization-candidate")
        )
    })
}

#[test]
fn assertion_overlap_preserves_values_effects_and_callable_execution() {
    for (first, second, expected) in [
        (
            "-> { check.call(state).to_be('ready') }",
            "(-> { check.call(state).to_be('ready') }).call",
            false,
        ),
        (
            "check.call(state).to_be('ready')",
            "check.call(state).to_be('ready')",
            true,
        ),
        (
            "check.call(state).to_be('ready')",
            "check.call(state).to_be('idle')",
            false,
        ),
        (
            "inspect = ->(expect) { expect.call('unrelated') }; check.call(state).to_be('ready')",
            "inspect = ->(expect) { expect.call('unrelated') }; check.call(state).to_be('idle')",
            false,
        ),
        (
            "if allowed; check.call(state).to_be('ready'); end",
            "if allowed; check.call(state).to_be('idle'); end",
            false,
        ),
        (
            "register(callback: -> { check.call(state).to_be('ready') })",
            "other_wrapper([proc { check.call(state).to_be('ready') }])",
            true,
        ),
        (
            "register(-> { check.call(state).to_be('ready') })",
            "register(-> { check.call(state).not.to_be('ready') })",
            false,
        ),
        (
            "register(-> { (->(value) { save(value) }).call(external) })",
            "register(-> { (proc { |value| save(value) }).call(external) })",
            false,
        ),
        (
            "check.public_send(:soft, state).to_be('ready')",
            "check.public_send(:soft, state).to_be('idle')",
            false,
        ),
        (
            "expected = 'ready'; check.call(state).to_be(expected)",
            "expected = 'idle'; check.call(state).to_be(expected)",
            false,
        ),
    ] {
        let rows = findings(
            PROVIDER,
            "check = SyntheticAssertions.api.fetch(:expect)",
            first,
            second,
        );
        assert_eq!(
            rows.last().unwrap()["summary"]["definitionsAnalyzed"],
            2,
            "{first} / {second}"
        );
        assert_eq!(handler_findings(&rows), expected, "{first} / {second}");
    }
}

#[test]
fn malformed_factory_origins_and_lexical_shadowing_gain_no_authority() {
    for provider in [
        format!("{PROVIDER}\nclass SyntheticAssertions::Factory; def call(actual); replacement.call(actual); end; end"),
        PROVIDER.replace("alias soft call", "alias soft call\n    def dup; self; end"),
        PROVIDER.replace("attr_accessor :not", "attr_accessor :not\n    def not; replacement; end"),
        PROVIDER.replace(
            "Factory.new(method(:expect))",
            "Factory.new(other.method(:expect))",
        ),
        PROVIDER.replace(
            "Factory.new(method(:expect))",
            "Factory.new(method(:expect), replacement)",
        ),
        PROVIDER.replace(
            "@exported_expect.call(actual)",
            "@exported_expect.call(other)",
        ),
        PROVIDER.replace(
            "@exported_expect.call(actual)",
            "@exported_expect.call(actual) { modify() }",
        ),
        PROVIDER.replace(
            "alias soft call",
            "alias soft call\n    def call(actual); replacement.call(actual); end",
        ),
    ] {
        let rows = findings(
            &provider,
            "check = SyntheticAssertions.api.fetch(:expect)",
            "check.call(state).to_be_visible",
            "check.call(state).to_be_visible",
        );
        assert!(!handler_findings(&rows), "{provider}");
    }
    for setup in [
        "check = SyntheticAssertions.api.fetch(:expect); SyntheticAssertions = replacement",
        "check = SyntheticAssertions.api.fetch(:expect); SyntheticAssertions.expect = replacement",
        "check = SyntheticAssertions.api.fetch(:expect); unknown(check)",
        "check = SyntheticAssertions.api.fetch(:expect); check.reset()",
        "check = SyntheticAssertions.api.values_at(:expect)",
        "api = SyntheticAssertions.api; check = api[:expect]; api.clear()",
        "check = SyntheticAssertions.api.fetch(:expect); SyntheticAssertions.reset()",
    ] {
        let rows = findings(
            PROVIDER,
            setup,
            "check.call(state).to_be_visible",
            "check.call(state).to_be_visible",
        );
        assert!(!handler_findings(&rows), "{setup}");
    }
    let rows = findings(
        PROVIDER,
        "check = SyntheticAssertions.api.fetch(:expect)",
        "check = unknown; check.call(state).to_be_visible",
        "check = other; check.call(state).to_be_visible",
    );
    assert!(!handler_findings(&rows));
}

#[test]
fn bound_assertion_fields_preserve_receiver_context_and_opposing_values() {
    for (second, allocations, expected) in [
        ("ready", "second = first", false),
        ("ready", "", true),
        ("idle", "", false),
        ("ready", "second = BoundChecks.new(check)", false),
        ("ready", "second = BoundChecks.new(unknown)", false),
    ] {
        let root = project(PROVIDER);
        fs::write(root.path().join(".cuke-dedup.json"), r#"{"assertionModules":["provider"],"threshold":100,"rules":{"unused-definition":"off"}}"#).unwrap();
        let receiver = if allocations.is_empty() {
            "first"
        } else {
            "second"
        };
        fs::write(root.path().join("steps.rb"), format!(
            "require_relative 'provider'\ncheck = SyntheticAssertions.api.fetch(:expect)\nclass BoundChecks\n def initialize(check); @check = check; end\n def primary(state); @check.call(state.status).to_be('ready'); end\n def secondary(state); @check.call(state.status).to_be('{second}'); end\nend\nfirst = BoundChecks.new(check)\n{allocations}\nThen('the parcel status is verified', &first.method(:primary))\nThen('the parcel status is now verified', &{receiver}.method(:secondary))\n"
        )).unwrap();
        let rows = analyze(root.path(), "steps.rb");
        assert_eq!(handler_findings(&rows), expected, "{second}; {allocations}");
        if allocations.is_empty() {
            assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], false);
        }
    }
}

#[test]
fn preceding_fixture_exports_retain_assertions_without_global_scope_leakage() {
    for (entry, expected) in [
        ("require_relative 'fixture'\nrequire_relative 'steps'", true),
        (
            "require_relative 'steps'\nrequire_relative 'fixture'",
            false,
        ),
    ] {
        let root = project(PROVIDER);
        fs::write(root.path().join("fixture.rb"), "require_relative 'provider'\nmodule BoundFixture\n EXPECT = SyntheticAssertions.method(:expect)\nend\n").unwrap();
        fs::write(root.path().join("entry.rb"), entry).unwrap();
        fs::write(root.path().join("steps.rb"), "check = BoundFixture::EXPECT\nThen('the parcel status is verified') { |state| check.call(state.status).to_be('ready') }\nThen('the parcel status is now verified') { |state| check.call(state.status).to_be('ready') }\n").unwrap();
        fs::write(root.path().join(".cuke-dedup.json"), r#"{"assertionModules":["./provider"],"threshold":100,"rules":{"unused-definition":"off"}}"#).unwrap();
        let rows = analyze(root.path(), "entry.rb");
        assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
        assert_eq!(handler_findings(&rows), expected, "{entry}");
        if expected {
            assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], false);
        }
    }
}

#[test]
fn rejected_factory_comparison_does_not_hide_unsupported_handler_syntax() {
    let rows = findings(
        PROVIDER,
        "check = SyntheticAssertions.api.fetch(:expect); check.object_containing = replacement",
        "check.call(state).to_be_visible",
        "check.call(state).to_be_visible; ->(amount = 1) { apply(amount) }",
    );
    assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
    assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], true);
    assert!(!handler_findings(&rows));
}

fn analyze(root: &std::path::Path, definitions: &str) -> Vec<Value> {
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(root)
        .args([
            "--definitions",
            definitions,
            "--reporters",
            "jsonl",
            "--no-metrics",
        ])
        .output()
        .unwrap();
    assert!(
        !output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn project(provider: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("provider.rb"), provider).unwrap();
    root
}
