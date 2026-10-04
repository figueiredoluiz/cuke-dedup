use assert_cmd::Command;
use serde_json::Value;
use std::fs;

const PROVIDER: &str = include_str!(
    "../fixtures/ruby-parity/equivalent-handlers/precision-inline-local/providers/assertions.rb"
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
        fs::write(root.path().join("steps.rb"), format!("require_relative '{load}'\ncheck = SyntheticAssertions.method(:expect)\nThen('the parcel status is verified') {{ |state| register(-> {{ check.call(state).to_be('ready') }}) }}\nThen('the parcel status is now verified') {{ |state| other_wrapper([proc {{ check.call(state).to_be('ready') }}]) }}\n")).unwrap();
        let rows = analyze(root.path(), "steps.rb");
        assert_eq!(handler_findings(&rows), expected, "{configured} / {file}");
        assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
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
