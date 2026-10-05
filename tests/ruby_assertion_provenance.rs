use assert_cmd::Command;
use serde_json::Value;
use std::fs;

mod common;

const PROVIDER: &str = include_str!(
    "../fixtures/ruby-parity/equivalent-handlers/precision-inline-local/providers/assertions.rb"
);
const UNRELATED: &str = include_str!(
    "../fixtures/ruby-parity/equivalent-handlers/precision-import-esm/providers/unrelated.rb"
);

/// Checks configured assertion conflicts and unconfigured parameterization controls.
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

/// Checks shared alias mutation without suppressing valid immutable controls.
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
        let root = configured_project();
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

/// Runs a synthetic configured provider pair through the final analyzer.
fn findings(provider: &str, setup: &str, first: &str, second: &str) -> Vec<Value> {
    let root = project(provider);
    fs::write(root.path().join(".cuke-dedup.json"), r#"{"assertionModules":["provider"],"threshold":100,"nearDuplicateHandlerSimilarity":0.5,"rules":{"unused-definition":"off"}}"#).unwrap();
    fs::write(root.path().join("steps.rb"), format!("require_relative 'provider'\n{setup}\nThen('the parcel status is verified') {{ |state| {first} }}\nThen('the parcel status is now verified') {{ |state| {second} }}\n")).unwrap();
    analyze(root.path(), "steps.rb")
}

/// Reports whether analyzer output contains a handler-comparison finding.
fn handler_findings(rows: &[Value]) -> bool {
    rows.iter().any(|row| {
        matches!(
            row["rule"].as_str(),
            Some("duplicate-handler" | "near-duplicate-step" | "parameterization-candidate")
        )
    })
}

/// Checks assertion overlap against conflicting effects, values, and execution timing.
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

/// Checks that malformed and shadowed factories never gain assertion trust.
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

/// Checks injected assertion fields against allocation and value conflicts.
#[test]
fn bound_assertion_fields_preserve_receiver_context_and_opposing_values() {
    for (second, allocations, expected) in [
        ("ready", "second = first", false),
        ("ready", "", true),
        ("idle", "", false),
        ("ready", "second = BoundChecks.new(check)", false),
        ("ready", "second = BoundChecks.new(unknown)", false),
    ] {
        let root = configured_project();
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

/// Checks preceding source exports without leaking authority into unrelated files.
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

/// Checks independent discovery uncertainty beside rejected optional comparison.
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
    for (effect, incomplete) in [
        ("", false),
        ("work(self)", true),
        ("work(@field)", true),
        ("work(@@field)", true),
        ("work($field)", true),
    ] {
        let root = configured_project();
        fs::write(root.path().join("steps.rb"), format!(
            "require_relative 'provider'\ndef primary(state)\n SyntheticAssertions.expect(state, extra).to_be_visible\n {effect}\nend\ndef secondary(state)\n SyntheticAssertions.expect(state, extra).to_be_visible\n {effect}\nend\nThen('the parcel status is verified', &method(:primary))\nThen('the parcel status is now verified', &method(:secondary))"
        )).unwrap();
        let rows = analyze(root.path(), "steps.rb");
        assert_eq!(
            rows.last().unwrap()["summary"]["definitionsAnalyzed"],
            2,
            "{effect}"
        );
        assert_eq!(
            rows.last().unwrap()["corpus"]["incomplete"],
            incomplete,
            "{effect}"
        );
        assert!(!handler_findings(&rows), "{effect}");
    }
    for body in [
        "@check.call(state).to_be_visible(extra)",
        "@check.call(state).to_be_visible",
    ] {
        let root = configured_project();
        fs::write(root.path().join("steps.rb"), format!(
            "require_relative 'provider'\nclass BoundChecks\n def initialize(check); @check = check; end\n def primary(state); {body}; end\n def secondary(state); {body}; end\nend\ncheck = SyntheticAssertions.api.fetch(:expect)\nfirst = BoundChecks.new(check)\nThen('the parcel status is verified', &first.method(:primary))\nThen('the parcel status is now verified', &first.method(:secondary))"
        )).unwrap();
        let rows = analyze(root.path(), "steps.rb");
        assert_eq!(
            rows.last().unwrap()["summary"]["definitionsAnalyzed"],
            2,
            "{body}"
        );
        assert_eq!(
            rows.last().unwrap()["corpus"]["incomplete"],
            false,
            "{body}"
        );
        assert_eq!(handler_findings(&rows), !body.contains("extra"), "{body}");
    }
}

/// Runs Ruby analysis and parses its JSONL findings and final summary.
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

/// Creates an original synthetic Ruby provider in a temporary analysis root.
fn project(provider: &str) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("provider.rb"), provider).unwrap();
    root
}

/// Checks literal origin lookup, alias escapes, shadowing, and immutable controls.
#[test]
fn source_origin_lookup_and_escape_matrix() {
    for (setup, expected) in [
        ("check, = SyntheticAssertions.api.values_at(:expect)", true),
        ("namespace = SyntheticAssertions; check = namespace.method(:expect)", false),
        ("check = (SyntheticAssertions.public_method(:expect))", true),
        ("api = SyntheticAssertions.api; check = api[:expect, :other]", false),
        ("api = SyntheticAssertions.api; check = api[:other]", false),
        ("check = SyntheticAssertions.api.fetch(:expect); check = replacement", false),
        ("namespace = SyntheticAssertions; namespace.reset; check = namespace.method(:expect)", false),
        ("namespace = SyntheticAssertions; unknown(namespace); check = namespace.method(:expect)", false),
        ("namespace = SyntheticAssertions; second = namespace; unknown(second); check = namespace.method(:expect)", false),
        ("check = SyntheticAssertions.api.fetch(:expect); nested = lambda { |check| check.call(state) }", true),
        ("namespace = SyntheticAssertions; def namespace.expect(value); value; end; check = namespace.method(:expect)", false),
        ("check = SyntheticAssertions.api.fetch(:expect); later { local = check }", false),
        ("check = SyntheticAssertions.api.fetch(:expect); later { check = other }", false),
        ("namespace = SyntheticAssertions; check = namespace.method(:expect); unknown(check)", false),
        ("check = SyntheticAssertions.api.fetch(:expect); expectation = check.call(actual); unknown(expectation)", false),
        ("check = SyntheticAssertions.api.fetch(:expect); check.send(:send, :send, :send, :send, :send, :send, :send, :send, :send, :send, :send, :send, :send, :send, :send, :send, :call, actual)", false),
        ("module ::Other; end; check = SyntheticAssertions.api.fetch(:expect)", true),
    ] {
        let rows = findings(PROVIDER, setup, "check.call(state).to_be_visible", "check.call(state).to_be_visible");
        assert_eq!(handler_findings(&rows), expected, "{setup}");
        if expected {
            assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
        }
    }
}

/// Checks unsupported export members and wrappers cannot grant comparison authority.
#[test]
fn malformed_export_shapes_never_grant_comparison() {
    for (from, to) in [
        (
            "{ expect: Factory.new(method(:expect)) }",
            "{ other: Factory.new(method(:expect)) }",
        ),
        (
            "{ expect: Factory.new(method(:expect)) }",
            "{ expect: Factory.new(method(:expect)), other: 1 }",
        ),
        (
            "Factory.new(method(:expect))",
            "Factory.build(method(:expect))",
        ),
        (
            "Factory.new(method(:expect))",
            "Factory.new(method(:other))",
        ),
        (
            "Factory.new(method(:expect))",
            "Factory.new(method(:expect)) { work() }",
        ),
        (
            "@exported_expect.call(actual)",
            "@exported_expect.call(actual); work()",
        ),
        ("@exported_expect.call(actual)", "other.call(actual)"),
        (
            "@exported_expect = exported_expect",
            "@exported_expect = replacement",
        ),
        ("def call(actual)", "def call(actual, extra)"),
        ("def call(actual)", "def call(*actual)"),
        ("alias soft call", "alias different call"),
        ("class Factory", "class Factory < Foreign"),
        ("alias soft call", "alias soft call\n    alias soft call"),
        ("alias soft call", "alias soft call\n    class Nested; end"),
        (
            "alias soft call",
            "alias soft call\n    def initialize(value); @exported_expect = value; end",
        ),
        ("def self.api", "def self.api\n    work()"),
        (
            "def self.expect(actual)",
            "def self.expect(actual); def nested; end;",
        ),
    ] {
        let provider = PROVIDER.replace(from, to);
        let rows = findings(
            &provider,
            "check = SyntheticAssertions.api.fetch(:expect)",
            "check.call(state).to_be_visible",
            "check.call(state).to_be_visible",
        );
        assert!(!handler_findings(&rows), "{from} => {to}");
    }
}

/// Checks expected-value resolution without collapsing captures or unknown values.
#[test]
fn expected_values_preserve_bindings_and_uncertainty() {
    for (first, second, expected) in [
        (
            "check.call(state).to_be(state.status)",
            "check.call(state).to_be(state.status)",
            true,
        ),
        (
            "check.call(state).to_be(state)",
            "check.call(state).to_be(state)",
            true,
        ),
        (
            "check.call(state).to_be(state.status())",
            "check.call(state).to_be(state.status())",
            true,
        ),
        (
            "check.call(state).to_be(state.status(1))",
            "check.call(state).to_be(state.status(2))",
            false,
        ),
        (
            "check.call(state).to_be(state.send(:status))",
            "check.call(state).to_be(state.send(:status))",
            false,
        ),
        (
            "check.call(state).to_be(state.status { work() })",
            "check.call(state).to_be(state.status { work() })",
            false,
        ),
        (
            "check.call(state).to_be(external)",
            "check.call(state).to_be(external)",
            false,
        ),
        (
            "expected = 'ready'; expected = 'idle'; check.call(state).to_be(expected)",
            "expected = 'ready'; expected = 'idle'; check.call(state).to_be(expected)",
            false,
        ),
        (
            "check.call(state).to_be(expected); expected = 'ready'",
            "check.call(state).to_be(expected); expected = 'ready'",
            false,
        ),
        (
            "if allowed; expected = 'ready'; end; check.call(state).to_be(expected)",
            "if allowed; expected = 'ready'; end; check.call(state).to_be(expected)",
            false,
        ),
        (
            "check.call(state).to_equal(check.not.object_containing(enabled: false))",
            "check.call(state).to_equal(check.not.object_containing(enabled: false))",
            true,
        ),
        (
            "expected = nil; check.call(state).to_be(expected)",
            "expected = false; check.call(state).to_be(expected)",
            false,
        ),
        (
            "check.call(state).to_be(check)",
            "check.call(state).to_be(check)",
            false,
        ),
        (
            "check.call(state).to_equal(check.not.object_containing(types: [false, nil]))",
            "check.call(state).to_equal(check.not.object_containing(types: [false, nil]))",
            true,
        ),
    ] {
        let rows = findings(
            PROVIDER,
            "check = SyntheticAssertions.api.fetch(:expect)",
            first,
            second,
        );
        assert_eq!(handler_findings(&rows), expected, "{first} / {second}");
    }
}

/// Checks that ambiguous classes and allocations cannot authorize injected assertions.
#[test]
fn ambiguous_constructor_injection_withdraws_authority() {
    let class = "class BoundChecks\n def initialize(check); @check = check; end\n def primary(state); @check.call(state).to_be_visible; end\n def secondary(state); @check.call(state).to_be_visible; end\nend";
    for (from, to) in [
        ("class BoundChecks", "class BoundChecks < Parent"),
        ("def initialize(check)", "def initialize(check, extra)"),
        ("def initialize(check)", "def initialize(*check)"),
        ("@check = check", "@check = check; work()"),
        ("@check = check", "work()"),
        ("@check = check", "local = check"),
        ("@check = check", "@check = replacement"),
        ("@check.call(state)", "@other.call(state)"),
        ("@check.call(state)", "@check.soft(state)"),
        ("@check.call(state)", "@check.call(state) { work() }"),
        ("def initialize(check); @check = check; end", ""),
        (
            "def initialize(check)",
            "attr_reader :check; def initialize(check)",
        ),
    ] {
        let root = configured_project();
        fs::write(root.path().join("steps.rb"), format!("require_relative 'provider'\ncheck = SyntheticAssertions.api.fetch(:expect)\n{}\nfirst = BoundChecks.new(check)\nThen('the parcel status is verified', &first.method(:primary))\nThen('the parcel status is now verified', &first.method(:secondary))", class.replace(from, to))).unwrap();
        let rows = analyze(root.path(), "steps.rb");
        assert!(!handler_findings(&rows), "{from} => {to}");
    }
}

/// Checks export intersections across multiple incoming source loads.
#[test]
fn incoming_load_contexts_require_common_preceding_exports() {
    for (second_entry, expected) in [
        ("require_relative 'fixture'; require_relative 'steps'", true),
        (
            "require_relative 'steps'; require_relative 'fixture'",
            false,
        ),
        ("later { require_relative 'steps' }", true),
    ] {
        let root = project(PROVIDER);
        fs::write(root.path().join("fixture.rb"), "require_relative 'provider'; module Fixture; EXPECT = SyntheticAssertions.method(:expect); OTHER = unknown; end").unwrap();
        fs::write(
            root.path().join("first.rb"),
            "require_relative 'fixture'; require_relative 'steps'",
        )
        .unwrap();
        fs::write(root.path().join("second.rb"), second_entry).unwrap();
        fs::write(root.path().join("steps.rb"), "check = Fixture::EXPECT; Then('the parcel status is verified') { |state| check.call(state).to_be_visible }; Then('the parcel status is now verified') { |state| check.call(state).to_be_visible }").unwrap();
        fs::write(root.path().join(".cuke-dedup.json"), r#"{"assertionModules":["./provider"],"threshold":100,"rules":{"unused-definition":"off"}}"#).unwrap();
        let rows = analyze(root.path(), "first.rb,second.rb");
        assert_eq!(handler_findings(&rows), expected, "{second_entry}");
    }
}

/// Checks supported literal dispatch alongside dynamic and mutated-receiver controls.
#[test]
fn literal_dispatch_preserves_assertions_without_trusting_mutated_receivers() {
    for (setup, expression, expected) in [
        (
            "check = SyntheticAssertions.method(:expect)",
            "check.call(state).public_send(:to_be, 'ready')",
            true,
        ),
        (
            "check = SyntheticAssertions.api.fetch(:expect)",
            "check.public_send(:not).call(state).to_be('ready')",
            true,
        ),
        (
            "check = SyntheticAssertions.api.fetch(:expect); unknown(check)",
            "check.public_send(:soft, state).to_be('ready')",
            false,
        ),
        (
            "check = SyntheticAssertions.api.fetch(:expect); unknown(check)",
            "if allowed; check.send(:call, state).to_be('ready'); end",
            false,
        ),
        (
            "check = SyntheticAssertions.api.fetch(:expect)",
            "check.soft(state).send(:not).send(:to).send(:to_be, 'ready')",
            true,
        ),
    ] {
        let rows = findings(PROVIDER, setup, expression, expression);
        assert_eq!(handler_findings(&rows), expected, "{setup}; {expression}");
    }
}

/// Creates a synthetic project with explicit assertion-provider configuration.
fn configured_project() -> tempfile::TempDir {
    let root = project(PROVIDER);
    fs::write(
        root.path().join(".cuke-dedup.json"),
        r#"{"assertionModules":["provider"],"threshold":100,"rules":{"unused-definition":"off"}}"#,
    )
    .unwrap();
    root
}

/// Checks namespace mutation, reflective selectors, and argument-shape trust boundaries.
#[test]
fn namespace_trust_requires_unmutated_expect_lookups() {
    let provider = "module SyntheticAssertions; def self.expect(actual); actual; end; end";
    for setup in [
        "",
        "SyntheticAssertions.expect = replacement",
        "SyntheticAssertions.expect ||= replacement",
        "SyntheticAssertions.method(:other)",
        "SyntheticAssertions.public_method(:other)",
        "namespace = SyntheticAssertions; namespace.public_method(:other)",
        "SyntheticAssertions.method(runtime_key)",
        "SyntheticAssertions.api(untrusted)",
        "SyntheticAssertions.expect(state, extra)",
        "SyntheticAssertions.expect",
        "SyntheticAssertions.method(:expect, extra)",
        "SyntheticAssertions.public_method(:expect, extra)",
        "SyntheticAssertions.expect(*values)",
        "SyntheticAssertions.expect(**values)",
        "SyntheticAssertions.api(*values)",
        "SyntheticAssertions.method(*names)",
    ] {
        let rows = findings(
            provider,
            setup,
            "register(-> { SyntheticAssertions.expect(state).to_be('ready') })",
            "other_wrapper([proc { SyntheticAssertions.expect(state).to_be('ready') }])",
        );
        assert_eq!(handler_findings(&rows), setup.is_empty(), "{setup}");
    }
}

/// Checks equivalent negated factories and conflicting polarity or expected values.
#[test]
fn negated_factory_dispatch_retains_assertions_and_polarity() {
    for (first, second, expected) in [
        (
            "check.not.call(state).to_be('ready')",
            "check.not.call(state).to_be('idle')",
            false,
        ),
        (
            "check.not.call(state).to_be('ready')",
            "check.call(state).to_be('ready')",
            false,
        ),
        (
            "register(-> { check.public_send(:not).call(state).to_be('ready') })",
            "other_wrapper([proc { check.public_send(:not).call(state).to_be('ready') }])",
            true,
        ),
        (
            "check.not.call(state).to_be('ready')",
            "check.call(state).not.to_be('ready')",
            true,
        ),
        (
            "check.not.soft(state).to_be('ready')",
            "check.call(state).not.to_be('ready')",
            true,
        ),
        (
            "check.dup.not.call(state).to_be('ready')",
            "check.call(state).not.to_be('ready')",
            true,
        ),
    ] {
        let rows = findings(
            PROVIDER,
            "check = SyntheticAssertions.api.fetch(:expect)",
            first,
            second,
        );
        assert_eq!(handler_findings(&rows), expected, "{first} / {second}");
    }
}

/// Checks uncertain provider shapes alongside direct immutable assertion controls.
#[test]
fn provider_adjacent_shapes_preserve_closed_controls() {
    for (provider, setup) in [
        (PROVIDER.to_owned(), "check = SyntheticAssertions.api[:expect][:other]"),
        (PROVIDER.to_owned(), "check = SyntheticAssertions.api.fetch(:expect); alias_check = check; SyntheticAssertions::Other = replacement"),
        (PROVIDER.replace("method(:expect)", "public_method(:expect)"), "check = SyntheticAssertions.api.fetch(:expect)"),
        (PROVIDER.replace("def self.expect(actual)", "def self.expect(actual); work()\n"), "check = SyntheticAssertions.api.fetch(:expect)"),
    ] {
        let rows = findings(&provider, setup, "check.call(state).to_be('ready')", "check.call(state).to_be('idle')");
        assert!(!handler_findings(&rows), "{setup}");
    }
    let rows = findings(
        PROVIDER,
        "check = SyntheticAssertions.api.fetch(:expect)",
        "work; check.call(state).to_be_visible",
        "work; check.call(state).to_be_visible",
    );
    assert!(handler_findings(&rows));
    for matcher in [
        "to_equal",
        "equal_to",
        "to_have_class",
        "to_have_value",
        "to_be_visible",
    ] {
        let expression = format!(
            "expectation.{matcher}{}",
            if matcher == "to_be_visible" {
                ""
            } else {
                "(1)"
            }
        );
        let rows = findings(
            PROVIDER,
            "check = SyntheticAssertions.api.fetch(:expect); expectation = check.call(state)",
            &expression,
            &expression,
        );
        assert!(
            !handler_findings(&rows),
            "cached expectation must remain uncertain: {matcher}"
        );
        let direct = expression.replace("expectation.", "check.call(state).");
        let rows = findings(
            PROVIDER,
            "check = SyntheticAssertions.api.fetch(:expect)",
            &direct,
            &direct,
        );
        assert!(handler_findings(&rows), "direct expectation: {matcher}");
    }
}

/// Checks that separate inline allocations do not collapse into one bound handler.
#[test]
fn inline_bound_receivers_keep_distinct_allocation_context() {
    let root = configured_project();
    fs::write(root.path().join("steps.rb"), "require_relative 'provider'; check = SyntheticAssertions.method(:expect); class Worker; def work; action(); end; end; Then('the parcel status is verified', &Worker.new.method(:work)); Then('the parcel status is now verified', &Worker.new.method(:work))").unwrap();
    let rows = analyze(root.path(), "steps.rb");
    assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
    assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], false);
    assert!(!handler_findings(&rows));
}

/// Checks ordinary literal parameterization beside unrelated assertion origins.
#[test]
fn ordinary_calls_beside_assertion_origins_retain_parameterization() {
    let root = configured_project();
    fs::write(root.path().join("steps.rb"), "require_relative 'provider'; check = SyntheticAssertions.method(:expect); Given('the parcel status is verified') { Worker.action(1) }; Given('the parcel status is now verified') { Worker.action(2) }").unwrap();
    let rows = analyze(root.path(), "steps.rb");
    assert!(handler_findings(&rows));
}

/// Checks exact, dotted, missing, and non-Ruby-suffix configuration paths.
///
/// Equal deferred values are near under both regimes (trusted deferred assertions match,
/// untrusted callback bodies overlap). Conflicting values discriminate: a trusted provider keeps
/// them in its deferred assertions and reports nothing; an untrusted one drops arguments and
/// still reports the near finding.
#[test]
fn configured_ruby_provider_paths_preserve_literal_specifiers() {
    for (configured, file, load, expected) in [
        ("./provider", "provider.rb", "provider", true),
        ("./provider.rb", "provider.rb", "provider", true),
        ("./fixtures.ts", "fixtures.rb", "fixtures", false),
        ("./fixtures.ts", "fixtures.ts.rb", "fixtures.ts.rb", true),
        (
            "./expect.helpers",
            "expect.helpers.rb",
            "expect.helpers.rb",
            true,
        ),
        ("./expect.helpers", "expect.rb", "expect", false),
        ("./missing", "provider.rb", "provider", false),
    ] {
        let root = project(PROVIDER);
        if file != "provider.rb" {
            fs::rename(root.path().join("provider.rb"), root.path().join(file)).unwrap();
        }
        fs::write(root.path().join(".cuke-dedup.json"), serde_json::json!({"assertionModules":[configured], "threshold":100, "nearDuplicateHandlerSimilarity":0.5, "rules":{"unused-definition":"off"}}).to_string()).unwrap();
        for (second_value, findings) in [("'ready'", true), ("'idle'", !expected)] {
            fs::write(root.path().join("steps.rb"), format!("require_relative '{load}'\ncheck = SyntheticAssertions.method(:expect)\nThen('the parcel status is verified') {{ |state| register(-> {{ check.call(state).to_be('ready') }}) }}\nThen('the parcel status is now verified') {{ |state| other_wrapper([proc {{ check.call(state).to_be({second_value}) }}]) }}\n")).unwrap();
            let rows = analyze(root.path(), "steps.rb");
            assert_eq!(
                handler_findings(&rows),
                findings,
                "{configured} / {file} / {second_value}"
            );
            assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
            assert_eq!(
                rows.last().unwrap()["corpus"]["incomplete"],
                false,
                "{configured} / {file}"
            );
        }
    }
}

/// Checks ordinary parameterization without erasing interpolated argument effects.
#[test]
fn unconfigured_parameterization_keeps_interpolation_effects() {
    for (first, second, expected) in [
        ("1", "2", true),
        ("1.5", "2.5", true),
        ("'ready'", "'idle'", true),
        (r##""#{one()}""##, r##""#{two()}""##, false),
        (r##""#{one()}""##, r##""#{one()}""##, true),
    ] {
        let root = project(PROVIDER);
        fs::write(
            root.path().join(".cuke-dedup.json"),
            r#"{"threshold":100,"rules":{"unused-definition":"off"}}"#,
        )
        .unwrap();
        fs::write(root.path().join("steps.rb"), format!("require_relative 'provider'; check = SyntheticAssertions.method(:expect); Given('the parcel status is verified') {{ check.call(gauge()).to_be({first}) }}; Given('the parcel status is now verified') {{ check.call(gauge()).to_be({second}) }}")).unwrap();
        let rows = analyze(root.path(), "steps.rb");
        assert_eq!(handler_findings(&rows), expected, "{first} / {second}");
    }
}

/// Checks namespace invalidation across source-owned constant aliases and unrelated modules.
#[test]
fn cross_file_constant_aliases_cannot_hide_namespace_mutation() {
    for (aliases, mutation, expected) in [
        ("", "", true),
        (
            "ProviderAlias = SyntheticAssertions",
            "module Other; def self.expect(actual); replacement(); end; end",
            false,
        ),
        (
            "ProviderAlias = SyntheticAssertions",
            "module ProviderAlias; def self.expect(actual); replacement(); end; end",
            false,
        ),
        (
            "ProviderAlias = SyntheticAssertions",
            "ProviderAlias.reset",
            false,
        ),
        (
            "ProviderAlias = SyntheticAssertions",
            "unknown(ProviderAlias)",
            false,
        ),
        (
            "ProviderAlias = SyntheticAssertions; OtherAlias = ProviderAlias",
            "module OtherAlias; def self.expect(actual); replacement(); end; end",
            false,
        ),
    ] {
        let root = configured_project();
        fs::write(root.path().join(".cuke-dedup.json"), r#"{"assertionModules":["provider"],"threshold":100,"nearDuplicateHandlerSimilarity":0.5,"rules":{"unused-definition":"off"}}"#).unwrap();
        fs::write(
            root.path().join("aliases.rb"),
            format!("require_relative 'provider'; {aliases}"),
        )
        .unwrap();
        fs::write(
            root.path().join("mutation.rb"),
            format!("require_relative 'aliases'; {mutation}"),
        )
        .unwrap();
        let loads = if aliases.is_empty() {
            "require_relative 'provider'"
        } else {
            "require_relative 'mutation'; require_relative 'provider'"
        };
        fs::write(root.path().join("steps.rb"), format!("{loads}; check = SyntheticAssertions.api.fetch(:expect); Then('the parcel status is verified') {{ |state| register(-> {{ check.call(state).to_be('ready') }}) }}; Then('the parcel status is now verified') {{ |state| other_wrapper([proc {{ check.call(state).to_be('ready') }}]) }}")).unwrap();
        let rows = analyze(root.path(), "steps.rb");
        assert_eq!(handler_findings(&rows), expected, "{aliases} / {mutation}");
        let summary = rows.last().unwrap();
        assert_eq!(
            summary["summary"]["definitionsAnalyzed"],
            if expected { 2 } else { 0 }
        );
        assert_eq!(summary["corpus"]["incomplete"], !expected);
    }
}

/// Runs a configured-provider project and reports active handler rules plus completeness.
fn handler_rules(files: &[(&str, &str)]) -> (Vec<String>, bool) {
    let root = project(PROVIDER);
    fs::write(root.path().join(".cuke-dedup.json"), r#"{"assertionModules":["provider"],"threshold":100,"nearDuplicateHandlerSimilarity":0.5,"rules":{"unused-definition":"off"}}"#).unwrap();
    for (name, body) in files {
        fs::write(root.path().join(name), format!("{body}\n")).unwrap();
    }
    let rows = analyze(root.path(), "*.rb");
    let summary = rows.last().unwrap();
    let rules = common::active_handler_rules(&rows);
    (rules, summary["corpus"]["incomplete"].as_bool().unwrap())
}

const LOAD: &str = "require_relative 'provider'\n";

/// Two differently worded registrations with the given handler bodies and a `state` parameter.
fn step_pair(first: &str, second: &str) -> String {
    format!("Then('the parcel status is verified') {{ |state| {first} }}\nThen('the parcel status is now verified') {{ |state| {second} }}")
}

/// Declared handler lambdas execute only where a single declaration is invoked directly.
#[test]
fn declared_local_callables_execute_only_at_proven_invocations() {
    const CHECK: &str = "SyntheticAssertions.expect(state).to_be";
    let body = |declaration: &str, value: u8, invocation: &str| {
        format!("{declaration}{CHECK}({value}) }}; {invocation}")
    };
    for (label, first, second, expected) in [
        // Identical called declarations are one handler; conflicting values keep them apart
        // without parameterization, because the executed assertions differ.
        ("called identical", body("check = -> { ", 1, "check.call"), body("check = -> { ", 1, "check.call"), vec!["duplicate-handler", "near-duplicate-step"]),
        ("called conflicting", body("check = -> { ", 1, "check.call"), body("check = -> { ", 2, "check.call"), vec![]),
        ("dot-parentheses invocation", body("check = -> { ", 1, "check.()"), body("check = -> { ", 2, "check.()"), vec![]),
        ("parenthesized callee", body("act = -> { ", 1, "(act).call"), body("act = -> { ", 2, "(act).call"), vec![]),
        ("lambda keyword", body("act = lambda { ", 1, "act.call"), body("act = lambda { ", 2, "act.call"), vec![]),
        ("proc keyword", body("act = proc { ", 1, "act.call"), body("act = proc { ", 2, "act.call"), vec![]),
        // Never executed, reassigned, shadowed, nested or deferred declarations are syntax only.
        ("never called", body("act = -> { ", 1, ""), body("act = -> { ", 2, ""), vec!["parameterization-candidate"]),
        ("reassigned before call", body("check = -> { ", 9, "check = -> { SyntheticAssertions.expect(state).to_be(1) }; check.call"), body("check = -> { ", 9, "check = -> { SyntheticAssertions.expect(state).to_be(2) }; check.call"), vec!["parameterization-candidate"]),
        ("block-local shadow", body("check = -> { ", 9, "1.times do |iteration; check| check = -> { SyntheticAssertions.expect(state).to_be(1) }; check.call end"), body("check = -> { ", 9, "1.times do |iteration; check| check = -> { SyntheticAssertions.expect(state).to_be(2) }; check.call end"), vec!["parameterization-candidate"]),
        ("nested declaration", body("outer = -> { act = -> { ", 1, "}; act.call"), body("outer = -> { act = -> { ", 2, "}; act.call"), vec!["parameterization-candidate"]),
        // A handler-level declaration runs wherever it is invoked later: in an expression, or
        // deferred inside a nested block, keeping its values either way.
        ("assigned invocation", body("act = -> { ", 1, "result = act.call"), body("act = -> { ", 2, "result = act.call"), vec![]),
        ("assigned invocation identical", body("act = -> { ", 1, "result = act.call"), body("act = -> { ", 1, "result = act.call"), vec!["duplicate-handler", "near-duplicate-step"]),
        ("argument invocation", body("act = -> { ", 1, "work(act.call)"), body("act = -> { ", 2, "work(act.call)"), vec![]),
        ("invoked only inside a block", body("act = -> { ", 1, "1.times { act.call }"), body("act = -> { ", 2, "1.times { act.call }"), vec![]),
        ("invoked only inside a block identical", body("act = -> { ", 1, "1.times { act.call }"), body("act = -> { ", 1, "1.times { act.call }"), vec!["duplicate-handler", "near-duplicate-step"]),
        // Single-level expansion: recursion inside the body is an ordinary call.
        ("recursive identical", body("act = -> { act.call; ", 1, "act.call"), body("act = -> { act.call; ", 1, "act.call"), vec!["duplicate-handler", "near-duplicate-step"]),
        ("recursive conflicting", body("act = -> { act.call; ", 1, "act.call"), body("act = -> { act.call; ", 2, "act.call"), vec![]),
        // A parameterized declaration invoked with arguments is unproven: no comparison.
        ("parameterized invocation", "act = ->(value) { SyntheticAssertions.expect(state).to_be(value) }; act.call(1)".to_owned(), "act = ->(value) { SyntheticAssertions.expect(state).to_be(value) }; act.call(2)".to_owned(), vec![]),
        // Unmodeled uses keep the deferred value-bearing identity: conflicting values stay
        // distinct and equal values still match.
        ("escaping argument", body("act = -> { ", 1, "run(act)"), body("act = -> { ", 2, "run(act)"), vec![]),
        ("escaping argument identical", body("act = -> { ", 1, "run(act)"), body("act = -> { ", 1, "run(act)"), vec!["duplicate-handler", "near-duplicate-step"]),
        ("element reference invocation", body("act = -> { ", 1, "act[]"), body("act = -> { ", 2, "act[]"), vec![]),
        ("argument to parameterless lambda", body("act = -> { ", 1, "act.call(1)"), body("act = -> { ", 2, "act.call(1)"), vec![]),
        ("operator assignment", body("act ||= -> { ", 1, "act.call"), body("act ||= -> { ", 2, "act.call"), vec![]),
        ("mixed write kinds", body("act = -> { ", 1, "act = other; act.call"), body("act = -> { ", 2, "act = other; act.call"), vec![]),
        ("destructured write", body("act = -> { ", 1, "act, extra = act, 1; act.call"), body("act = -> { ", 2, "act, extra = act, 1; act.call"), vec![]),
        ("mixed arity declarations", body("act = -> { ", 1, "act = ->(value) { value }; act.call"), body("act = -> { ", 2, "act = ->(value) { value }; act.call"), vec![]),
    ] {
        let steps = format!("{LOAD}{}", step_pair(&first, &second));
        let (rules, incomplete) = handler_rules(&[("steps.rb", &steps)]);
        assert_eq!(rules, expected, "{label}");
        assert!(!incomplete, "{label}");
    }
}

/// Top-level constants assigned once with an immutable literal reach the fingerprint.
#[test]
fn top_level_constants_resolve_to_immutable_literal_values() {
    const CHECK: &str = "SyntheticAssertions.expect(state).to_be";
    for (label, prelude, first, second, epilogue, expected) in [
        ("equal integers", "ONE = 1\nTWO = 1", "ONE", "TWO", "", true),
        (
            "conflicting integers",
            "ONE = 1\nTWO = 2",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "declared after the handlers",
            "",
            "ONE",
            "TWO",
            "ONE = 1\nTWO = 1",
            true,
        ),
        (
            "conflicting after the handlers",
            "",
            "ONE",
            "TWO",
            "ONE = 1\nTWO = 2",
            false,
        ),
        (
            "frozen strings",
            "ONE = 'ready'.freeze\nTWO = \"ready\".freeze",
            "ONE",
            "TWO",
            "",
            true,
        ),
        (
            "unfrozen strings",
            "ONE = 'ready'\nTWO = 'ready'",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "frozen string literal comment",
            "# frozen_string_literal: true\nONE = 'ready'\nTWO = 'ready'",
            "ONE",
            "TWO",
            "",
            true,
        ),
        (
            "frozen string literal comment after code",
            "ONE = 'ready'\n# frozen_string_literal: true\nTWO = 'ready'",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "symbols",
            "ONE = :ready\nTWO = :ready",
            "ONE",
            "TWO",
            "",
            true,
        ),
        (
            "second write poisons the name",
            "ONE = 1\nONE = 1\nTWO = 1",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "qualified write poisons the name",
            "ONE = 1\nOther::ONE = 2\nTWO = 1",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "nested write poisons the name",
            "ONE = 1\nTWO = 1\nlater = -> { ONE = 2 }",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "operator write poisons the name",
            "ONE = 1\nTWO = 1\nONE ||= 3",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "destructured write poisons the name",
            "ONE = 1\nTWO = 1\nONE, EXTRA = 1, 2",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "reflective constant mutation",
            "ONE = 1\nTWO = 1\nObject.const_set(:ONE, 2)",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "non-literal value",
            "ONE = compute\nTWO = compute",
            "ONE",
            "TWO",
            "",
            false,
        ),
        (
            "qualified read keeps the name",
            "ONE = 1\nTWO = 1",
            "Other::ONE",
            "Other::TWO",
            "",
            false,
        ),
        ("unknown constants keep names", "", "ONE", "TWO", "", false),
    ] {
        // The magic comment only counts in the leading comment block, so it precedes the load.
        let steps = format!(
            "{prelude}\n{LOAD}{}\n{epilogue}",
            step_pair(&format!("{CHECK}({first})"), &format!("{CHECK}({second})"))
        );
        let (rules, incomplete) = handler_rules(&[("steps.rb", &steps)]);
        assert_eq!(
            rules.contains(&"duplicate-handler".to_owned()),
            expected,
            "{label}: {rules:?}"
        );
        // Reflective constant mutation is already unresolved registration provenance.
        assert_eq!(incomplete, prelude.contains("const_set"), "{label}");
    }
    // A constant written inside a handler is a target, not a read, and its nested write
    // poisons the file-level value.
    let written = format!(
        "ONE = 1\nTWO = 1\n{LOAD}{}",
        step_pair(&format!("ONE = 2; {CHECK}(ONE)"), &format!("{CHECK}(TWO)"))
    );
    assert_eq!(handler_rules(&[("steps.rb", &written)]), (vec![], false));
    // The same values collapse ordinary actions, where no assertion evidence decides.
    let actions = |values: &str| {
        format!("{LOAD}{values}\nGiven('the top slot is pressed first') {{ click_button(ONE) }}\nGiven('the top slot is pressed second') {{ click_button(TWO) }}")
    };
    assert_eq!(
        handler_rules(&[("steps.rb", &actions("ONE = 1\nTWO = 1"))]),
        (
            vec![
                "duplicate-handler".to_owned(),
                "near-duplicate-step".to_owned()
            ],
            false
        )
    );
    assert_eq!(
        handler_rules(&[("steps.rb", &actions("ONE = 1\nTWO = 2"))]),
        (vec![], false)
    );
}

/// Enclosing lambda scopes bind parameters and single-literal locals before file locals.
///
/// Registrations inside executed lambdas stay non-comparable, and the execution proof only
/// accepts literal assignments beside registrations, so the enclosing binding is observed through
/// completeness: a stable binding lets the unresolved expected value reject comparison quietly,
/// an unstable one reports incompleteness.
#[test]
fn enclosing_scope_bindings_shadow_file_locals_with_scope_identity() {
    let register = "Then('the parcel status is verified') { |state| SyntheticAssertions.expect(state).to_be(limit) }";
    for (label, source, incomplete) in [
        ("block-local written once", format!("limit = 1\nbuild = lambda do |; limit|\n  limit = 7\n  {register}\nend\nbuild.call"), false),
        ("parameter", format!("limit = 1\nbuild = lambda do |limit|\n  {register}\nend\nbuild.call(7)"), false),
        ("block-local never written", format!("build = lambda do |; limit|\n  {register}\nend\nbuild.call"), false),
        ("arrow lambda local", format!("build = -> {{\n  limit = 7\n  {register}\n}}\nbuild.call"), false),
        ("second write in scope", format!("build = lambda do |; limit|\n  limit = 7\n  limit = 8\n  {register}\nend\nbuild.call"), true),
        ("parameter written later", format!("build = lambda do |limit|\n  limit = 8\n  limit = 9\n  {register}\nend\nbuild.call(7)"), true),
        ("write inside the handler", "build = lambda do |; limit|\n  limit = 7\n  Then('the parcel status is verified') { |state| limit = 9; SyntheticAssertions.expect(state).to_be(limit) }\nend\nbuild.call".to_owned(), true),
        ("file local written in the lambda", format!("limit = 1\nbuild = lambda do\n  limit = 7\n  {register}\nend\nbuild.call"), true),
    ] {
        let (rules, observed) = handler_rules(&[("steps.rb", &format!("{LOAD}{source}"))]);
        assert_eq!(rules, Vec::<String>::new(), "{label}");
        assert_eq!(observed, incomplete, "{label}");
    }
    // Writes reach a file-level capture unless a nearer scope rebinds the name.
    let pair = "Then('the parcel status is verified') { |state| work(shared) }\nThen('the parcel status is now verified') { |state| work(shared) }";
    for (label, prelude, incomplete) in [
        (
            "block-local write in a handler",
            "shared = 1\nThen('other') { |state; shared| shared = 2; work(shared) }",
            false,
        ),
        (
            "parameter write in a handler",
            "shared = 1\nThen('other') { |shared| shared = 2; work(shared) }",
            false,
        ),
        (
            "plain write in a handler",
            "shared = 1\nThen('other') { |state| shared = 2; work(shared) }",
            true,
        ),
        (
            "rebinding block write",
            "shared = 1\n[1].each { |shared| shared = 2 }",
            false,
        ),
        (
            "plain block write",
            "shared = 1\n[1].each { shared = 2 }",
            true,
        ),
        (
            "rebinding block with a nested plain write",
            "shared = 1\n[1].each { |shared| [2].each { shared = 3 } }",
            false,
        ),
        (
            "nested handler rebinding",
            "shared = 1\nThen('other') { |state| [1].each { |shared| shared = 2 } }",
            false,
        ),
        (
            "nested handler plain write",
            "shared = 1\nThen('other') { |state| [1].each { shared = 2 } }",
            true,
        ),
        (
            "string read outside handlers",
            "shared = 'one'\nother(shared)",
            true,
        ),
        (
            "integer read outside handlers",
            "shared = 1\nother(shared)",
            false,
        ),
    ] {
        let (rules, observed) = handler_rules(&[("steps.rb", &format!("{LOAD}{prelude}\n{pair}"))]);
        assert_eq!(
            rules,
            if incomplete {
                vec![]
            } else {
                vec!["duplicate-handler", "near-duplicate-step"]
            },
            "{label}"
        );
        assert_eq!(observed, incomplete, "{label}");
    }
    // `def` opens a new local scope, so its write never reaches the capture; the defining
    // handler itself stays non-comparable and incomplete as before.
    let walled = format!(
        "{LOAD}shared = 1\nThen('other') {{ |state| def helper; shared = 2; end }}\n{pair}"
    );
    assert_eq!(
        handler_rules(&[("steps.rb", &walled)]),
        (
            vec![
                "duplicate-handler".to_owned(),
                "near-duplicate-step".to_owned()
            ],
            true
        )
    );
}

/// Lambda captures are stable without trust; resolved provider aliases compare across files.
#[test]
fn lambda_captures_and_resolved_aliases_keep_file_identity_rules() {
    let pair = step_pair(
        "expect.call(state).to_be('ready')",
        "expect.call(state).to_be('ready')",
    );
    for (label, prelude, expected) in [
        (
            "file-level lambda capture",
            "expect = ->(value) { value }",
            vec!["duplicate-handler", "near-duplicate-step"],
        ),
        (
            "file-level lambda keyword capture",
            "expect = lambda { |value| value }",
            vec!["duplicate-handler", "near-duplicate-step"],
        ),
        (
            "file-level proc capture",
            "expect = proc { |value| value }",
            vec!["duplicate-handler", "near-duplicate-step"],
        ),
    ] {
        let (rules, incomplete) =
            handler_rules(&[("steps.rb", &format!("{LOAD}{prelude}\n{pair}"))]);
        assert_eq!(rules, expected, "{label}");
        assert!(!incomplete, "{label}");
    }
    let conflicting = step_pair(
        "expect.call(state).to_be('ready')",
        "expect.call(state).to_be('idle')",
    );
    assert_eq!(
        handler_rules(&[(
            "steps.rb",
            &format!("{LOAD}expect = ->(value) {{ value }}\n{conflicting}")
        )]),
        (vec![], false)
    );
    let reassigned =
        format!("{LOAD}expect = ->(value) {{ value }}\nexpect = ->(value) {{ value }}\n{pair}");
    assert!(handler_rules(&[("steps.rb", &reassigned)]).1);

    let first = format!("{LOAD}expect = SyntheticAssertions.method(:expect)\nThen('the parcel is ready') {{ |state| expect.call(state).to_be('ready') }}");
    let second = format!("{LOAD}expect = SyntheticAssertions.api.fetch(:expect)\nThen('shipment readiness has been confirmed') {{ |state| expect.call(state).to_be('ready') }}");
    assert_eq!(
        handler_rules(&[("a.rb", &first), ("b.rb", &second)]),
        (vec!["duplicate-handler".to_owned()], false)
    );
    let lexical_a =
        format!("{LOAD}limit = 1\nThen('the parcel is ready') {{ |state| work(limit) }}");
    let lexical_b = format!(
        "{LOAD}limit = 1\nThen('shipment readiness has been confirmed') {{ |state| work(limit) }}"
    );
    assert_eq!(
        handler_rules(&[("a.rb", &lexical_a), ("b.rb", &lexical_b)]),
        (vec![], false)
    );
    let runtime = format!("{LOAD}expect = ->(value) {{ value }}\nThen('the parcel status is now verified') {{ |state| expect.call(state).to_be('ready') }}");
    assert_eq!(
        handler_rules(&[("a.rb", &first), ("b.rb", &runtime)]),
        (vec![], false)
    );
    // Without a terminal matcher only syntax is recorded, so the resolved owner must keep two
    // same-named aliases of different providers apart across files.
    let synthetic = format!("{LOAD}expect = SyntheticAssertions.method(:expect)\nThen('the parcel is ready') {{ |state| expect.call(state) }}");
    let unrelated = "require_relative 'unrelated'\nexpect = UnrelatedAssertions.method(:expect)\nThen('shipment readiness has been confirmed') { |state| expect.call(state) }";
    let same = format!("{LOAD}expect = SyntheticAssertions.method(:expect)\nThen('shipment readiness has been confirmed') {{ |state| expect.call(state) }}");
    assert_eq!(
        handler_rules(&[
            ("a.rb", &synthetic),
            ("b.rb", unrelated),
            ("unrelated.rb", UNRELATED)
        ]),
        (vec![], false)
    );
    assert_eq!(
        handler_rules(&[("a.rb", &synthetic), ("b.rb", &same)]),
        (vec!["duplicate-handler".to_owned()], false)
    );
}

/// Syntax that binds no local never marks later handlers uncertain; binding syntax still does.
#[test]
fn unbinding_syntax_before_handlers_keeps_comparison() {
    let pair = "Then('the parcel status is verified') { |state| work(1) }\nThen('the parcel status is now verified') { |state| work(1) }";
    for (label, prelude, incomplete) in [
        (
            "bare rescue",
            "begin\n  risky\nrescue\n  recover\nend",
            false,
        ),
        (
            "rescue with a variable",
            "begin\n  risky\nrescue => error\n  recover(error)\nend",
            true,
        ),
        ("string on the left of =~", "ENV['X'] =~ /y/", false),
        (
            "named group on the right of =~",
            "ENV['X'] =~ /(?<found>y)/",
            false,
        ),
        (
            "named group regex literal on the left",
            "/(?<found>y)/ =~ ENV['X']",
            true,
        ),
        (
            "quoted named group on the left",
            "/(?'found'y)/ =~ ENV['X']",
            true,
        ),
        ("lookbehind on the left", "/(?<=a)y/ =~ ENV['X']", false),
        (
            "escaped group text on the left",
            "/\\(?<found>y/ =~ ENV['X']",
            false,
        ),
        (
            "character class on the left",
            "/[(?<found>]y/ =~ ENV['X']",
            false,
        ),
        (
            "named group after a class on the left",
            "/[(]x(?<found>y)/ =~ ENV['X']",
            true,
        ),
        (
            "interpolated named group on the left",
            "value = 'y'\n/(?<found>#{value})/ =~ ENV['X']",
            false,
        ),
        (
            "regex held in a variable",
            "pattern = /(?<found>y)/\npattern =~ ENV['X']",
            false,
        ),
        ("for loop", "for item in [1]\n  use(item)\nend", true),
    ] {
        let (rules, observed) = handler_rules(&[("steps.rb", &format!("{LOAD}{prelude}\n{pair}"))]);
        assert_eq!(observed, incomplete, "{label}");
        assert_eq!(
            rules.contains(&"duplicate-handler".to_owned()),
            !incomplete,
            "{label}"
        );
    }
    // A `def` handler does not close over file locals, so file-level binding syntax is irrelevant.
    let methods = format!("{LOAD}begin\n  risky\nrescue => error\n  recover(error)\nend\ndef first\n  work(1)\nend\ndef second\n  work(1)\nend\nThen('the parcel status is verified', &method(:first))\nThen('the parcel status is now verified', &method(:second))");
    assert_eq!(
        handler_rules(&[("steps.rb", &methods)]),
        (
            vec![
                "duplicate-handler".to_owned(),
                "near-duplicate-step".to_owned()
            ],
            false
        )
    );
}

/// Configured assertion evidence keeps the complete-syntax stream; a mixed pair never compares.
#[test]
fn configured_assertion_streams_do_not_mix_with_action_streams() {
    for (label, first, second, expected) in [
        (
            "configured polarity",
            "SyntheticAssertions.expect(state).to_be(1)",
            "SyntheticAssertions.expect(state).not.to_be(1)",
            vec![],
        ),
        (
            "configured values",
            "SyntheticAssertions.expect(state).to_be(1)",
            "SyntheticAssertions.expect(state).to_be(2)",
            vec![],
        ),
        (
            "assertion versus action",
            "SyntheticAssertions.expect(state).to_be(1)",
            "work(state)",
            vec![],
        ),
        (
            "action pair beside configured trust",
            "page.goto('/login')",
            "page.goto('/login'); page.wait_for_load_state()",
            vec!["near-duplicate-step"],
        ),
        // A receiverless `expect` beside a configured factory is checked against the factory
        // positions and, being an ordinary helper, keeps the exact policy.
        (
            "ordinary helper named expect beside a configured factory",
            "SyntheticAssertions.expect(state).to_be(1)",
            "expect(state).run()",
            vec![],
        ),
        // The exact policy wins over configured evidence in the same handler: an untrusted
        // polarity flip beside a trusted assertion is neither near nor a candidate, while the
        // identical pair still duplicates.
        (
            "trusted assertion beside untrusted polarity",
            "SyntheticAssertions.expect(state).to_be(1); expect(x).to eq(1)",
            "SyntheticAssertions.expect(state).to_be(1); expect(x).not_to eq(1)",
            vec![],
        ),
        (
            "trusted assertion beside identical untrusted chain",
            "SyntheticAssertions.expect(state).to_be(1); expect(x).to eq(1)",
            "SyntheticAssertions.expect(state).to_be(1); expect(x).to eq(1)",
            vec!["duplicate-handler", "near-duplicate-step"],
        ),
        // Shared action anchors do not bridge the two regimes.
        (
            "assertion versus action with shared anchors",
            "page.goto('/x'); SyntheticAssertions.expect(state).to_be(1)",
            "page.goto('/x'); page.wait()",
            vec![],
        ),
        (
            "two ordinary expect helpers beside trust",
            "expect(state).run(); SyntheticAssertions.expect(state).to_be(1)",
            "expect(state).run(); flush(); SyntheticAssertions.expect(state).to_be(1)",
            vec![],
        ),
    ] {
        let steps = format!("{LOAD}{}", step_pair(first, second));
        let (rules, incomplete) = handler_rules(&[("steps.rb", &steps)]);
        assert_eq!(rules, expected, "{label}");
        assert!(!incomplete, "{label}");
    }
}
