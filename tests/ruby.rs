use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command as ProcessCommand;

mod common;

fn project_command(root: &std::path::Path, patterns: &str) -> Command {
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command.arg(root).args([
        "--definitions",
        patterns,
        "--reporters",
        "jsonl",
        "--no-metrics",
    ]);
    command
}

fn run_project(root: &std::path::Path, patterns: &str, extra: &[&str]) -> std::process::Output {
    project_command(root, patterns)
        .args(extra)
        .output()
        .unwrap()
}

fn records(stdout: Vec<u8>) -> Vec<Value> {
    String::from_utf8(stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn project_records(root: &std::path::Path) -> Vec<Value> {
    records(run_project(root, "*.rb", &[]).stdout)
}

fn analyze(source: &str, extra: &[&str]) -> (i32, Value, String) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("steps.rb"), source).unwrap();
    let output = run_project(dir.path(), "*.rb", extra);
    let records = records(output.stdout);
    (
        output.status.code().unwrap(),
        Value::Array(records),
        String::from_utf8(output.stderr).unwrap(),
    )
}

fn assert_handler_finding(rows: &Value, expected: bool, source: &str) {
    assert_eq!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|r| r["rule"] == "duplicate-handler"),
        expected,
        "{source}"
    );
}

fn write_registration_provider_fixture(root: &std::path::Path) {
    fs::write(root.join("provider.rb"), "REGISTER = method(:Given)").unwrap();
    fs::write(
        root.join("consumer.rb"),
        "register = REGISTER; register.call('same') { work() }; register.call('same') { work() }",
    )
    .unwrap();
}

fn assert_resolved_handler_source(source: &str, count: usize) {
    let (_, rows, _) = analyze(source, &[]);
    assert_discovery(rows.as_array().unwrap(), count, count == 0, source);
    assert_handler_finding(&rows, count > 0, source);
}

fn assert_discovery(rows: &[Value], definitions: usize, incomplete: bool, source: &str) {
    let summary = rows.last().unwrap();
    assert_eq!(
        summary["summary"]["definitionsAnalyzed"], definitions,
        "{source}"
    );
    assert_eq!(summary["corpus"]["incomplete"], incomplete, "{source}");
}

/// Checks CLI findings for transparent forwarding and rejects unsupported execution or argument shapes.
#[test]
fn ruby_transparent_wrapper_outcomes_preserve_forwarding_and_reject_uncertainty() {
    for keyword in ["Given", "When", "Then", "And", "But"] {
        for body in ["Given(text, &handler)", "Given text, &handler"] {
            let source = format!("def wrap(text, &handler); {body}; end\nwrap('same') {{ first() }}\nwrap('same') do; second(); end").replace("Given", keyword);
            let (exit, rows, _) = analyze(&source, &["--fail-on-incomplete"]);
            let rows = rows.as_array().unwrap();
            assert_discovery(rows, 2, false, &source);
            assert_eq!(exit, 1, "{source}");
            assert_eq!(
                rows.iter()
                    .filter(|r| r["rule"] == "duplicate-matcher")
                    .count(),
                1,
                "{source}"
            );
            assert!(
                !rows.iter().any(|r| r["rule"] == "duplicate-handler"),
                "{source}"
            );
        }
    }
    for (declaration, calls, definitions, incomplete, handler) in [
        (
            "def wrap(value, &block) = Given(value, &block)",
            "wrap('alpha') { work() }; wrap('omega') { work() }",
            2,
            false,
            true,
        ),
        (
            "def wrap(text, &text); Given(text, &text); end",
            "wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); if ready; Given(text, &handler); end; end",
            "wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); foreign.Given(text, &handler); end",
            "wrap('alpha') { work() }",
            0,
            false,
            false,
        ),
        (
            "def wrap(value, &block); Given(value, &block); end",
            "wrap('alpha') { work() }; wrap('omega') { work() }",
            2,
            false,
            true,
        ),
        (
            "def wrap(text, &handler); Given(text, &handler); end; def ordinary; :local; end",
            "Given('alpha') { work() }; Given('omega') { work() }",
            2,
            false,
            true,
        ),
        (
            "def wrap(text, &handler); :ignored; end",
            "wrap('alpha') { work() }; wrap('omega') { work() }",
            0,
            false,
            false,
        ),
        (
            "def wrap(text, &handler); extra(); Given(text, &handler); end",
            "wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); Given(text, &handler); extra(); end",
            "wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); Given(transform(text), &handler); end",
            "wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); Given(text, &other); end",
            "wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text = 'fallback', &handler); Given(text, &handler); end",
            "wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(*text, &handler); Given(text, &handler); end",
            "wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); Given(text, &handler); end",
            "if ready; wrap('alpha') { work() }; end",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); Given(text, &handler); end",
            "def later; wrap('alpha') { work() }; end",
            0,
            true,
            false,
        ),
        (
            "wrap('early') { work() }; def wrap(text, &handler); Given(text, &handler); end",
            "wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); Given(text, &handler); end",
            "foreign.wrap('alpha') { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); Given(text, &handler); end",
            "wrap(dynamic) { work() }",
            0,
            true,
            false,
        ),
        (
            "def wrap(text, &handler); Given(text, &handler); end",
            "wrap('alpha', &handler)",
            0,
            true,
            false,
        ),
    ] {
        let source = format!("{declaration}\n{calls}");
        let (exit, rows, _) = analyze(&source, &["--fail-on-incomplete"]);
        let rows = rows.as_array().unwrap();
        assert_discovery(rows, definitions, incomplete, &source);
        assert_eq!(
            exit,
            if incomplete { 2 } else { i32::from(handler) },
            "{source}"
        );
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "duplicate-handler"),
            handler,
            "{source}"
        );
        assert!(
            !rows
                .iter()
                .any(|r| r["rule"] == "parameterization-candidate"),
            "{source}"
        );
    }
}

/// Requires order-independent rejection of conflicting providers while retaining unrelated-helper positives.
#[test]
fn ruby_wrapper_ownership_is_checked_across_selected_sources_in_both_orders() {
    let registrar = "def wrap(text, &handler); Given(text, &handler); end\nwrap('same') { first() }; wrap('same') { second() }";
    for (interference, incomplete) in [
        ("def wrap(text, &handler); :ignored; end", true),
        ("alias wrap replacement", true),
        ("undef wrap", true),
        ("define_method(:wrap) { :ignored }", true),
        ("send(:define_method, :wrap) { :ignored }", true),
        ("remove_method(:wrap)", true),
        ("alias_method(:wrap, :replacement)", true),
        ("attr_reader(:wrap)", true),
        ("attr_accessor(dynamic)", true),
        ("wrap('elsewhere') { work() }", true),
        ("def ordinary; :local; end", false),
        ("worker = -> { :ordinary }; worker.()", false),
    ] {
        for sources in [[registrar, interference], [interference, registrar]] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("a.rb"), sources[0]).unwrap();
            fs::write(dir.path().join("b.rb"), sources[1]).unwrap();
            let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
            assert_eq!(
                output.status.code(),
                Some(if incomplete { 2 } else { 1 }),
                "{interference}"
            );
            let rows = records(output.stdout);
            assert_discovery(
                &rows,
                if incomplete { 0 } else { 2 },
                incomplete,
                interference,
            );
            assert!(
                rows.iter().any(|r| r["rule"] == "duplicate-matcher") != incomplete,
                "{interference}"
            );
        }
    }
}

/// Preserves known registrations when deferred dispatch makes indirect usage incomplete.
#[test]
fn ruby_wrapper_deferred_dispatch_retains_valid_registrations() {
    let source = "def wrap(text, &handler); Given(text, &handler); end\nwrap('same') { send(dynamic) }; wrap('same') { other() }";
    let (exit, rows, _) = analyze(source, &["--fail-on-incomplete"]);
    let rows = rows.as_array().unwrap();
    assert_discovery(rows, 2, true, source);
    assert_eq!(exit, 2);
    assert_eq!(
        rows.iter()
            .filter(|r| r["rule"] == "duplicate-matcher")
            .count(),
        1
    );
}

#[test]
fn ruby_cucumber_normalization_preserves_matcher_and_handler_boundaries() {
    for (left, right, normalized, exact, definitions, incomplete) in [
        ("'a   panel'", "'a panel'", true, false, 2, false),
        ("'the Ａ panel'", "'the A panel'", true, false, 2, false),
        (
            "'a { word } panel'",
            "'a {word} panel'",
            true,
            false,
            2,
            false,
        ),
        ("'a panel'", "'a panel'", false, true, 2, false),
        ("'a panel'", "'another panel'", false, false, 2, false),
        ("'a panel'", "/a panel/", false, false, 2, false),
        ("/a   panel/", "/a panel/", false, false, 2, false),
        ("/a panel/m", "/a panel/", false, false, 2, false),
        ("\"a #{value} panel\"", "'a panel'", false, false, 1, true),
    ] {
        let source = format!("Given({left}) {{ page.open() }}; Given({right}) {{ page.close() }}");
        let (code, records, _) = analyze(&source, &["--fail-on-incomplete"]);
        let rows = records.as_array().unwrap();
        assert_discovery(rows, definitions, incomplete, &source);
        assert_eq!(
            code,
            if incomplete {
                2
            } else {
                i32::from(normalized || exact)
            },
            "{source}"
        );
        for (rule, present) in [
            ("normalized-matcher", normalized),
            ("duplicate-matcher", exact),
            ("duplicate-handler", false),
            ("near-duplicate-step", false),
            ("parameterization-candidate", false),
        ] {
            assert_eq!(
                rows.iter().filter(|row| row["rule"] == rule).count(),
                usize::from(present),
                "{source}: {rule}"
            );
        }
    }
}

#[test]
fn ruby_final_outcomes_have_positive_and_conflicting_controls() {
    for (source, definitions, handler, incomplete) in [
        (
            include_str!("../fixtures/ruby/identical/steps.rb"),
            2,
            true,
            false,
        ),
        (
            include_str!("../fixtures/ruby/conflicting/steps.rb"),
            2,
            false,
            false,
        ),
        (
            "Given('a') { store.write('x') }; Then('b') { store.write('x') }",
            2,
            true,
            false,
        ),
        (
            "Given(/a/) { store.write('x') }; Then(%r{b}) { store.write('x') }",
            2,
            true,
            false,
        ),
        ("Given 'a' do; store.write('x'); end", 1, false, false),
        ("object.Given('a') { store.write('x') }", 0, false, false),
        (
            "def Given(x); end; Given('a') { store.write('x') }",
            0,
            false,
            true,
        ),
        (
            "def later; Given('a') { store.write('x') }; end",
            0,
            false,
            true,
        ),
        ("Given(\"a #{value}\") { store.write('x') }", 0, false, true),
        (
            "require 'aruba/cucumber'; Given('a') { store.write('x') }",
            1,
            false,
            true,
        ),
    ] {
        let (_, records, _) = analyze(source, &[]);
        let rows = records.as_array().unwrap();
        assert_discovery(rows, definitions, incomplete, source);
        assert_eq!(
            rows.iter().any(|v| v["rule"] == "duplicate-handler"),
            handler,
            "{source}: {records}"
        );
        assert!(!rows
            .iter()
            .any(|v| v["rule"] == "parameterization-candidate"));
        if !handler {
            assert!(!rows.iter().any(|v| v["rule"] == "near-duplicate-step"));
        }
    }
}

#[test]
fn ruby_strictness_and_duplicate_matchers_retain_partial_findings() {
    let source = "require 'external/glue'; Given('same') { read() }; Then('same') { write() }";
    let (code, records, stderr) = analyze(source, &["--fail-on-incomplete"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(records
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["rule"] == "duplicate-matcher"));
    assert_eq!(
        records.as_array().unwrap().last().unwrap()["corpus"]["incomplete"],
        true
    );
}

#[test]
fn ruby_exact_handlers_preserve_semantics_and_refuse_dynamic_registrations() {
    for (left, right) in [
        ("store.write('x')", "other.write('x')"),
        ("@first = read()", "@second = read()"),
        ("a(); b()", "b(); a()"),
        ("expect(value).to eq(1)", "expect(value).to eq(2)"),
        ("outer inner", "outer\ninner"),
    ] {
        let (_, records, _) = analyze(
            &format!("Given('a') {{ {left} }}; Then('b') {{ {right} }}"),
            &[],
        );
        let rows = records.as_array().unwrap();
        assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
        assert!(
            !rows.iter().any(|v| v["rule"] == "duplicate-handler"),
            "{left} / {right}"
        );
    }
    for source in [
        "if enabled; Given('a') { read() }; end",
        "factory { Given('a') { read() } }",
        "Given(pattern) { read() }",
        "eval(code); Given('a') { read() }",
        "alias Given other; Given('a') { read() }",
        "class Object; alias_method :Given, :other; end; Given('a') { read() }",
        "Given(/a#{suffix}/) { read() }",
    ] {
        let (code, records, _) = analyze(source, &["--fail-on-incomplete"]);
        let summary = records.as_array().unwrap().last().unwrap();
        assert_eq!(code, 2, "{source}");
        assert_eq!(summary["summary"]["definitionsAnalyzed"], 0, "{source}");
        assert_eq!(summary["corpus"]["incomplete"], true, "{source}");
    }
}

#[test]
fn ruby_usage_respects_anchors_and_keeps_unsupported_patterns_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("suite.feature"),
        "Feature: tokens\n Scenario: token\n  Given value 12\n",
    )
    .unwrap();
    for (matcher, flags, unused, incomplete) in [
        (r"\Avalue \d+\z", "", false, false),
        ("^different$", "", true, false),
        ("^different$", "i", false, true),
        (r"value (\d+)\1", "", false, true),
        ("value 1++2", "", false, true),
        ("value 1+2", "", false, false),
    ] {
        fs::write(
            dir.path().join("steps.rb"),
            format!("Given(/{matcher}/{flags}) {{ work() }}"),
        )
        .unwrap();
        let rows = project_records(dir.path());
        assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 1);
        assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], incomplete);
        assert_eq!(
            rows.iter().any(|v| v["rule"] == "unused-definition"),
            unused,
            "{matcher}/{flags}"
        );
    }
}

#[test]
fn ruby_requires_opt_in_and_rejects_mixed_language_scope() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("steps.rb"), "Given('same') { work() }").unwrap();
    let default = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(dir.path())
        .args(["--require-definitions"])
        .output()
        .unwrap();
    assert_eq!(default.status.code(), Some(2));
    fs::write(dir.path().join("steps.js"), "Given('same', () => work());").unwrap();
    let mixed = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(dir.path())
        .args(["--definitions", "*.rb,*.js"])
        .output()
        .unwrap();
    assert_eq!(mixed.status.code(), Some(2));
    assert!(String::from_utf8(mixed.stderr)
        .unwrap()
        .contains("separate analysis runs"));
}

#[test]
fn ruby_cross_file_redefinition_invalidates_earlier_registrations() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("a.rb"),
        "Given('a') { work() }; Then('b') { work() }",
    )
    .unwrap();
    fs::write(dir.path().join("z.rb"), "def Given(x); end").unwrap();
    let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
    assert_eq!(output.status.code(), Some(2));
    let rows = records(output.stdout);
    let summary = rows.last().unwrap();
    assert_eq!(summary["summary"]["definitionsAnalyzed"], 0);
    assert_eq!(summary["summary"]["findings"], 0);
    assert_eq!(summary["corpus"]["incomplete"], true);
}

#[test]
fn ruby_parse_failure_preserves_valid_neighbor_findings_and_strict_exit() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("features/support")).unwrap();
    fs::write(
        dir.path().join("features/support/steps.rb"),
        "Given('same') { first() }; Then('same') { second() }",
    )
    .unwrap();
    fs::write(dir.path().join("broken.rb"), "Given('broken') do\n value =").unwrap();
    let output = run_project(dir.path(), "**/*.rb", &["--fail-on-unparseable"]);
    assert_eq!(output.status.code(), Some(2));
    let rows = records(output.stdout);
    assert!(rows.iter().any(|v| v["rule"] == "duplicate-matcher"));
    assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], true);
    assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
}

#[test]
fn ruby_indirect_usage_final_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("suite.feature"),
        "Feature: usage\n Scenario: caller\n  Given caller\n",
    )
    .unwrap();
    for (call, target_unused, unrelated_unused, incomplete) in [
        ("step 'target'", false, true, false),
        ("step(\"target\")", false, true, false),
        ("self.step('target', table)", false, true, false),
        ("later { step 'target' }", false, true, false),
        ("message = \"step 'target'\"", true, true, false),
        ("# step 'target'\n work()", true, true, false),
        ("step name", false, false, true),
        ("__send__(method_name)", false, false, true),
        ("method(:step).call(name)", false, false, true),
        ("step 'target'; step name", false, false, true),
        ("step %q{target}", false, false, true),
        (
            "def step(x); super('elsewhere'); end; step 'target'",
            false,
            false,
            true,
        ),
        ("step \"#{name}\"", false, false, true),
        ("steps 'Given target'", false, false, true),
        ("object.step('target')", false, false, true),
        ("send(:step, name)", false, false, true),
    ] {
        fs::write(dir.path().join("steps.rb"), format!("Given('caller') do\n {call}\nend\nThen('target') {{ target_work() }}\nThen('unrelated') {{ other_work() }}\n")).unwrap();
        let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
        let rows = records(output.stdout);
        let unused = |name: &str| {
            rows.iter().any(|row| {
                row["rule"] == "unused-definition"
                    && row["message"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("`{name}`"))
            })
        };
        assert_eq!(unused("target"), target_unused, "{call}: {rows:?}");
        assert_eq!(unused("unrelated"), unrelated_unused, "{call}: {rows:?}");
        assert_eq!(rows.last().unwrap()["summary"]["featureStepsAnalyzed"], 1);
        assert_eq!(
            rows.last().unwrap()["corpus"]["incomplete"],
            incomplete,
            "{call}"
        );
        if incomplete {
            assert_eq!(output.status.code(), Some(2), "{call}");
        }
    }
}

#[test]
fn ruby_support_file_calls_preserve_unrelated_unused_and_duplicate_findings() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("suite.feature"),
        "Feature: usage\n Scenario: caller\n  Given caller\n",
    )
    .unwrap();
    fs::write(dir.path().join("steps.rb"), "Given('caller') { helper() }; Then('target') { target_work() }; Then('unrelated') { other_work() }; Then('unrelated') { another_work() }").unwrap();
    for file in ["a.rb", "z.rb"] {
        for (call, unused_count) in [("step('target')", 2), ("step(name)", 0)] {
            fs::write(dir.path().join(file), format!("def helper; {call}; end")).unwrap();
            let rows = records(run_project(dir.path(), "*.rb", &[]).stdout);
            assert_eq!(
                rows.iter()
                    .filter(|row| row["rule"] == "unused-definition")
                    .count(),
                unused_count,
                "{file}: {call}"
            );
            assert!(rows.iter().any(|row| row["rule"] == "duplicate-matcher"));
            fs::remove_file(dir.path().join(file)).unwrap();
        }
    }
}

#[test]
fn ruby_dispatch_context_retains_only_supported_registration_findings() {
    let dir = tempfile::tempdir().unwrap();
    for (source, retained) in [
        ("settings.send(:define_method, :feature_files) { [] }", true),
        (
            "settings.public_send('undef_method', 'feature_files')",
            true,
        ),
        ("settings.define_method(:feature_files) { [] }", true),
        (
            "settings.send(:send, :define_method, :feature_files) { [] }",
            true,
        ),
        ("settings.send(:define_method, :Given) { nil }", false),
        ("settings.public_send('remove_method', 'Then')", false),
        (
            "settings.send(:send, :define_method, :Given) { nil }",
            false,
        ),
        ("settings.send(:define_method, name) { nil }", false),
        ("settings.send(name)", false),
        ("settings.remove_method(:unrelated, :Given)", false),
        ("settings.send(:undef_method, :unrelated, :Then)", false),
        (
            "settings.send(:define_method, :define_method) { nil }",
            false,
        ),
        ("settings.send(:remove_method, *names)", false),
        (
            "settings.__send__(:define_method, :feature_files) { [] }",
            true,
        ),
        ("settings.__send__(:define_method, :Given) { nil }", false),
        ("settings.define_method(:method_missing) { nil }", false),
        (
            "settings.method(:define_method).call(:Given) { nil }",
            false,
        ),
        (
            "ParameterType(name: 'x', transformer: lambda { |v| v.send(property) })",
            true,
        ),
        (
            "ParameterType(name: 'x', transformer: proc { |v| v.send(property) })",
            true,
        ),
        (
            "ParameterType(name: 'x', transformer: ->(v) { v.send(property) })",
            true,
        ),
        (
            "ParameterType(name: 'x', transformer: lambda { |v| v.send(property) }.call)",
            false,
        ),
        (
            "ParameterType(name: 'x', transformer: wrap(lambda { |v| v.send(property) }))",
            false,
        ),
        (
            "ParameterType(name: 'x', other: lambda { |v| v.send(property) })",
            false,
        ),
        (
            "Other(name: 'x', transformer: lambda { |v| v.send(property) })",
            false,
        ),
        (
            "def lambda; end; ParameterType(transformer: lambda { send(name) })",
            false,
        ),
        ("Given(send(name)) { work() }", false),
        (
            "ParameterType(name: 'x', regexp: /x/, transformer: lambda() { send(name) })",
            true,
        ),
        (
            "def ParameterType(x); end; ParameterType(transformer: -> { send(name) })",
            false,
        ),
    ] {
        for support in ["a.rb", "z.rb"] {
            fs::write(
                dir.path().join("steps.rb"),
                "Given('first') { store.write('same') }; Then('second') { store.write('same') }",
            )
            .unwrap();
            fs::write(dir.path().join(support), source).unwrap();
            let rows = project_records(dir.path());
            assert_eq!(
                rows.last().unwrap()["summary"]["definitionsAnalyzed"],
                if retained { 2 } else { 0 },
                "{source}: {rows:?}"
            );
            assert_eq!(
                rows.iter().any(|r| r["rule"] == "duplicate-handler"),
                retained,
                "{source}"
            );
            fs::remove_file(dir.path().join(support)).unwrap();
        }
    }
}

#[test]
fn ruby_static_dependency_graph_final_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("lib")).unwrap();
    fs::write(
        dir.path().join("suite.feature"),
        "Feature: graph\n Scenario: used\n  Given local\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("lib/provider.rb"),
        "Given('provider') { work() }",
    )
    .unwrap();
    fs::write(
        dir.path().join(".cuke-dedup.json"),
        r#"{"rubyLoadPaths":["lib"]}"#,
    )
    .unwrap();
    for (load, count, incomplete) in [
        ("require_relative 'lib/provider'", 2, false),
        ("require 'provider'", 2, false),
        ("require_relative 'missing'", 1, true),
        ("require name", 1, true),
        ("Kernel.require 'provider'", 1, true),
        ("def later; require 'provider'; end", 1, true),
        ("load 'lib/provider.rb'", 1, true),
        ("def require(x); end; require 'provider'", 0, true),
    ] {
        fs::write(
            dir.path().join("entry.rb"),
            format!("{load}\nGiven('local') {{ work() }}\n"),
        )
        .unwrap();
        let output = run_project(dir.path(), "entry.rb", &["--fail-on-incomplete"]);
        let rows = records(output.stdout);
        assert_eq!(
            rows.last().unwrap()["summary"]["definitionsAnalyzed"],
            count,
            "{load}: {rows:?}"
        );
        assert_eq!(
            rows.last().unwrap()["corpus"]["incomplete"],
            incomplete,
            "{load}"
        );
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "duplicate-handler"),
            count == 2,
            "{load}"
        );
        if count == 2 {
            assert!(rows.iter().any(|r| r["rule"] == "unused-definition"));
        }
        if incomplete {
            assert_eq!(output.status.code(), Some(2), "{load}");
        }
    }
}

#[test]
fn ruby_dependency_cycles_load_order_and_exclusions_preserve_outcomes() {
    let dir = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("lib/blocked")).unwrap();
    fs::write(
        dir.path().join("entry.rb"),
        "require 'provider'\nGiven('local') { work() }",
    )
    .unwrap();
    fs::write(
        dir.path().join("lib/provider.rb"),
        "require_relative '../entry'\nGiven('provider') { work() }",
    )
    .unwrap();
    fs::write(
        external.path().join("provider.rb"),
        "Given('external') { different() }",
    )
    .unwrap();
    for (paths, handler) in [
        (
            vec![dir.path().join("lib"), external.path().to_owned()],
            true,
        ),
        (
            vec![external.path().to_owned(), dir.path().join("lib")],
            false,
        ),
    ] {
        fs::write(
            dir.path().join(".cuke-dedup.json"),
            serde_json::json!({"rubyLoadPaths":paths}).to_string(),
        )
        .unwrap();
        let rows = records(run_project(dir.path(), "entry.rb", &[]).stdout);
        assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
        assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], false);
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "duplicate-handler"),
            handler
        );
    }
    fs::write(
        dir.path().join("lib/blocked/provider.rb"),
        "Given('provider') { work() }",
    )
    .unwrap();
    fs::write(
        dir.path().join("entry.rb"),
        "require_relative 'lib/blocked/provider'\nGiven('local') { work() }",
    )
    .unwrap();
    fs::write(
        dir.path().join(".cuke-dedup.json"),
        r#"{"exclude":["lib/blocked"]}"#,
    )
    .unwrap();
    let output = run_project(dir.path(), "entry.rb", &["--fail-on-incomplete"]);
    assert_eq!(output.status.code(), Some(2));
    let rows = records(output.stdout);
    assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 1);
    assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], true);
}

#[test]
fn ruby_dependency_boundaries_and_parse_failures_keep_independent_findings() {
    let dir = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    fs::write(
        external.path().join("provider.rb"),
        "Given('external') { work() }",
    )
    .unwrap();
    fs::write(dir.path().join("broken.rb"), "Given('broken') do\n").unwrap();
    fs::write(dir.path().join("native.so"), "not Ruby source").unwrap();
    let outside = external.path().join("provider.rb");
    let requests = [
        format!("require '{}'", outside.display()),
        "require_relative 'native.so'".to_owned(),
        "require_relative 'broken'".to_owned(),
    ];
    for request in requests {
        fs::write(
            dir.path().join("entry.rb"),
            format!("{request}\nGiven('first') {{ work() }}\nThen('second') {{ work() }}"),
        )
        .unwrap();
        let output = run_project(dir.path(), "entry.rb", &["--fail-on-incomplete"]);
        assert_eq!(output.status.code(), Some(2), "{request}");
        let rows = records(output.stdout);
        assert_eq!(
            rows.last().unwrap()["summary"]["definitionsAnalyzed"],
            2,
            "{request}"
        );
        assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], true);
        assert!(
            rows.iter().any(|r| r["rule"] == "duplicate-handler"),
            "{request}"
        );
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside, dir.path().join("linked.rb")).unwrap();
        fs::write(
            dir.path().join("entry.rb"),
            "require_relative 'linked'\nGiven('local') { work() }",
        )
        .unwrap();
        let rows = records(run_project(dir.path(), "entry.rb", &["--fail-on-incomplete"]).stdout);
        assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 1);
        assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], true);
    }
}

#[test]
fn ruby_dependency_usage_requires_a_complete_source_graph() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("support")).unwrap();
    fs::write(
        dir.path().join("suite.feature"),
        "Feature: usage\n Scenario: entry\n  Given caller\n",
    )
    .unwrap();
    for (load, provider, exclude, unused, incomplete) in [
        ("", b"".as_slice(), false, 2, false),
        (
            "require_relative 'support/provider'",
            b"def delegate; step('target'); end".as_slice(),
            false,
            1,
            false,
        ),
        ("require_relative 'missing'", b"".as_slice(), false, 0, true),
        ("require path", b"".as_slice(), false, 0, true),
        (
            "require_relative 'support/provider'",
            b"def delegate; step(target_name); end".as_slice(),
            false,
            0,
            true,
        ),
        (
            "require_relative 'support/provider'",
            b"\xff".as_slice(),
            false,
            0,
            true,
        ),
        (
            "require_relative 'support/provider'",
            b"def delegate; step('target'); end".as_slice(),
            true,
            0,
            true,
        ),
        (
            "require_relative 'support/provider'",
            b"\0".as_slice(),
            false,
            0,
            true,
        ),
        (
            "require_relative 'support/provider'",
            b"def broken(".as_slice(),
            false,
            0,
            true,
        ),
    ] {
        fs::write(dir.path().join("support/provider.rb"), provider).unwrap();
        fs::write(
            dir.path().join(".cuke-dedup.json"),
            serde_json::json!({"exclude": if exclude { vec!["support/**"] } else { vec![] }})
                .to_string(),
        )
        .unwrap();
        fs::write(dir.path().join("entry.rb"), format!("{load}\nGiven('caller') {{ first() }}; Then('caller') {{ second() }}; Then('target') {{ target_work() }}; Then('unrelated') {{ other_work() }}")).unwrap();
        let output = run_project(dir.path(), "entry.rb", &["--fail-on-incomplete"]);
        let rows = records(output.stdout);
        assert_eq!(
            rows.iter()
                .filter(|r| r["rule"] == "unused-definition")
                .count(),
            unused,
            "{load}, excluded={exclude}, provider={provider:?}"
        );
        assert!(rows.iter().any(|r| r["rule"] == "duplicate-matcher"));
        assert!(rows.iter().any(|r| r["rule"] == "ambiguous-step"));
        assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], incomplete);
        assert_eq!(output.status.code(), Some(if incomplete { 2 } else { 1 }));
    }
}

#[test]
fn ruby_regex_classes_and_dotall_preserve_final_matching_outcomes() {
    for (pattern, flags, value, matches) in [
        (r"\Avalue %r{(.*)}\z", "", "%r{text}", Some(true)),
        (r"\Avalue %r{(.*)}\z", "", "%rtext", Some(false)),
        (r"\Avalue a{2}\z", "", "aa", Some(true)),
        (r"\Avalue a{2}\z", "", "a{2}", Some(false)),
        (r"\Avalue a{2,3}\z", "", "aaa", Some(true)),
        (r"\Avalue a{2,}\z", "", "aaaa", Some(true)),
        (r"\Avalue a{,3}\z", "", "aa", Some(true)),
        (r"\Avalue [{2}]\z", "", "{", Some(true)),
        (r"\Avalue []a{,3}]\z", "", "0", Some(false)),
        (r"\Avalue []a{,3}]\z", "", "]", Some(true)),
        (r"\Avalue a\{2\}\z", "", "a{2}", Some(true)),
        (r"\Avalue a{z}\z", "", "a{z}", Some(true)),
        (r#"\Avalue \"([^\"]*)\"\z"#, "", "\"quoted\"", Some(true)),
        (r#"\Avalue \"([^\"]*)\"\z"#, "", "unquoted", Some(false)),
        (r"\Avalue \'text\'\z", "", "'text'", Some(true)),
        (r"\Avalue \#\!\z", "", "#!", Some(true)),
        (r"\Avalue [a\-z]+\z", "", "a-z", Some(true)),
        (r"\Avalue [a\-z]+\z", "", "m", Some(false)),
        (r"\Avalue a\-z\z", "", "a-z", Some(true)),
        (r"\Avalue a\.b\z", "", "axb", Some(false)),
        (r"\Avalue a\.b\z", "", "a.b", Some(true)),
        (r"\Avalue \q\z", "", "q", None),
        (r"\Avalue \w+\z", "", "alpha_19", Some(true)),
        (r"\Avalue \w+\z", "", "日本", Some(false)),
        (r"\Avalue [\w]+\z", "", "alpha_19", Some(true)),
        (r"\Avalue \W+\z", "", "日本", Some(true)),
        (r"\Avalue [\W]+\z", "", "alpha", Some(false)),
        (r"\Avalue a\sb\z", "", "a b", Some(true)),
        (r"\Avalue a\sb\z", "", "a\u{a0}b", Some(false)),
        (r"\Avalue a\Sb\z", "", "a\u{a0}b", Some(true)),
        (r"\Avalue \D+\z", "", "１２", Some(true)),
        (r"\Avalue \d+\z", "", "１２", Some(false)),
        (r"\Avalue (?:red|blue)\z", "", "blue", Some(true)),
        (r"\Avalue (?:red|blue)\z", "", "green", Some(false)),
        (r"\Avalue a.b\z", "m", "a\nb", Some(true)),
        (r"\Avalue a.b\z", "", "a\nb", Some(false)),
        (r"^value a$", "m", "a\nb", Some(true)),
        (r"\Avalue a.b\z", "i", "axb", None),
        (r"\Avalue (?=a)a\z", "", "a", None),
    ] {
        for (open, close) in [("/", "/"), ("%r{", "}")] {
            let dir = tempfile::tempdir().unwrap();
            let literal = serde_json::to_string(&format!("value {value}")).unwrap();
            fs::write(dir.path().join("steps.rb"), format!(
                "Given({open}{pattern}{close}{flags}) {{ first() }}\nThen({open}{pattern}{close}{flags}) {{ second() }}\nThen({literal}) {{ control() }}"
            )).unwrap();
            let cell = value.replace('\\', "\\\\").replace('\n', "\\n");
            fs::write(dir.path().join("suite.feature"), format!(
                "Feature: matching\n Scenario Outline: value\n  Given value <text>\n Examples:\n  | text |\n  | {cell} |\n"
            )).unwrap();
            let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
            let rows = records(output.stdout);
            let has = |rule| rows.iter().any(|r| r["rule"] == rule);
            assert!(has("duplicate-matcher"), "{pattern}/{flags}: {value}");
            assert_eq!(
                has("ambiguous-step"),
                matches == Some(true),
                "{pattern}/{flags}: {value}"
            );
            assert_eq!(
                has("unused-definition"),
                matches == Some(false),
                "{pattern}/{flags}: {value}"
            );
            assert_eq!(
                rows.last().unwrap()["corpus"]["incomplete"],
                matches.is_none(),
                "{pattern}/{flags}: {value}"
            );
            assert_eq!(
                output.status.code(),
                Some(if matches.is_none() { 2 } else { 1 })
            );
        }
    }
}

/// Stderr text of the matcher-overlap incompleteness diagnostic.
const RUBY_OVERLAP_INCOMPLETE: &str = "static matcher-overlap analysis is incomplete";

/// The summary's `matcherOverlap` census as `(evaluated, skipped)`.
fn ruby_overlap_census(rows: &[Value]) -> (u64, u64) {
    let census = &rows.last().unwrap()["analysis"]["candidateSources"]["matcherOverlap"];
    let count = |field: &str| census[field].as_u64().unwrap();
    (count("evaluated"), count("skipped"))
}

/// Expands a `;`-separated spec of space-separated locations into sorted `ruby_rule_locations`
/// rows; a bare number is a `steps.rb` line.
fn ruby_locations(spec: &str) -> Vec<String> {
    let location = |token: &str| match token.contains(':') {
        true => token.to_owned(),
        false => format!("steps.rb:{token}"),
    };
    let row = |row: &str| row.split(' ').map(location).collect::<Vec<_>>().join(" ");
    let mut rows: Vec<String> = spec
        .split(';')
        .filter(|row| !row.is_empty())
        .map(row)
        .collect();
    rows.sort();
    rows
}

/// One Ruby `ParameterType` declaration line for `name` with the regexp literal body `regexp`.
fn ruby_parameter_type(name: &str, regexp: &str) -> String {
    format!("ParameterType(name: '{name}', regexp: /{regexp}/, transformer: ->(s) {{ s }})\n")
}

/// Declares each `(name, regexp)` type, then registers `value {name}` for each in order.
fn ruby_value_types(types: &[(&str, &str)]) -> String {
    let declarations = types
        .iter()
        .map(|(name, regexp)| ruby_parameter_type(name, regexp));
    let steps = types
        .iter()
        .map(|(name, _)| format!("Given('value {{{name}}}') {{ {name}_action() }}\n"));
    declarations.chain(steps).collect()
}

/// Matcher overlap shares the global candidate limit with handler comparisons, spends it once per
/// unordered pair (a reverse witness is not a second comparison), and charges one index scan per
/// matcher window before any pair is proposed; every truncation is one stderr diagnostic.
#[test]
fn ruby_matcher_overlap_budget_counts_unique_pairs_and_charges_index_scans() {
    let shared = |names: &[&'static str]| {
        ruby_value_types(
            &names
                .iter()
                .map(|name| (*name, "red|green"))
                .collect::<Vec<_>>(),
        )
    };
    let (three, two) = (
        shared(&["first", "second", "third"]),
        shared(&["first", "second"]),
    );
    // Lines 7-8 form one identical-handler comparison, evaluated before overlap, whose literal
    // witnesses accept only their own definition.
    let handlers = format!(
        "{three}Given('alpha step') {{ shared_work(1) }}\nGiven('beta step') {{ shared_work(1) }}\n"
    );
    // `<name>-only` witnesses match only their own definition, so nothing overlaps and only the
    // scan charge can truncate: five windows of one cost 5 scans per witness against a witness
    // scan budget of 4 x limit, while one window costs 1.
    let names = ["first", "second", "third", "fourth", "fifth"];
    let own = names.map(|name| format!("{name}-only"));
    let five = ruby_value_types(
        &names
            .into_iter()
            .zip(own.iter().map(String::as_str))
            .collect::<Vec<_>>(),
    );
    // context, source, window, candidate limit, handler comparisons, (evaluated, skipped),
    // overlap findings
    type Row<'a> = (
        &'a str,
        &'a str,
        Option<usize>,
        usize,
        u64,
        (u64, u64),
        &'a str,
    );
    let rows: [Row<'_>; 8] = [
        ("limit 1 of 3 pairs", &three, None, 1, 0, (1, 1), "4 5"),
        (
            "limit 3 of 3 pairs",
            &three,
            None,
            3,
            0,
            (3, 0),
            "4 5;4 6;5 6",
        ),
        (
            "handler pair spends 1 of limit 3",
            &handlers,
            None,
            3,
            1,
            (2, 1),
            "4 5;4 6",
        ),
        (
            "handler pair within limit 4",
            &handlers,
            None,
            4,
            1,
            (3, 0),
            "4 5;4 6;5 6",
        ),
        (
            "reverse witness reuses the pair",
            &two,
            None,
            1,
            0,
            (1, 0),
            "3 4",
        ),
        ("five windows at limit 1", &five, Some(1), 1, 0, (0, 1), ""),
        ("five windows at limit 2", &five, Some(1), 2, 0, (0, 1), ""),
        ("one window at limit 2", &five, None, 2, 0, (0, 0), ""),
    ];
    for (context, source, window, limit, handlers, census, overlaps) in rows {
        let limit = limit.to_string();
        let arguments = ["--max-candidate-comparisons", &limit];
        let (rows, stderr) = ruby_records_run(&[("steps.rb", source)], window, &arguments);
        assert_eq!(ruby_overlap_census(&rows), census, "{context}");
        let analysis = &rows.last().unwrap()["analysis"];
        assert_eq!(
            analysis["candidateSources"]["identicalHandler"]["evaluated"], handlers,
            "{context}"
        );
        assert_eq!(
            analysis["candidateComparisonsEvaluated"],
            census.0 + handlers,
            "{context}"
        );
        assert_eq!(analysis["truncated"], census.1 > 0, "{context}");
        let found = ruby_rule_locations(&rows, "overlapping-matcher");
        assert_eq!(found, ruby_locations(overlaps), "{context}");
        // Two lines report the absent feature corpus; the rest is the overlap diagnostic.
        let truncated = usize::from(census.1 > 0);
        assert_eq!(stderr.lines().count(), 2 + truncated, "{context}: {stderr}");
        assert_eq!(
            stderr.matches(RUBY_OVERLAP_INCOMPLETE).count(),
            truncated,
            "{context}"
        );
    }
}

/// Overlap findings stop at the 10,000 retained-finding cap and mark the run incomplete; a corpus
/// whose 9,870 pairs fit under the cap reports every pair and stays complete.
#[test]
fn ruby_matcher_overlap_caps_finding_storms_at_the_retained_limit() {
    // 150 mutually overlapping definitions form 11,175 pairs, more than the cap, inside a
    // candidate limit of 30,000; 141 form 9,870.
    for (count, findings, census) in [(150, 10_000, (10_001, 1)), (141, 9_870, (9_870, 0))] {
        let names: Vec<_> = (0..count).map(|index| format!("type_{index}")).collect();
        let types: Vec<_> = names
            .iter()
            .map(|name| (name.as_str(), "red|green"))
            .collect();
        let arguments = ["--max-candidate-comparisons", "30000"];
        let source = ruby_value_types(&types);
        let (rows, stderr) = ruby_records_run(&[("steps.rb", &source)], None, &arguments);
        let overlaps = ruby_rule_locations(&rows, "overlapping-matcher");
        assert_eq!(overlaps.len(), findings, "{count}");
        assert_eq!(rows.len(), findings + 1, "{count}: only overlap findings");
        let first = format!("steps.rb:{} steps.rb:{}", count + 1, count + 2);
        assert!(overlaps.contains(&first), "{count}");
        assert_eq!(ruby_overlap_census(&rows), census, "{count}");
        let truncated = usize::from(census.1 > 0);
        assert_eq!(stderr.matches(RUBY_OVERLAP_INCOMPLETE).count(), truncated);
    }
}

/// A window whose combined matcher program exceeds the 1 MiB regex limit is split into several
/// sets, and each feature step still marks exactly the definition it matches as used; the split is
/// observable as the overlap scan charge of one witness per set.
#[test]
fn ruby_split_matcher_index_keeps_each_step_on_its_own_definition() {
    // `.{10}` compiles to a Unicode-wide program: 200 of them exceed the combined limit, so the
    // default window splits into four sets and definitions 123 and 199 sit in later sets than
    // definition 0. `[a-j]{10}` accepts the same feature steps and fits one set. The two declared
    // `-only` witnesses (lines 203-204) match only their own definition, so nothing overlaps and
    // only the scan charge can truncate: at limit 1 the witness scan budget is 4, which affords
    // both witnesses against one set but one witness against four.
    let used = [0, 123, 199];
    let feature: String = used
        .iter()
        .map(|index| format!("  Given distinct matcher value {index:04} abcdefghij\n"))
        .collect();
    let witnesses = ruby_value_types(&[("first", "first-only"), ("second", "second-only")]);
    let arguments = ["--max-candidate-comparisons", "1"];
    // matcher suffix, (evaluated, skipped) overlap census
    for (suffix, census) in [(".{10}", (0, 1)), ("[a-j]{10}", (0, 0))] {
        let steps = ruby_steps(200, |index| {
            format!("Given(/^distinct matcher value {index:04} {suffix}$/) {{ action_{index}() }}")
        });
        let steps = format!("{steps}{witnesses}");
        let (rows, stderr) =
            ruby_usage_run(&steps, &format!(" Scenario: S\n{feature}"), &arguments);
        let truncated = usize::from(census.1 > 0);
        assert_eq!(stderr.lines().count(), truncated, "{suffix}: {stderr}");
        assert_eq!(
            stderr.matches(RUBY_OVERLAP_INCOMPLETE).count(),
            truncated,
            "{suffix}"
        );
        assert_eq!(ruby_overlap_census(&rows), census, "{suffix}");
        let analysis = &rows.last().unwrap()["analysis"];
        assert_eq!(analysis["truncated"], census.1 > 0, "{suffix}");
        let unused: Vec<String> = (1..=200)
            .filter(|line| !used.contains(&(line - 1)))
            .chain([203, 204])
            .map(|line| line.to_string())
            .collect();
        let unused = ruby_locations(&unused.join(";"));
        assert_eq!(
            ruby_rule_locations(&rows, "unused-definition"),
            unused,
            "{suffix}"
        );
        for rule in ["ambiguous-step", "overlapping-matcher"] {
            assert!(
                ruby_rule_locations(&rows, rule).is_empty(),
                "{suffix}: {rule}"
            );
        }
    }
}

/// Proven ambiguity withholds an overlap only for definitions that share one ambiguity group,
/// including when their memberships are ordered and interleaved; without feature steps every
/// overlapping pair is reported.
#[test]
fn ruby_proven_ambiguity_membership_withholds_only_pairs_sharing_a_group() {
    // Feature steps prove groups {0,2}, {1,3}, {2,4} (lines 6-10). Witnesses propose (0,1), (0,2),
    // (1,2), (1,3), (1,4) and (2,4): (2,4) needs the intersection to step past definition 2's
    // first group, and (1,2) starts with the right side's smaller group.
    let source = ruby_value_types(&[
        ("p0", "w01|g02|w20"),
        ("p1", "w14|w01|g13"),
        ("p2", "w20|g02|g24|w42|w14"),
        ("p3", "g13"),
        ("p4", "w42|w14|g24"),
    ]);
    let groups = "usage.feature:3 6 8;usage.feature:4 7 9;usage.feature:5 8 10";
    let every = "6 7;6 8;7 8;7 9;7 10;8 10";
    for (steps, ambiguous, overlaps) in [
        (
            "value g02\n  Given value g13\n  Given value g24",
            groups,
            "6 7;7 8;7 10",
        ),
        ("value none", "", every),
    ] {
        let feature = format!(" Scenario: S\n  Given {steps}\n");
        let (rows, stderr) = ruby_usage_run(&source, &feature, &[]);
        assert_eq!(stderr, "", "{steps}");
        let found = ruby_rule_locations(&rows, "ambiguous-step");
        assert_eq!(found, ruby_locations(ambiguous), "{steps}");
        let found = ruby_rule_locations(&rows, "overlapping-matcher");
        assert_eq!(found, ruby_locations(overlaps), "{steps}");
    }
}

/// Repeated outline step text gets the full match set at each location, and a path suppression
/// marks both resulting ambiguities suppressed while both definitions stay used.
#[test]
fn ruby_repeated_step_text_and_suppressed_ambiguity_keep_usage() {
    let steps = "Given('same step') { first_action() }\nGiven(/^same step$/) { second_action() }\n";
    let feature = "Feature: F\n Scenario Outline: S\n  Given same step\n  And same step\n  Examples:\n   | value |\n   | one |\n";
    let suppression = r#"{"suppressions":[{"rule":"ambiguous-step","path":"steps.rb","reason":"known overlap"}]}"#;
    for (config, reason) in [(suppression, Some("known overlap")), ("{}", None)] {
        let files = [
            ("steps.rb", steps),
            ("usage.feature", feature),
            (".cuke-dedup.json", config),
        ];
        let (rows, stderr) = ruby_records_run(&files, None, &["--features", "usage.feature"]);
        assert_eq!(stderr, "", "{config}");
        let ambiguous = ruby_locations("usage.feature:3 1 2;usage.feature:4 1 2");
        assert_eq!(
            ruby_rule_locations(&rows, "ambiguous-step"),
            ambiguous,
            "{config}"
        );
        for rule in ["unused-definition", "overlapping-matcher"] {
            assert!(
                ruby_rule_locations(&rows, rule).is_empty(),
                "{config}: {rule}"
            );
        }
        for row in rows.iter().filter(|row| row["rule"] == "ambiguous-step") {
            assert_eq!(row["suppression"]["reason"].as_str(), reason, "{config}");
            assert_eq!(row["active"], reason.is_none(), "{config}");
        }
    }
}

/// `overlapping-matcher` and `unused-definition` set to `off` report nothing and leave the overlap
/// census at zero; an enabled `unused-definition` still skips a definition a step uses.
#[test]
fn ruby_disabled_overlap_and_unused_rules_report_nothing() {
    let steps = "Given('unused') { unused_work() }\nGiven('pick {word}') { pick_one() }\nGiven(/^pick .*$/) { pick_other() }\n";
    let both = [
        "--rule",
        "overlapping-matcher=off",
        "--rule",
        "unused-definition=off",
    ];
    // arguments, feature step, overlap findings, unused findings, overlap census
    type Row<'a> = (&'a [&'a str], &'a str, &'a str, &'a str, (u64, u64));
    let rows: [Row<'_>; 3] = [
        (&both, "nothing here", "", "", (0, 0)),
        (&[], "nothing here", "2 3", "1;2;3", (1, 0)),
        (&both[..2], "unused", "", "2;3", (0, 0)),
    ];
    for (arguments, step, overlaps, unused, census) in rows {
        let feature = format!(" Scenario: S\n  Given {step}\n");
        let (rows, stderr) = ruby_usage_run(steps, &feature, arguments);
        assert_eq!(stderr, "", "{arguments:?}");
        let found = ruby_rule_locations(&rows, "overlapping-matcher");
        assert_eq!(found, ruby_locations(overlaps), "{arguments:?}");
        let found = ruby_rule_locations(&rows, "unused-definition");
        assert_eq!(found, ruby_locations(unused), "{arguments:?}");
        assert_eq!(ruby_overlap_census(&rows), census, "{arguments:?}");
        let findings = rows.iter().filter(|row| row["type"] == "finding").count();
        assert_eq!(findings, rows.len() - 1, "{arguments:?}");
        assert_eq!(
            rows.last().unwrap()["summary"]["findings"],
            findings,
            "{arguments:?}"
        );
    }
}

/// Witnesses come from built-in parameter samples, declared enumerable patterns (Ruby regexp
/// literals and configured `(?ms:` scopes) and resolved Cucumber literal syntax; regex matchers and
/// non-enumerable declared patterns yield none, so the permissive regex reports no overlap for them.
#[test]
fn ruby_overlap_witnesses_cover_builtins_declared_literals_and_unknowns() {
    // `declared regexp;configured pattern;expression;permissive regex;witness`, with `-` for no
    // declaration or configuration and an empty witness for no overlap. The regex-only pair on
    // every row must never overlap: regexes have no witness, and the unanchored `/regex one/` text
    // would match `/^regex .*$/` if a regex were reversed into one.
    let rows = [
        "-;-;I have {int} item(s) in red/blue;^I have .*$;I have 1 items in red",
        r#"-;-;n {float} w {word} s {string};^n .*$;n 1.5 w sample s "sample""#,
        r"-;-;x a\\(b) c/d;^x a.*$;x a(b) c",
        "-;-;open(unclosed;^open.*$;open(unclosed",
        "red|green;-;pick {colour};^pick .*$;pick red",
        "(red|green);-;pick {colour};^pick .*$;pick red",
        "(?:red|green);-;pick {colour};^pick .*$;pick red",
        "((red|green));-;pick {colour};^pick .*$;pick red",
        "(red)|(green);-;pick {colour};^pick .*$;",
        ";-;pick {colour};^pick .*$;",
        "red.*;-;pick {colour};^pick .*$;",
        "-;(?ms:red|green);pick {colour};^pick .*$;pick red",
        "-;(?m:red|green);pick {colour};^pick .*$;pick red",
    ];
    for row in rows {
        let [declared, configured, expression, regex, witness] =
            <[&str; 5]>::try_from(row.split(';').collect::<Vec<_>>()).unwrap();
        let declared = match declared {
            "-" => String::new(),
            pattern => ruby_parameter_type("colour", pattern),
        };
        let config = format!(r#"{{"parameterTypes":{{"colour":"{configured}"}}}}"#);
        let config = if configured == "-" { "{}" } else { &config };
        let source = format!("{declared}Given('{expression}') {{ expression_action() }}\nGiven(/{regex}/) {{ regex_action() }}\nGiven(/regex one/) {{ one_action() }}\nGiven(/^regex .*$/) {{ any_action() }}\n");
        let files = [("steps.rb", source.as_str()), (".cuke-dedup.json", config)];
        let (rows, stderr) = ruby_records_run(&files, None, &["--rule", "unused-definition=off"]);
        assert_eq!(
            rows.last().unwrap()["corpus"]["incomplete"],
            false,
            "{source}: {stderr}"
        );
        let messages: Vec<_> = rows
            .iter()
            .filter(|row| row["rule"] == "overlapping-matcher")
            .map(|row| row["message"].as_str().unwrap())
            .collect();
        let shown = expression.replace(r"\\", r"\");
        let accepts = format!("Matchers `{shown}` and `{regex}` both accept the step `{witness}`");
        let expected: &[&str] = if witness.is_empty() { &[] } else { &[&accepts] };
        assert_eq!(messages, expected, "{source}");
    }
}

/// The custom-parameter fallback keeps optional text, `/` alternatives, signed integers, escaped
/// unclosed parentheses and repeated spaces exact with a declared type, so a step outside that
/// text is unused; an undeclared type makes the fallback inexact, so the definition is never
/// unused.
#[test]
fn ruby_fallback_expressions_keep_optional_alternative_and_declared_text_exact() {
    let colour = ruby_parameter_type("colour", "red|green");
    let custom = ruby_parameter_type("custom", "known");
    let eat = "I eat/eats {int} cucumber(s) with {colour}";
    // declarations, expression, feature step, unused
    let rows: [(&str, &str, &str, bool); 10] = [
        (&colour, eat, "I eat 2 cucumbers with red", false),
        (&colour, eat, "I eats -2 cucumber with green", false),
        (&colour, eat, "I eat 2 cucumbers with blue", true),
        (
            &colour,
            "open(unclosed {colour}",
            "open(unclosed red",
            false,
        ),
        (&colour, "open(unclosed {colour}", "openunclosed red", true),
        (&colour, "two  spaces {colour}", "two  spaces red", false),
        (&colour, "two  spaces {colour}", "two spaces red", true),
        ("", "a {custom}", "b anything", false),
        (&custom, "a {custom}", "b anything", true),
        (&custom, "a {custom}", "a known", false),
    ];
    for (types, expression, step, unused) in rows {
        let steps = format!("{types}Given('{expression}') {{ work() }}\n");
        let feature = format!(" Scenario: S\n  Given {step}\n");
        let (rows, stderr) = ruby_usage_run(&steps, &feature, &[]);
        assert_eq!(stderr, "", "{expression}: {step}");
        let line = types.lines().count() + 1;
        let expected = if unused {
            line.to_string()
        } else {
            String::new()
        };
        let found = ruby_rule_locations(&rows, "unused-definition");
        assert_eq!(found, ruby_locations(&expected), "{expression}: {step}");
    }
}

/// Regex and Cucumber Expression matchers over the 1 MiB pattern limit each report the regex
/// resource limit at their own location; a matcher outside the supported regex subset reports
/// its subset diagnostic, never the limit.
#[test]
fn ruby_matcher_compilation_reports_limits_but_not_unsupported_syntax() {
    let oversized = "x".repeat(1024 * 1024 + 1);
    let source = format!("# limits\nGiven(/{oversized}/) {{ long_regex() }}\nGiven(/^a++$/) {{ possessive() }}\nGiven('{oversized}') {{ long_expression() }}\nGiven('fine') {{ fine() }}\n");
    let arguments = ["--rule", "unused-definition=off"];
    let (_, stderr) = ruby_records_run(&[("steps.rb", &source)], None, &arguments);
    // The resource-limit diagnostic names the absolute temporary path, so each line is pinned by
    // its fixed prefix and its located suffix, joined by the platform path separator.
    let limits: Vec<_> = stderr
        .lines()
        .filter(|line| line.contains("resource limit"))
        .collect();
    assert_eq!(limits.len(), 2, "{stderr}");
    for (limit, line) in limits.iter().zip([2, 4]) {
        let suffix = format!("{}steps.rb:{line}:1 exceeds the 1048576-byte regex resource limit; simplify the matcher or remove its source from definition discovery", std::path::MAIN_SEPARATOR);
        assert!(
            limit.starts_with("cuke-dedup: warning: step matcher at "),
            "{limit}"
        );
        assert!(limit.ends_with(&suffix), "{limit}");
    }
    let subset = "Ruby regular expression is outside the supported static matching subset";
    assert!(
        stderr
            .lines()
            .any(|line| line == format!("cuke-dedup: warning: steps.rb:3:1: {subset}")),
        "{stderr}"
    );
}

/// Matcher windows of one, two and wider than the corpus produce identical records and stderr on
/// every axis: step matches split across windows, a proven ambiguity split across windows that
/// withholds its overlap, overlap without feature usage, an equivalent-matcher group spanning
/// windows, and usage decided across windows.
#[test]
fn ruby_windowing_does_not_change_findings_on_any_axis() {
    let gate = "Given('the gate opens {word}') { gate_word() }\nGiven(/^the gate opens wide$/) { gate_regex() }\nGiven('the gate opens wide') { gate_text() }\n";
    // At window 1 the proven pair sits in two windows and must still withhold the overlap.
    let dial = "Given('the dial reads {int}') { dial_int() }\nGiven(/^the dial reads \\d+$/) { dial_regex() }\n";
    let pump = "Given('the pump runs {word}') { pump_word() }\nGiven(/^the pump runs .+$/) { pump_regex() }\n";
    let lamp = "Given('the lamp glows {word}') { lamp_one() }\nGiven('the lamp glows {word}') { lamp_two() }\nGiven(/^the lamp glows .+$/) { lamp_regex() }\n";
    let parts = "Given('the seal closes') { seal() }\nGiven('the rotor spins') { rotor() }\nGiven('the brake holds') { brake() }\n";
    // steps, feature step, ambiguous-step, overlapping-matcher and unused-definition findings
    let cases = [
        (gate, "the gate opens wide", "usage.feature:3 1 2 3", "", ""),
        (dial, "the dial reads 7", "usage.feature:3 1 2", "", ""),
        (pump, "something unrelated", "", "1 2", "1;2"),
        (lamp, "nothing here", "", "1 3", "1;2;3"),
        (parts, "the rotor spins", "", "", "1;3"),
    ];
    for (steps, step, ambiguous, overlaps, unused) in cases {
        let feature = format!("Feature: F\n Scenario: S\n  Given {step}\n");
        let files = [("steps.rb", steps), ("usage.feature", feature.as_str())];
        let arguments = ["--features", "usage.feature"];
        let windows = [1, 2, steps.lines().count() + 5];
        let runs = windows.map(|window| ruby_records_run(&files, Some(window), &arguments));
        assert_eq!(runs[1], runs[0], "{step}: window 2");
        assert_eq!(runs[2], runs[0], "{step}: one window");
        let rows = &runs[0].0;
        for (rule, expected) in [
            ("ambiguous-step", ambiguous),
            ("overlapping-matcher", overlaps),
            ("unused-definition", unused),
        ] {
            let found = ruby_rule_locations(rows, rule);
            assert_eq!(found, ruby_locations(expected), "{step}: {rule}");
        }
    }
}

#[test]
fn ruby_nested_handler_outcomes_preserve_structure_and_capture_scope() {
    for (left, right, duplicate) in [
        (
            "items.each { |item| store(item) }",
            "items.each { |item| store(item) }",
            true,
        ),
        (
            "items.each { |item| store('first') }",
            "items.each { |item| store('second') }",
            false,
        ),
        (
            "store(\"value #{input}\")",
            "store(\"value #{input}\")",
            true,
        ),
        (
            "store(\"value #{input}\")",
            "store(\"other #{input}\")",
            false,
        ),
        (
            "begin; work(); rescue Error; repair(); ensure; close(); end",
            "begin; work(); rescue Error; repair(); ensure; close(); end",
            true,
        ),
        (
            "begin; work(); rescue Error; repair(); ensure; close(); end",
            "begin; work(); rescue Error; close(); ensure; repair(); end",
            false,
        ),
        (
            "schedule(-> { store('first') })",
            "schedule(-> { store('first') })",
            true,
        ),
        (
            "schedule(-> { store('first') })",
            "schedule(-> { store('second') })",
            false,
        ),
    ] {
        let (_, rows, _) = analyze(
            &format!("Given('first') {{ {left} }}; Then('second') {{ {right} }}"),
            &[],
        );
        let rows = rows.as_array().unwrap();
        assert_eq!(
            rows.iter().any(|row| row["rule"] == "duplicate-handler"),
            duplicate,
            "{left} / {right}"
        );
        assert_eq!(
            rows.last().unwrap()["corpus"]["incomplete"],
            false,
            "{left}"
        );
    }
    for (prefix, body, duplicate) in [
        ("captured = 'value'", "store(captured)", false),
        (
            "captured = 'value'",
            "items.each { |captured| store(captured) }",
            true,
        ),
        (
            "captured = 'value'",
            "items.each { |item; captured| store(captured) }",
            true,
        ),
        (
            "captured = 'value'",
            "items.each { |item| store(captured) }",
            false,
        ),
        ("captured = 'value'", "store(\"value #{captured}\")", false),
        ("captured = 'value'", "store(binding)", false),
        (
            "captured = 'value'",
            "schedule(->(captured) { store(captured) })",
            true,
        ),
        ("", "store(__FILE__)", false),
        ("", "store(__dir__)", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        for (file, matcher) in [("a.rb", "first"), ("b.rb", "second")] {
            fs::write(
                dir.path().join(file),
                format!("{prefix}\nGiven('{matcher}') {{ {body} }}"),
            )
            .unwrap();
        }
        let rows = project_records(dir.path());
        assert_eq!(
            rows.iter().any(|row| row["rule"] == "duplicate-handler"),
            duplicate,
            "cross-file: {body}"
        );
    }
    let (_, rows, _) = analyze("captured = 'value'; Given('first') { store(captured) }; Then('second') { store(captured) }", &[]);
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["rule"] == "duplicate-handler"));
}

#[test]
fn ruby_capture_finding_identity_survives_project_relocation() {
    let source = "captured = 'value'; Given('first') { store(captured) }; Then('second') { store(captured) }";
    let finding = || {
        let (_, rows, _) = analyze(source, &[]);
        rows.as_array()
            .unwrap()
            .iter()
            .find(|row| row["rule"] == "duplicate-handler")
            .unwrap()["fingerprint"]
            .clone()
    };
    assert_eq!(finding(), finding());
}

#[test]
fn ruby_source_line_values_do_not_create_handler_reuse() {
    let (_, rows, _) = analyze(
        "Given('first') { store(__LINE__) }\nThen('second') { store(__LINE__) }",
        &[],
    );
    assert!(!rows
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["rule"] == "duplicate-handler"));
    let (_, rows, _) = analyze(
        "Given('first') { store(__LINE__) }; Then('second') { store(__LINE__) }",
        &[],
    );
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["rule"] == "duplicate-handler"));
}

#[test]
fn ruby_registration_owner_matrix_preserves_findings_without_granting_alias_trust() {
    let dir = tempfile::tempdir().unwrap();
    let definitions = "Given('first') { write(:ready) }; Then('second') { write(:ready) }; Then('conflict') { write(:rejected) }";
    for (owner, exposure, trusted) in [
        (
            "module Helpers; def render; :local; end; alias display render; end",
            "Helpers",
            true,
        ),
        (
            "module Helpers; def render; :local; end; alias :display :render; end",
            "Helpers",
            true,
        ),
        (
            "module Helpers; alias Given render; end",
            "extend Helpers",
            false,
        ),
        (
            "module Helpers; alias display Given; end",
            "extend Helpers",
            false,
        ),
        ("module Helpers; def Given(x); :local; end; end", "", true),
        (
            "module Helpers; def Given(x); :local; end; end",
            "World(Helpers)",
            true,
        ),
        (
            "module Helpers; def render; :local; end; alias display render; end",
            "",
            true,
        ),
        (
            "module Outer; module Helpers; def Given(x); :local; end; end; end",
            "",
            true,
        ),
        (
            "module Helpers; def Given(x); :local; end; end",
            "extend Helpers",
            false,
        ),
        (
            "module Helpers; def Given(x); :local; end; end",
            "include Helpers",
            false,
        ),
        (
            "module Helpers; def Given(x); :local; end; end",
            "Helpers = Cucumber::Glue::Dsl",
            false,
        ),
        (
            "module Helpers; def Given(x); :local; end; end",
            "Alias = Helpers; extend Alias",
            false,
        ),
        (
            "module Helpers; def Given(x); :local; end; register(self); end",
            "",
            false,
        ),
        (
            "module Cucumber; module Glue; module Dsl; def Given(x); end; end; end; end",
            "",
            false,
        ),
        (
            "module Helpers; def Given(x); :local; end; end",
            "def World(x); extend x; end; World(Helpers)",
            false,
        ),
        (
            "module Helpers; def Given(x); :local; end; end",
            "unknown.send(:define_method, :Given) {}",
            false,
        ),
        (
            "module Helpers; def Given(x); end; end",
            "extend provider",
            false,
        ),
        (
            "module Helpers; def Given(x); end; end",
            "extend Object.const_get(:Helpers)",
            false,
        ),
        (
            "module Helpers; def Given(x); end; end",
            "Object.send(:include, provider)",
            false,
        ),
        ("module Helpers; eval(code); end", "", false),
        ("module Helpers; send(name); end", "", false),
        ("def Given(x); :local; end", "", false),
        ("class Object; def Given(x); :local; end; end", "", false),
        (
            "module Helpers; def Given(x); :local; end; end",
            "module Helpers; expose(self); end",
            false,
        ),
    ] {
        for (owner_file, exposure_file) in [("a.rb", "z.rb"), ("z.rb", "a.rb")] {
            fs::write(dir.path().join(owner_file), owner).unwrap();
            fs::write(dir.path().join(exposure_file), exposure).unwrap();
            fs::write(dir.path().join("steps.rb"), definitions).unwrap();
            let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
            let code = output.status.code().unwrap();
            let rows = records(output.stdout);
            let summary = rows.last().unwrap();
            assert_eq!(
                summary["summary"]["definitionsAnalyzed"],
                if trusted { 3 } else { 0 },
                "{owner} / {exposure}: {rows:?}"
            );
            assert_eq!(
                summary["corpus"]["incomplete"], !trusted,
                "{owner} / {exposure}"
            );
            assert_eq!(code, if trusted { 1 } else { 2 }, "{owner} / {exposure}");
            let findings: Vec<_> = rows
                .iter()
                .filter(|r| r["rule"] == "duplicate-handler")
                .collect();
            assert_eq!(
                findings.len(),
                usize::from(trusted),
                "{owner} / {exposure}: {findings:?}"
            );
            if trusted {
                assert!(!serde_json::to_string(&findings)
                    .unwrap()
                    .contains("conflict"));
            }
        }
    }
}

#[test]
fn ruby_namespace_identity_and_loader_effects_fail_closed() {
    let definitions = "Given('first') { write(:ready) }; Given('second') { write(:ready) }; Given('conflict') { write(:rejected) }";
    for (source, trusted, complete) in [
        ("module Helpers; def Given(x); :local; end; end", true, true),
        ("module Outer; module Helpers; def Given(x); :local; end; end; end", true, true),
        ("module Outer; end; module Outer::Helpers; def Given(x); :local; end; end", true, true),
        ("module Outer; end; module ::Outer::Helpers; def Given(x); :local; end; end", true, true),
        ("module First; module Helpers; def Given(x); :local; end; end; end; module Second; module Helpers; end; end; observe(Second::Helpers)", true, true),
        ("module Helpers; def Given(x); :local; end; end; World(Helpers)", true, true),
        ("module Helpers; def render; :local; end; alias display render; end; observe(Helpers)", true, true),
        ("# autoload :Loader, 'loader'\nmessage = 'autoload Loader'", true, true),
        ("def Object.const_missing(name); Cucumber::Glue; end; module Ghost::Dsl; def Given(x); :local; end; end", false, false),
        ("module Ghost::Dsl; def Given(x); :local; end; end", false, false),
        ("module Ghost::Helpers; end", false, false),
        ("module A; module B; end; end; module Outer; module A; def self.const_missing(name); Cucumber::Glue; end; end; module A::B::Dsl; def Given(x); :local; end; end; end", false, false),
        ("module Ghost::Dsl; def Given(x); :local; end; end; module Ghost; end", false, false),
        ("module provider::Dsl; def Given(x); :local; end; end", false, false),
        ("module Outer; end; Outer = Cucumber::Glue; module Outer::Dsl; def Given(x); :local; end; end", false, false),
        ("Alias = Cucumber::Glue; module Alias::Dsl; def Given(x); :local; end; end", false, false),
        ("module Outer; module Helpers; def Given(x); :local; end; end; end; observe(Outer)", false, false),
        ("autoload :Loader, './providers/loader'; Loader", false, false),
        ("Object.autoload :Loader, './providers/loader'; Loader", false, false),
        ("autoload loader_name, loader_path", false, false),
        ("loader = Kernel; loader.load('./providers/loader.rb')", true, false),
        ("YAML = Kernel; YAML.load('./providers/loader.rb')", true, false),
        ("loader = Kernel; loader.autoload(:Loader, './providers/loader')", false, false),
        ("send(:autoload, :Loader, './providers/loader')", false, false),
        ("Object.public_send('autoload', :Loader, './providers/loader')", false, false),
        ("alias lazy_load autoload; lazy_load :Loader, './providers/loader'", false, false),
        ("def install; autoload :Loader, './providers/loader'; end; install", false, false),
        ("-> { autoload :Loader, './providers/loader' }.call", false, false),
        ("Given('deferred') { autoload :Loader, './providers/loader' }", true, false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("providers")).unwrap();
        fs::write(dir.path().join("providers/loader.rb"), "File.write('executed', 'bad'); Loader = true; def Given(*args); :local; end").unwrap();
        fs::write(dir.path().join("steps.rb"), format!("{source}\n{definitions}")).unwrap();
        let output = run_project(dir.path(), "steps.rb", &["--fail-on-incomplete"]);
        assert_eq!(output.status.code(), Some(if complete { 1 } else { 2 }), "{source}");
        let rows = records(output.stdout);
        let expected = if !trusted { 0 } else if source.contains("Given('deferred')") { 4 } else { 3 };
        assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], expected, "{source}: {rows:?}");
        assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], !complete, "{source}");
        let findings: Vec<_> = rows.iter().filter(|row| row["rule"] == "duplicate-handler").collect();
        assert_eq!(findings.len(), usize::from(trusted), "{source}: {rows:?}");
        assert!(!serde_json::to_string(&findings).unwrap().contains("conflict"), "{source}");
        assert!(!dir.path().join("executed").exists());
    }
}

#[test]
fn ruby_namespace_effects_are_independent_of_extraction_order() {
    for (owner, effect, trusted) in [
        (
            "module Outer; module Helpers; def Given(x); :local; end; end; end",
            "Outer = Cucumber::Glue",
            false,
        ),
        (
            "module Outer; module Helpers; def Given(x); :local; end; end; end",
            "module Other; module Helpers; end; end; observe(Other::Helpers)",
            true,
        ),
        (
            "module Ghost::Dsl; def Given(x); :local; end; end",
            "def Object.const_missing(name); Cucumber::Glue; end",
            false,
        ),
        (
            "module Ghost::Dsl; def Given(x); :local; end; end",
            "module Ghost; end",
            false,
        ),
        ("", "autoload :Loader, 'provider'", false),
    ] {
        for (left, right) in [("a.rb", "z.rb"), ("z.rb", "a.rb")] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(left), owner).unwrap();
            fs::write(dir.path().join(right), effect).unwrap();
            fs::write(dir.path().join("steps.rb"), "Given('first') { work(:same) }; Given('second') { work(:same) }; Given('conflict') { work(:different) }").unwrap();
            let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
            assert_eq!(
                output.status.code(),
                Some(if trusted { 1 } else { 2 }),
                "{owner} / {effect}"
            );
            let rows = records(output.stdout);
            assert_eq!(
                rows.last().unwrap()["summary"]["definitionsAnalyzed"],
                if trusted { 3 } else { 0 },
                "{owner} / {effect}: {rows:?}"
            );
            assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], !trusted);
            let findings: Vec<_> = rows
                .iter()
                .filter(|row| row["rule"] == "duplicate-handler")
                .collect();
            assert_eq!(findings.len(), usize::from(trusted));
            assert!(!serde_json::to_string(&findings)
                .unwrap()
                .contains("conflict"));
        }
    }
}

#[test]
fn ruby_unsupported_self_registrations_report_incomplete_discovery() {
    for receiver in [
        "self",
        "(self)",
        "((self))",
        "(# comment\n self)",
        "(work(); self)",
        "object",
    ] {
        for keyword in ["Given", "When", "Then", "And", "But"] {
            for operator in [".", "&."] {
                let source = format!("Given('first') {{ work(:same) }}; Given('second') {{ work(:same) }}; {receiver}{operator}{keyword}('hidden') {{ other() }}");
                let (code, rows, _) = analyze(&source, &["--fail-on-incomplete"]);
                let rows = rows.as_array().unwrap();
                let incomplete = receiver != "object";
                assert_eq!(code, if incomplete { 2 } else { 1 }, "{source}: {rows:?}");
                assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
                assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], incomplete);
                assert_eq!(
                    rows.iter()
                        .filter(|row| row["rule"] == "duplicate-handler")
                        .count(),
                    1
                );
            }
        }
    }
}

#[test]
fn ruby_near_wording_requires_exact_execution_and_real_matcher_similarity() {
    for (left, right, left_body, right_body, near) in [
        ("abcdefghijkl", "abcdefghijxx", "verify()", "verify()", true),
        (
            "abcdefghijkl",
            "abcdefghixxx",
            "verify()",
            "verify()",
            false,
        ),
        (
            "abcdefghijklm",
            "abcdefghijxxx",
            "verify()",
            "verify()",
            true,
        ),
        (
            "abcdefghijklm",
            "abcdefghixxxx",
            "verify()",
            "verify()",
            false,
        ),
        (
            "the parcel is ready",
            "the parcel is now ready",
            "verify(:ready)",
            "verify(:ready)",
            true,
        ),
        (
            "the account is enabled",
            "the account is not enabled",
            "verify(:ready)",
            "verify(:ready)",
            false,
        ),
        ("a", "b", "verify(:ready)", "verify(:ready)", false),
        (
            "the parcel is ready",
            "the parcel is now ready",
            "verify(:ready)",
            "verify(:rejected)",
            false,
        ),
        (
            "the parcel is ready",
            "the parcel is now ready",
            "expect(parcel).to eq(:ready)",
            "expect(parcel).not_to eq(:ready)",
            false,
        ),
        (
            "the parcel is ready",
            "the parcel is now ready",
            "wrap { expect(parcel).to eq(:ready) }",
            "wrap { expect(parcel).not_to eq(:ready) }",
            false,
        ),
        (
            "the parcel is ready",
            "the parcel is now ready",
            "first.verify(:ready)",
            "other.verify(:ready)",
            false,
        ),
    ] {
        let source =
            format!("Given({left:?}) {{ {left_body} }}; Then({right:?}) {{ {right_body} }}");
        let (_, rows, _) = analyze(&source, &[]);
        let rows = rows.as_array().unwrap();
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "near-duplicate-step"),
            near,
            "{source}: {rows:?}"
        );
        assert!(!rows
            .iter()
            .any(|r| r["rule"] == "parameterization-candidate"));
    }
    let (_, rows, _) = analyze(
        "Given(/parcel ready/) { verify() }; Then(/parcel now ready/m) { verify() }",
        &[],
    );
    assert!(!rows
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["rule"] == "near-duplicate-step"));
}

#[test]
fn ruby_near_exact_respects_handler_threshold_and_capture_context() {
    let dir = tempfile::tempdir().unwrap();
    for threshold in [0.5, 1.0] {
        fs::write(
            dir.path().join(".cuke-dedup.json"),
            format!(r#"{{"nearDuplicateHandlerSimilarity":{threshold}}}"#),
        )
        .unwrap();
        for (capture, expected) in [("", true), ("captured = :value", false)] {
            for (file, matcher) in [
                ("a.rb", "the parcel is ready"),
                ("b.rb", "the parcel is now ready"),
            ] {
                fs::write(
                    dir.path().join(file),
                    format!("{capture}\nGiven('{matcher}') {{ store(captured) }}"),
                )
                .unwrap();
            }
            let rows = project_records(dir.path());
            assert_eq!(
                rows.iter().any(|r| r["rule"] == "near-duplicate-step"),
                expected,
                "threshold={threshold}, capture={capture}"
            );
        }
    }
}

/// Runs the binary over `files` written to a fresh project with the JSON reporter, returning the
/// exit code, the parsed report and stderr; a run that writes no report fails with both.
fn ruby_pair_report(files: &[(&str, &str)], pattern: &str, extra: &[&str]) -> (i32, Value, String) {
    let directory = ruby_project(files);
    let output = ruby_cli(directory.path(), pattern)
        .args(["--reporters", "json", "--output", "out", "--no-metrics"])
        .args(extra)
        .output()
        .unwrap();
    let code = output.status.code().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let path = directory.path().join("out/cuke-dedup.json");
    assert!(path.is_file(), "exit {code}, no report: {stderr}");
    (code, ruby_report(&path), stderr)
}

/// The `rule` findings of a JSON report as `(primary line, first related line, evidence)`.
fn ruby_pair_findings<'a>(report: &'a Value, rule: &str) -> Vec<(u64, u64, &'a Value)> {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["rule"] == rule)
        .map(|finding| {
            let line = |location: &Value| location["line"].as_u64().unwrap();
            (
                line(&finding["primary"]),
                line(&finding["related"][0]),
                &finding["evidence"],
            )
        })
        .collect()
}

/// Matcher blocking proposes a near-worded Ruby pair only when the action streams overlap; the
/// configured floor (default 0.70) decides the finding, and the same bodies under one matcher
/// expose the handler similarity through duplicate-matcher evidence.
#[test]
fn ruby_matcher_blocking_admits_only_handlers_that_share_actions() {
    // `left | right | near at a 0.5 floor | handler similarity`; a pair is a matcher-blocking
    // candidate exactly when it is near here, and near at the default floor from 0.70 up. The control-flow row needs differing streams
    // (two `if` events against one) with no call: identical streams are vetoed for another reason.
    let login = "page.goto('/login'); page.fill('#email', 'user@example.test')";
    let rows = [
        format!("{login} | {login}; page.wait_for_load_state | true | 0.667"),
        format!("{login} | unrelated_audit; unrelated_notification | false | 0.0"),
        "page.goto('/login') | page.goto('/login'); page.wait_for_load_state | true | 0.5".into(),
        "page.click('#save') | page.locator('#save').click | true | 0.5".into(),
        "load_account | write_audit | false | 0.0".into(),
        "page.click('#save') | audit.click('#save') | false | 0.0".into(),
        "ready = true; if ready then :a end; if ready then :b end | active = true; if active then :c end | false | 0.5".into(),
        "page.goto('/'); page.fill('#x', 'x'); page.click('#save') | page.goto('/'); page.reload; page.screenshot | false | 0.333".into(),
        "page.click('#save') | page.fill('#save', 'value') | false | 0.0".into(),
        format!("{login}; page.click('#go') | {login}; page.click('#go'); page.wait_for_load_state | true | 0.75"),
    ];
    for row in rows {
        let [left, right, near, similarity] = row.split(" | ").collect::<Vec<_>>()[..] else {
            panic!("{row}")
        };
        let near: bool = near.parse().unwrap();
        let similarity = Some(similarity.parse::<f64>().unwrap());
        let pair = |first: &str, second: &str| {
            format!("Given({first:?}) {{ {left} }}\nGiven({second:?}) {{ {right} }}\n")
        };
        let (_, report, _) = ruby_pair_report(&[("steps.rb", &pair("same", "same"))], "*.rb", &[]);
        let duplicates = ruby_pair_findings(&report, "duplicate-matcher");
        assert_eq!(duplicates.len(), 1, "{row}");
        assert_eq!(
            duplicates[0].2["handlerSimilarity"].as_f64(),
            similarity,
            "{row}"
        );
        let near_pair = pair("I am on login page", "I am on the login page");
        for (config, expected) in [
            (r#"{"nearDuplicateHandlerSimilarity":0.5}"#, near),
            ("{}", near && similarity >= Some(0.7)),
        ] {
            let files = [("steps.rb", &near_pair[..]), (".cuke-dedup.json", config)];
            let (_, report, _) = ruby_pair_report(&files, "*.rb", &[]);
            let blocked = &report["analysis"]["candidateSources"]["matcherBlocking"]["evaluated"];
            assert_eq!(blocked, u64::from(near), "{config}: {row}");
            let found: Vec<_> = ruby_pair_findings(&report, "near-duplicate-step")
                .into_iter()
                .map(|(line, related, evidence)| {
                    (line, related, evidence["handlerSimilarity"].as_f64())
                })
                .collect();
            let wanted = expected
                .then_some((1, 2, similarity))
                .into_iter()
                .collect::<Vec<_>>();
            assert_eq!(found, wanted, "{config}: {row}");
        }
    }
}

/// An identical-handler spanning tree does not hide the near-worded pair it leaves out, also when
/// the handlers record no actions at all.
#[test]
fn ruby_matcher_blocking_reaches_near_pairs_beside_identical_handlers() {
    // `stored = value` records no action event, so only identical-handler structure admits it.
    for body in ["shared_implementation()", "|value| stored = value"] {
        let source = format!(
            "Given('completely unrelated setup') {{ {body} }}\nGiven('the account is enabled') {{ {body} }}\nGiven('the accounts are enabled') {{ {body} }}\n"
        );
        let (_, report, _) = ruby_pair_report(&[("steps.rb", &source)], "*.rb", &[]);
        let sources = &report["analysis"]["candidateSources"];
        assert_eq!(sources["identicalHandler"]["evaluated"], 2, "{body}");
        assert_eq!(sources["matcherBlocking"]["evaluated"], 1, "{body}");
        let near = ruby_pair_findings(&report, "near-duplicate-step");
        assert_eq!(near.len(), 1, "{body}");
        let comparison = &near[0].2["comparison"];
        assert_eq!(
            [&comparison["leftMatcher"], &comparison["rightMatcher"]],
            ["the account is enabled", "the accounts are enabled"],
            "{body}"
        );
    }
}

/// Matcher blocking stops at the global candidate limit, counts the proposal it could not keep and
/// marks the analysis truncated; the unlimited run is the control.
#[test]
fn ruby_matcher_blocking_respects_the_global_candidate_limit() {
    // Distinct handlers sharing two of three actions: every pair passes the blocking gate, and no
    // pair has identical streams, which Ruby never proposes as near material.
    let source: String = ["one", "two", "three", "four"]
        .iter()
        .map(|name| {
            format!("Given('account operation {name}') {{ page.open; page.fill; {name}_step }}\n")
        })
        .collect();
    for (extra, evaluated, skipped, truncated) in [
        (&["--max-candidate-comparisons", "2"][..], 2, 1, true),
        (&[][..], 6, 0, false),
    ] {
        let (code, report, stderr) = ruby_pair_report(&[("steps.rb", &source)], "*.rb", extra);
        assert_eq!(code, 0, "{stderr}");
        ruby_assert_pointers(
            &report["analysis"],
            &[
                ("/truncated", truncated.into()),
                ("/candidateComparisonsEvaluated", evaluated.into()),
                (
                    "/candidateSources/matcherBlocking/evaluated",
                    evaluated.into(),
                ),
                ("/candidateSources/matcherBlocking/skipped", skipped.into()),
            ],
        );
        assert_eq!(
            stderr.contains("analysis is incomplete: evaluated 2 candidate definition comparisons and skipped 1"),
            truncated,
            "{stderr}"
        );
    }
}

/// A parameterization candidate needs one bounded, value-like matcher change without a polarity
/// conflict, however much context the matchers share; it never becomes a near finding.
#[test]
fn ruby_parameterization_requires_one_bounded_value_change() {
    for (left, right, expected) in [
        (
            "the save button is shown",
            "the cancel button is shown",
            true,
        ),
        (
            "the AI chatbot toggle button should be visible",
            "the AI chatbot window should be open",
            false,
        ),
        (
            "the panel should be shown",
            "the panel should not be shown",
            false,
        ),
        (
            "the client should connect",
            "the client should disconnect",
            false,
        ),
        ("the value is valid", "the value is invalid", false),
        (
            "the argument is logical",
            "the argument is illogical",
            false,
        ),
        ("the action is possible", "the action is impossible", false),
        ("the layout is regular", "the layout is irregular", false),
        ("the red button shown", "the blue button hidden", false),
        (
            "the New York office is open",
            "the York City office is open",
            true,
        ),
        (
            "I click the save button on the checkout summary page",
            "I click the cancel button on the checkout summary page",
            true,
        ),
        (
            "I select the first row of the table",
            "I select the last row of the table",
            true,
        ),
        (
            "the advanced reporting feature is enabled for this account",
            "the advanced reporting feature is disabled for this account",
            false,
        ),
        (
            "the advanced reporting feature uses dark theme for this account",
            "the advanced reporting feature uses light theme for this account",
            true,
        ),
    ] {
        // Literal-only handler difference: the handlers share one structural class, so only the
        // matcher shape decides.
        let source =
            format!("Then({left:?}) {{ page.choose('left') }}\nThen({right:?}) {{ page.choose('right') }}\n");
        let (_, report, _) = ruby_pair_report(&[("steps.rb", &source)], "*.rb", &[]);
        let rows = report["findings"].as_array().unwrap();
        assert_parameterization_outcome(rows, expected, &source);
    }
}

/// Near wording needs matchers of one syntax without opposite words; the same handlers under a
/// compatible pair are the control.
#[test]
fn ruby_near_wording_rejects_mixed_syntax_and_opposite_words() {
    for (left, right, near) in [
        (
            "'the account is active'",
            "'the account is inactive'",
            false,
        ),
        ("'the user can log in'", "'the user cannot log in'", false),
        (
            "'the account is active'",
            "'the account is now active'",
            true,
        ),
        ("'account is active'", "/account is active/", false),
        ("'account is active'", "/account is now active/", false),
        ("/account is active/", "/account is now active/", true),
    ] {
        let source = format!(
            "Given({left}) {{ page.check(account) }}\nGiven({right}) {{ page.check(account) }}\n"
        );
        let (_, report, _) = ruby_pair_report(&[("steps.rb", &source)], "*.rb", &[]);
        let found = ruby_pair_findings(&report, "near-duplicate-step");
        assert_eq!(found.len(), usize::from(near), "{source}");
        assert_eq!(
            ruby_pair_findings(&report, "duplicate-handler").len(),
            1,
            "{source}"
        );
    }
}

/// Empty handlers and matchers with disjoint placeholder types complete without handler or
/// matcher findings; non-empty handlers and a whitespace-only matcher change are the controls.
#[test]
fn ruby_empty_handlers_and_disjoint_placeholder_types_do_not_fail_analysis() {
    for (body, extra_matcher, rules) in [
        ("", "another empty step", &[][..]),
        ("work()", "another empty step", &["duplicate-handler"][..]),
        ("", "an  integer {int}", &["normalized-matcher"][..]),
    ] {
        let source = format!(
            "Given('an integer {{int}}') {{ {body} }}\nGiven('a string {{string}}') {{ {body} }}\nGiven('{extra_matcher}') {{ }}\n"
        );
        let (code, report, stderr) = ruby_pair_report(&[("steps.rb", &source)], "*.rb", &[]);
        assert_eq!(code, i32::from(!rules.is_empty()), "{source}: {stderr}");
        ruby_assert_pointers(
            &report,
            &[
                ("/summary/definitionsAnalyzed", 3.into()),
                ("/corpus/incomplete", false.into()),
            ],
        );
        let found: Vec<_> = report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|finding| finding["rule"].as_str().unwrap())
            .collect();
        assert_eq!(found, rules, "{source}");
    }
}

/// Configured assertions that differ only in their last values keep ordered overlap: ten
/// statements with one or three changed values stay near at the default 0.70 floor, four do not.
#[test]
fn ruby_mostly_matching_ordered_assertions_remain_near_duplicates() {
    let assertions = |changed: usize, value: &str| {
        (0..10)
            .map(|index| {
                let expected = if index >= 10 - changed {
                    value
                } else {
                    "shared"
                };
                format!("Assertions.expect(state.field{index}).to_be('{expected}'); ")
            })
            .collect::<String>()
    };
    for (changed, similarity) in [(1, Some(0.9)), (3, Some(0.7)), (4, None)] {
        let source = format!(
            "require_relative 'assertions'\nThen('the account workflow is ready') {{ {} }}\nThen('the account workflow is nearly ready') {{ {} }}\n",
            assertions(changed, "first"),
            assertions(changed, "second")
        );
        let files = [
            ("assertions.rb", ASSERTION_PROVIDER),
            ("steps.rb", &source[..]),
            (
                ".cuke-dedup.json",
                r#"{"assertionModules":["./assertions"]}"#,
            ),
        ];
        let (_, report, _) = ruby_pair_report(&files, "steps.rb", &[]);
        assert_eq!(report["corpus"]["incomplete"], false, "{changed}");
        let near = ruby_pair_findings(&report, "near-duplicate-step");
        let found = near
            .first()
            .map(|(_, _, evidence)| evidence["handlerSimilarity"].as_f64().unwrap());
        assert_eq!(found, similarity, "{changed}");
    }
}

/// Pair evidence over Ruby definitions: each handler-similarity path with its classification, the
/// rounded ordered overlap, and a source-bearing, Unicode-safe matcher delta with fingerprints.
#[test]
fn ruby_pair_evidence_reports_similarity_paths_and_matcher_delta() {
    let plain = "\
Given('exact') { work() }
Given('exact') { work() }
Given('alpha') { x = page.open; x.go }
Given('alpha') { y = page.open; y.go }
Given('structure') { page.open('/a') }
Given('structure') { page.open('/b') }
Given('overlap') { page.goto('/') }
Given('overlap') { page.goto('/'); page.wait_for_load_state }
Given('ordered') { page.open; page.fill; page.save }
Given('ordered') { page.open; page.save; page.notify }
Given('renamed') { |value| stored = value }
Given('renamed') { |other| stored = other }
Given('silent') { value = :a }
Given('silent') { value = :b; other = :c }
Given('the item is visible') { page.check(item) }
Given('the items are visible') { page.check(item) }
Given('the café is open') { page.check(cafe) }
Given('the cafè is open') { page.check(cafe) }
";
    // An untrusted `expect` chain withdraws the action stream, so `partial` cannot measure the
    // overlap that `full` measures; `theme` shares one structure with conflicting assertion values.
    let trusted = "require_relative 'assertions'
Given('theme') { Assertions.expect(page).to_be('dark') }
Given('theme') { Assertions.expect(page).to_be('light') }
Given('partial') { Assertions.expect(page).to_be('dark'); expect(page).to be_ready; page.goto('/') }
Given('partial') { Assertions.expect(page).to_be('dark'); expect(page).to be_ready; page.goto('/'); page.wait_for_load_state }
Given('full') { Assertions.expect(page).to_be('dark'); page.goto('/') }
Given('full') { Assertions.expect(page).to_be('dark'); page.goto('/'); page.wait_for_load_state }
";
    let (_, report, _) = ruby_pair_report(&[("steps.rb", plain)], "*.rb", &[]);
    let files = [
        ("assertions.rb", ASSERTION_PROVIDER),
        ("steps.rb", trusted),
        (
            ".cuke-dedup.json",
            r#"{"assertionModules":["./assertions"]}"#,
        ),
    ];
    let (_, trusted_report, _) = ruby_pair_report(&files, "steps.rb", &[]);
    let scores = |report: &Value| -> Vec<String> {
        ruby_pair_findings(report, "duplicate-matcher")
            .into_iter()
            .map(|(line, related, evidence)| {
                let field = |name: &str| evidence[name].to_string();
                let fields =
                    ["matcherSimilarity", "handlerSimilarity", "handlerEvidence"].map(field);
                format!("{line}:{related} {}", fields.join(" "))
            })
            .collect()
    };
    assert_eq!(
        scores(&report),
        [
            r#"1:2 1.0 1.0 "Handlers have the same exact syntax fingerprint""#,
            r#"3:4 1.0 1.0 "Handlers differ only in parameter or local-variable names""#,
            r#"5:6 1.0 0.95 "Handlers share the same structure after literal normalization""#,
            r#"7:8 1.0 0.5 "Handler behavior signatures differ""#,
            r#"9:10 1.0 0.667 "Handler behavior signatures differ""#,
            r#"11:12 1.0 1.0 "Handlers differ only in parameter or local-variable names""#,
            r#"13:14 1.0 0.0 "Handler behavior signatures differ""#,
        ]
    );
    assert_eq!(
        scores(&trusted_report),
        [
            r#"2:3 1.0 0.0 "Handlers share the same structure after literal normalization""#,
            r#"4:5 1.0 0.0 "Handler behavior signatures differ""#,
            r#"6:7 1.0 0.667 "Handler behavior signatures differ""#,
        ]
    );
    // Exact-equal handlers whose events conflict: only `a.rb` loads the provider, so trust splits.
    let step = "Given('same') { Assertions.expect(page).to_be('dark') }\n";
    let loaded = format!("require_relative 'assertions'\n{step}");
    for (other, similarity) in [(step, 0.0), (&loaded[..], 1.0)] {
        let split = [files[0], files[2], ("a.rb", &loaded[..]), ("b.rb", other)];
        let (_, split_report, _) = ruby_pair_report(&split, "[ab].rb", &[]);
        let evidence = ruby_pair_findings(&split_report, "duplicate-matcher")[0].2;
        let exact = "Handlers have the same exact syntax fingerprint";
        assert_eq!(evidence["handlerSimilarity"], similarity, "{other}");
        assert_eq!(evidence["handlerEvidence"], exact, "{other}");
    }
    // Differing matcher kinds skip the normalized shortcut; two empty matchers still score 1.0.
    let empty = [("steps.rb", "Given('') { work() }\nGiven(//) { work() }\n")];
    let (_, empty_report, _) = ruby_pair_report(&empty, "*.rb", &[]);
    let duplicate = ruby_pair_findings(&empty_report, "duplicate-handler")[0].2;
    assert_eq!(duplicate["matcherSimilarity"], 1.0);
    let near = ruby_pair_findings(&report, "near-duplicate-step");
    let deltas: Vec<_> = near
        .iter()
        .map(|(line, related, evidence)| {
            let comparison = &evidence["comparison"];
            assert!(comparison["leftHandler"]
                .as_str()
                .unwrap()
                .contains("page.check("));
            let fingerprints = ["leftFingerprint", "rightFingerprint"]
                .map(|side| comparison[side].as_str().unwrap());
            assert!(fingerprints
                .iter()
                .all(|fingerprint| fingerprint.len() == 32
                    && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())));
            assert_ne!(fingerprints[0], fingerprints[1]);
            let diff = &comparison["matcherDiff"];
            let parts = ["prefix", "leftChange", "rightChange", "suffix"]
                .map(|part| diff[part].as_str().unwrap());
            format!("{line}:{related} {}", parts.join("|"))
        })
        .collect();
    assert_eq!(
        deltas,
        [
            "15:16 the item| is|s are| visible",
            "17:18 the caf|é|è| is open"
        ]
    );
    assert_eq!(
        near[0].2["matcherDifference"],
        "`the item is visible` ↔ `the items are visible`"
    );
}

/// Duplicate-matcher findings over three same-matcher Ruby definitions keep one component of all
/// three handlers in two findings, whatever the definitions' source order.
#[test]
fn ruby_duplicate_matcher_components_are_invariant_to_definition_order() {
    let handlers = ["first_step", "second_step", "third_step"];
    for permutation in [[0, 1, 2], [2, 1, 0], [0, 2, 1], [1, 0, 2]] {
        let source: String = permutation
            .iter()
            .map(|&index| format!("Given('same') {{ {}() }}\n", handlers[index]))
            .collect();
        let (_, report, _) = ruby_pair_report(&[("steps.rb", &source)], "*.rb", &[]);
        let findings = ruby_pair_findings(&report, "duplicate-matcher");
        let mut members: Vec<_> = findings
            .iter()
            .flat_map(|(_, _, evidence)| {
                ["leftHandler", "rightHandler"]
                    .map(|side| evidence["comparison"][side].as_str().unwrap())
            })
            .collect();
        members.sort_unstable();
        members.dedup();
        assert_eq!(
            members,
            ["{ first_step() }", "{ second_step() }", "{ third_step() }"],
            "{permutation:?}"
        );
        assert_eq!(findings.len(), 2, "{permutation:?}");
    }
}

#[test]
fn ruby_parameter_types_are_static_and_suite_scoped() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("steps.rb"), "Given('channel {channel}') { first() }; Then('channel {channel}') { second() }; Then('unrelated {channel}') { other() }").unwrap();
    fs::write(
        dir.path().join("suite.feature"),
        "Feature: channels\n Scenario: output\n  Given channel stdout\n",
    )
    .unwrap();
    for (pattern, expected_match, complete) in [
        ("/stdout|stderr/", true, true),
        ("%r{stdout|stderr}", true, true),
        ("'stdout|stderr'", true, true),
        ("[/stderr/, 'stdout']", true, true),
        ("/stderr/", false, true),
        ("/stdout/m", false, false),
        ("/stdout/i", false, false),
        ("/stdout/x", false, false),
        ("pattern", false, false),
        ("\"#{pattern}\"", false, false),
    ] {
        for file in ["a.rb", "z.rb"] {
            fs::write(dir.path().join(file), format!("ParameterType(name: 'channel', regexp: {pattern}, transformer: ->(value) {{ File.write('executed', value); value }})")).unwrap();
            let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
            let code = output.status.code();
            let rows = records(output.stdout);
            let has = |rule| rows.iter().any(|r| r["rule"] == rule);
            assert_eq!(
                rows.last().unwrap()["corpus"]["incomplete"],
                !complete,
                "{pattern}: {rows:?}"
            );
            assert!(has("duplicate-matcher"), "{pattern}");
            assert_eq!(has("ambiguous-step"), expected_match, "{pattern}: {rows:?}");
            assert_eq!(code, Some(if complete { 1 } else { 2 }), "{pattern}");
            assert!(!dir.path().join("executed").exists());
            fs::remove_file(dir.path().join(file)).unwrap();
        }
    }
}

#[test]
fn ruby_parameter_type_uncertainty_never_preserves_manual_pattern_trust() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join(".cuke-dedup.json"),
        r#"{"parameterTypes":{"channel":"stdout"}}"#,
    )
    .unwrap();
    fs::write(
        dir.path().join("steps.rb"),
        "Given('channel {channel}') { first() }; Then('channel {channel}') { second() }",
    )
    .unwrap();
    fs::write(
        dir.path().join("suite.feature"),
        "Feature: channels\n Scenario: output\n  Given channel stdout\n",
    )
    .unwrap();
    for (declaration, duplicate, definitions) in [
        ("ParameterType(name: 'channel', regexp: pattern, transformer: ->(v) { v })", false, 2),
        ("ParameterType(name: 'channel', regexp: /stdout/, transformer: ->(v) { v })", true, 2),
        ("ParameterType(name: 'channel', regexp: [[/stdout/]], transformer: ->(v) { v })", false, 2),
        ("ParameterType(name: 'channel', regexp: /stdout/, prefer_for_regexp_match: true, transformer: ->(v) { v })", false, 2),
        ("ParameterType(name: 'channel', regexp: /stdout/, transformer: ->(v) { v }.call)", false, 2),
        ("ParameterType(name: name, regexp: /stdout/, transformer: ->(v) { v })", false, 0),
        ("ParameterType(name: 'int', regexp: /stdout/, transformer: ->(v) { v })", false, 0),
        ("def ParameterType(options); end; ParameterType(name: 'channel', regexp: /stdout/, transformer: ->(v) { v })", false, 0),
    ] {
        fs::write(dir.path().join("a.rb"), declaration).unwrap();
        fs::write(dir.path().join("z.rb"), if duplicate { declaration } else { "" }).unwrap();
        let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
        assert_eq!(output.status.code(), Some(2), "{declaration}");
        let rows = records(output.stdout);
        assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], definitions, "{declaration}: {rows:?}");
        assert!(rows.iter().all(|r| r["rule"] != "ambiguous-step" && r["rule"] != "unused-definition"), "{declaration}: {rows:?}");
        assert_eq!(rows.iter().any(|r| r["rule"] == "duplicate-matcher"), definitions > 0, "{declaration}");
    }
}

#[test]
fn malformed_ruby_literal_inputs_report_incompleteness_without_losing_controls() {
    for source in [
        "step '",
        "step \"",
        "send '",
        "ParameterType(name: '",
        "Given('",
    ] {
        let (exit, rows, _) = analyze(source, &["--fail-on-incomplete"]);
        assert_eq!(exit, 2, "{source}");
        assert_eq!(
            rows.as_array().unwrap().last().unwrap()["corpus"]["incomplete"],
            true,
            "{source}"
        );
    }
    let (_, rows, _) = analyze("Given('first') { work() }\nGiven('second') { work() }", &[]);
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["rule"] == "duplicate-handler"));
}

#[test]
fn ruby_review_matcher_text_does_not_spend_budget_on_internal_identity() {
    let (exit, rows, _) = analyze(
        "Given('abx') { alpha() }; Given('cdz') { beta() }; Given('efq') { gamma() }",
        &["--max-candidate-comparisons", "1", "--fail-on-incomplete"],
    );
    assert_eq!(exit, 0);
    let summary = rows.as_array().unwrap().last().unwrap();
    assert_eq!(summary["corpus"]["incomplete"], false);
    assert_eq!(
        summary["analysis"]["candidateSources"]["matcherBlocking"]["evaluated"],
        0
    );
    let (_, rows, _) = analyze(
        "Given('a  panel') { open() }; Given('a panel') { close() }",
        &[],
    );
    assert!(rows
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["rule"] == "normalized-matcher"));
    assert!(!rows.to_string().contains("ruby:CucumberExpression"));
}

/// The candidate source that proposes pairs from shared matcher shingles.
const BLOCKING: &str = "matcherBlocking";

/// Writes `files` and a `.cuke-dedup.json` holding `config`, runs the binary over `*.rb`, and
/// returns the JSONL records and stderr, failing with both when no record was produced.
fn ruby_pair_run(files: &[(String, String)], config: &str) -> (Vec<Value>, String) {
    let directory = tempfile::tempdir().unwrap();
    for (name, contents) in files {
        ruby_write(directory.path(), name, contents);
    }
    ruby_write(directory.path(), ".cuke-dedup.json", config);
    let output = run_project(directory.path(), "*.rb", &[]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    let rows = records(output.stdout);
    let code = output.status.code();
    assert!(!rows.is_empty(), "exit {code:?}: {stderr}");
    (rows, stderr)
}

/// One `steps.rb` holding two definitions, each `(matcher, body)`.
fn ruby_pair_file(
    left: (impl std::fmt::Display, impl std::fmt::Display),
    right: (impl std::fmt::Display, impl std::fmt::Display),
) -> Vec<(String, String)> {
    let source = format!(
        "Given('{}') {{ {} }}\nThen('{}') {{ {} }}",
        left.0, left.1, right.0, right.1
    );
    vec![("steps.rb".to_owned(), source)]
}

/// Spreads `definitions` over files of 32, each led by `prelude`: one large file extracts slowly.
fn ruby_spread(prelude: &str, definitions: &[String]) -> Vec<(String, String)> {
    let body = |chunk: &[String]| format!("{prelude}\n{}", chunk.join("\n"));
    let chunks = definitions.chunks(32).map(body).enumerate();
    chunks
        .map(|(index, body)| (format!("s{index:03}.rb"), body))
        .collect()
}

/// The census as `[truncated, evaluated, source evaluated, source skipped]`; a truncated census
/// prints exactly one incompleteness warning and a complete one prints none, and the public
/// `skippedCandidateComparisons` aggregate is the sum of every source's skipped count.
fn ruby_census(rows: &[Value], stderr: &str, source: &str) -> [u64; 4] {
    let analysis = &rows.last().unwrap()["analysis"];
    let truncated = u64::from(analysis["truncated"].as_bool().unwrap());
    let warnings = stderr.matches("analysis is incomplete").count();
    assert_eq!(u64::try_from(warnings).unwrap(), truncated, "{stderr}");
    let sources = analysis["candidateSources"].as_object().unwrap();
    let skipped_total: u64 = sources
        .values()
        .map(|census| census["skipped"].as_u64().unwrap())
        .sum();
    assert_eq!(
        analysis["skippedCandidateComparisons"], skipped_total,
        "{stderr}"
    );
    let census = &analysis["candidateSources"][source];
    let total = &analysis["candidateComparisonsEvaluated"];
    let [total, evaluated, skipped] =
        [total, &census["evaluated"], &census["skipped"]].map(|count| count.as_u64().unwrap());
    [truncated, total, evaluated, skipped]
}

/// Runs `files` under `config`, asserts the `source` census, and returns the records.
fn ruby_pair_census(
    files: &[(String, String)],
    config: &str,
    source: &str,
    census: [u64; 4],
) -> Vec<Value> {
    let (rows, stderr) = ruby_pair_run(files, config);
    assert_eq!(ruby_census(&rows, &stderr, source), census, "{config}");
    rows
}

/// The census of a run whose single candidate was verified or, when `truncated`, skipped.
fn ruby_single(truncated: bool) -> [u64; 4] {
    let skipped = u64::from(truncated);
    [skipped, 1 - skipped, 1 - skipped, skipped]
}

/// The finding records of `rule`.
fn ruby_rule_rows<'a>(rows: &'a [Value], rule: &str) -> Vec<&'a Value> {
    rows.iter().filter(|row| row["rule"] == rule).collect()
}

/// Primary then related line numbers of a finding.
fn ruby_finding_lines(row: &Value) -> Vec<u64> {
    let related = row["related"].as_array().unwrap().iter();
    let mut lines = vec![row["primary"]["line"].as_u64().unwrap()];
    lines.extend(related.map(|location| location["line"].as_u64().unwrap()));
    lines
}

/// Runs `source` beside the trusted assertion provider and returns the records and stderr.
fn ruby_assertion_run(source: &str) -> (Vec<Value>, String) {
    let root = assertion_project(ASSERTION_PROVIDER, source, true);
    let output = run_project(root.path(), "*.rb", &[]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    (records(output.stdout), stderr)
}

/// Near-duplicate pairs are never collapsed into clusters, and a suppressed duplicate-handler
/// pair stays a pair outside the active cluster it is connected to.
#[test]
fn ruby_pair_clusters_keep_fuzzy_and_suppressed_evidence_apart() {
    let source = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"]
        .map(|word| format!("Given('legacy step {word}') {{ verify(:ready) }}"))
        .join("\n");
    let config = r#"{"suppressions":[{"rule":"duplicate-handler","reason":"accepted legacy pair","matcher":"legacy step foxtrot"}]}"#;
    let (rows, _) = ruby_pair_run(&[("steps.rb".to_owned(), source)], config);
    // Four near edges connect five definitions, enough to form a cluster if near pairs collapsed.
    // The edges depend on matcher-similarity margins: alpha against bravo..echo scores
    // 0.906-0.921, above the near gate, and alpha against foxtrot 0.888, below it.
    let near = ruby_rule_rows(&rows, "near-duplicate-step");
    let near_lines = near.iter().map(|row| ruby_finding_lines(row));
    let expected: Vec<Vec<u64>> = vec![vec![1, 2], vec![1, 3], vec![1, 4], vec![1, 5]];
    assert_eq!(near_lines.collect::<Vec<_>>(), expected);
    let pairs = near.iter().map(|row| &row["evidence"]);
    assert!(pairs
        .clone()
        .all(|pair| pair["comparison"].is_object() && pair["cluster"].is_null()));
    let duplicates = ruby_rule_rows(&rows, "duplicate-handler");
    assert_eq!(duplicates.len(), 2);
    let (suppressed, active): (Vec<_>, Vec<_>) = duplicates
        .into_iter()
        .partition(|row| row["suppression"]["reason"] == "accepted legacy pair");
    assert_eq!(ruby_finding_lines(suppressed[0]), [1, 6]);
    assert!(suppressed[0]["evidence"]["comparison"].is_object());
    assert_eq!(ruby_finding_lines(active[0]), [1, 2, 3, 4, 5]);
    assert_eq!(active[0]["evidence"]["cluster"]["memberCount"], 5);
}

/// Each candidate source feeds its own rule; matcher-blocking survivors and pairs whose sibling
/// rule is disabled report the computed similarities, never a placeholder.
#[test]
fn ruby_candidate_sources_feed_their_rules_with_computed_evidence() {
    let enabled = ("the account is enabled", "open(); fill(); save()");
    let plural = ("the accounts are enabled", "open(); fill(); save()");
    let record = (
        "the account record is enabled",
        "open(); fill(); save(); close(); archive()",
    );
    let records = ("the account records are enabled", "open(); fill(); save()");
    let threshold = r#"{"nearDuplicateHandlerSimilarity":0.5}"#;
    let identical = ruby_pair_file(enabled, plural);
    let blocking = ruby_pair_file(record, records);
    let normalized = ruby_pair_file(("a  panel", "open()"), ("a panel", "close()"));
    let dark = ("the theme is chosen", "page.theme('dark')");
    let structural = ruby_pair_file(dark, ("the theme is picked", "page.theme('light')"));
    let expected = [
        "identicalHandler: duplicate-handler near-duplicate-step",
        "matcherBlocking: near-duplicate-step",
        "normalizedMatcher: normalized-matcher",
        "structuralHandler: parameterization-candidate",
    ];
    let pairs = [identical, blocking, normalized, structural];
    let configs = ["{}", threshold, "{}", "{}"];
    for ((pair, config), expected) in pairs.iter().zip(configs).zip(expected) {
        let (source, rules) = expected.split_once(": ").unwrap();
        let rows = ruby_pair_census(pair, config, source, [0, 1, 1, 0]);
        let found = rows.iter().filter_map(|row| row["rule"].as_str());
        assert_eq!(found.collect::<Vec<_>>().join(" "), rules);
    }
    // Identical handlers make the matcher similarity independent of the candidate source.
    let evidence = |left, right, config| {
        let (rows, _) = ruby_pair_run(&ruby_pair_file(left, right), config);
        ruby_rule_rows(&rows, "near-duplicate-step")[0]["evidence"].clone()
    };
    let identical = evidence(record, (records.0, record.1), "{}");
    let survivor = evidence(record, records, threshold);
    assert!(identical["matcherSimilarity"].as_f64().unwrap() < 1.0);
    let fields = ["matcherSimilarity", "handlerSimilarity", "handlerEvidence"];
    let overlap = "Handlers share 60.0% ordered behavior";
    let expected = [identical[fields[0]].clone(), 0.6.into(), overlap.into()];
    assert_eq!(fields.map(|field| survivor[field].clone()), expected);
    assert!(survivor["comparison"].is_object());
    let baseline = evidence(enabled, plural, "{}")["matcherSimilarity"].clone();
    for (disabled, kept) in [
        ("duplicate-handler", "near-duplicate-step"),
        ("near-duplicate-step", "duplicate-handler"),
    ] {
        let config = format!(r#"{{"rules":{{"{disabled}":"off"}}}}"#);
        let (rows, _) = ruby_pair_run(&ruby_pair_file(enabled, plural), &config);
        assert!(ruby_rule_rows(&rows, disabled).is_empty());
        let evidence = &ruby_rule_rows(&rows, kept)[0]["evidence"];
        assert_eq!(evidence["matcherSimilarity"], baseline, "{kept}");
        assert_eq!(evidence["handlerSimilarity"], 1.0, "{kept}");
        assert!(evidence["comparison"].is_object());
    }
}

/// Matcher-blocking pairs whose wording is not near, and structural pairs whose only rule is
/// disabled, are evaluated without charging the oversized handler or matcher matrix.
#[test]
fn ruby_unneeded_similarity_stages_do_not_spend_pair_work() {
    // 10,001 events per handler: the handler LCS matrix exceeds the 100,000,000-cell ceiling.
    let calls = "shared(); ".repeat(10_000);
    let (left, right) = (format!("{calls}left()"), format!("{calls}right()"));
    let (x, y) = ("x".repeat(64), "y".repeat(64));
    let near = ["the parcel is ready", "the parcel is now ready"].map(String::from);
    for ([given, then], truncated) in [
        ([format!("abc {x}"), format!("abc {y}")], false),
        (near, true),
    ] {
        let pair = ruby_pair_file((given, &left), (then, &right));
        let rows = ruby_pair_census(&pair, "{}", BLOCKING, ruby_single(truncated));
        assert_eq!(rows.len(), 1);
    }
    // 10,002-character matchers cross the production matrix ceiling; the original's 100,000
    // characters exceed the Ruby regex resource limit and drop the definitions. This half pins
    // the disabled-rule gate only: no Ruby structural pair is otherwise near-eligible. Action
    // streams require distinct events, configured-assertion streams bind every literal's source
    // text, and every other stream requires the same handler.
    let prefix = "a".repeat(10_001);
    let left = (format!("{prefix}b"), "page.theme(1)");
    let pair = ruby_pair_file(left, (format!("{prefix}c"), "page.theme(2)"));
    for (config, truncated) in [
        (r#"{"rules":{"parameterization-candidate":"off"}}"#, false),
        ("{}", true),
    ] {
        let rows = ruby_pair_census(&pair, config, "structuralHandler", ruby_single(truncated));
        assert_eq!(rows.len(), 1, "{config}");
    }
}

/// A pair mixing a complete event stream with an exact-policy handler needs the same handler in
/// both orders, never charges the matcher matrix it cannot use, and keeps positive controls.
#[test]
fn ruby_mixed_near_profiles_preserve_outcomes_and_budget_in_both_orders() {
    let check = "Assertions.expect(page).to_be(:ready); open(); fill()";
    // An untrusted `expect` keeps the exact-handler policy: its stream carries no complete marker.
    let exact = format!("{check}; expect(other)");
    let (one, two) = (format!("{check}; notify(1)"), format!("{check}; notify(2)"));
    let prefix = "a".repeat(10_001);
    let long = [format!("{prefix}b"), format!("{prefix}c")];
    let short = ["the parcel is verified", "the parcel is now verified"].map(String::from);
    for (left, right, matchers, finding) in [
        (check, exact.as_str(), &short, false),
        (check, exact.as_str(), &long, false),
        (one.as_str(), two.as_str(), &short, true),
        (exact.as_str(), exact.as_str(), &short, true),
    ] {
        for (first, second) in [(left, right), (right, left)] {
            let [given, then] = matchers;
            let source = format!("Given('{given}') {{ {first} }}\nThen('{then}') {{ {second} }}");
            let (rows, stderr) = ruby_assertion_run(&source);
            let census = ruby_census(&rows, &stderr, BLOCKING);
            assert_eq!(census[..2], [0, 1], "{first} | {second}");
            // Only the two missing-feature-file warnings: no incompleteness diagnostic.
            assert_eq!(stderr.matches("cuke-dedup:").count(), 2, "{stderr}");
            let rules = ["near-duplicate-step", "duplicate-handler"].map(Value::from);
            let reported = rows.iter().any(|row| rules.contains(&row["rule"]));
            assert_eq!(reported, finding, "{first} | {second}");
        }
    }
}

/// Matcher-blocking prefilters admit every pair that can reach the handler-similarity gate and
/// reject trivial, unresolved, conflicting and runtime-incompatible pairs.
#[test]
fn ruby_matcher_blocking_prefilters_are_safe_upper_bounds() {
    let ten: Vec<_> = (0..10).map(|step| format!("step{step}()")).collect();
    let ten_nine = format!("{} | {} | 1", ten.join("; "), ten[..9].join("; "));
    // `left | right | candidates`: overlap at least 1/2 of the longer stream is the gate.
    for case in [
        "open(); fill(); save(); close() | open(); fill() | 1",
        "open(); fill(); save(); close(); log() | open(); fill() | 0",
        "open(); open(); open() | open(); fill(); fill() | 0",
        "open(); fill(); fill() | open(); open(); open() | 0",
        "@x = 1; return | @x = 1; return; return | 0",
        " |  | 0",
        " | open() | 0",
        "|value:| open(value) | |value:| open(value) | 0",
        "verify(:ready) | verify(:rejected) | 0",
        "verify(:ready); log() | verify(:ready); audit() | 1",
        "open(); fill(); save() | save(); fill(); open() | 1",
        ten_nine.as_str(),
    ] {
        let parts = case.rsplitn(2, " | ").collect::<Vec<_>>();
        let (left, right) = parts[1].split_once(" | ").unwrap();
        let pair = ruby_pair_file(
            ("the parcel status is ready", left),
            ("the parcel status is now ready", right),
        );
        let candidates = parts[0].parse::<u64>().unwrap();
        let (rows, stderr) = ruby_pair_run(&pair, "{}");
        let census = ruby_census(&rows, &stderr, BLOCKING);
        assert_eq!(census, [0, candidates, candidates, 0], "{case}");
    }
    // A file-local capture binds each handler to its own file: only the same-file pair is proposed.
    let given = "Given('the parcel status is ready') { open(captured); fill() }";
    let then = "Then('the parcel status is now ready') { open(captured); save() }";
    let file = |name: &str, body| (name.to_owned(), format!("captured = 1\n{body}"));
    let joined = vec![file("steps.rb", format!("{given}\n{then}"))];
    let split = vec![
        file("a.rb", given.to_owned()),
        file("b.rb", then.to_owned()),
    ];
    for (files, candidates) in [(joined, 1), (split, 0)] {
        ruby_pair_census(&files, "{}", BLOCKING, [0, candidates, candidates, 0]);
    }
    // Configured assertion values keep equal handler structures apart in the event stream: the
    // equal-structure shortcut needs identical ordered events, so the conflicting pair still
    // meets the overlap gate, and two shared calls carry the control past it.
    for (tail, candidates) in [("", 0), ("; open(); fill()", 1)] {
        let check = |value| format!("Assertions.expect(page).to_be('{value}'){tail}");
        let (ready, rejected) = (check("ready"), check("rejected"));
        let given = format!("Given('the parcel status is ready') {{ {ready} }}");
        let then = format!("Then('the parcel status is now ready') {{ {rejected} }}");
        let (rows, stderr) = ruby_assertion_run(&format!("{given}\n{then}"));
        let census = ruby_census(&rows, &stderr, BLOCKING);
        assert_eq!(census, [0, candidates, candidates, 0], "{tail}");
    }
}

/// Suppression lookup work is charged per suppression entry for each rule that reports a finding,
/// summed across the pair's rules, and never for a configured rule the pair does not report.
#[test]
fn ruby_suppression_work_is_charged_only_for_reported_rules() {
    // Each entry costs 2 * (1 + 2 * (1 + 8 + 4,000)) = 16,038 for this pair against the
    // 100,000,000 limit: 2 * 3,117 entries fit and 2 * 3,118 do not.
    let pad = "x".repeat(3_978);
    let body = "open(); fill(); save()";
    let left = (format!("the account is enabled{pad}"), body);
    let right = (format!("the accounts are enabled{}", &pad[2..]), body);
    let mut files = ruby_pair_file(left, right);
    // Listed first, its short matcher keeps unmatched-suppression scanning cheap.
    files.push(("a.rb".to_owned(), "Given('a') { other() }".to_owned()));
    for (rules, count, truncated) in [
        ("duplicate-handler near-duplicate-step", 3_117, false),
        ("duplicate-handler near-duplicate-step", 3_118, true),
        (
            "parameterization-candidate near-duplicate-step",
            3_118,
            false,
        ),
    ] {
        let entry = |rule| format!(r#"{{"rule":"{rule}","reason":"migration","path":"**"}}"#);
        let entries: Vec<_> = rules
            .split(' ')
            .map(|rule| vec![entry(rule); count])
            .collect();
        let entries = entries.concat().join(",");
        let config = format!(r#"{{"suppressions":[{entries}]}}"#);
        ruby_pair_census(&files, &config, "identicalHandler", ruby_single(truncated));
    }
}

/// Postings past 256 members use a per-posting lexical fallback that keeps interleaved groups
/// independent, a 256-member posting is still expanded, and a pair repeated across postings
/// is charged once.
#[test]
fn ruby_saturated_matcher_postings_use_independent_linear_fallbacks() {
    let threshold = r#"{"nearDuplicateHandlerSimilarity":0.5}"#;
    let paired = |rows: &[Value], left: &str, right: &str| {
        let mut comparisons = rows.iter().map(|row| &row["evidence"]["comparison"]);
        comparisons.any(|row| row["leftMatcher"] == left && row["rightMatcher"] == right)
    };
    let record = |index| format!("the account record {index:03} is enabled");
    // Digit runs share one shingle, so every shingle's posting holds every definition.
    for (count, evaluated) in [(257, 256), (256, 32_640)] {
        let body = |index| format!("open(); fill(); specific{index}()");
        let definition = |index| format!("Given('{}') {{ {} }}", record(index), body(index));
        let files = ruby_spread("", &(0..count).map(definition).collect::<Vec<_>>());
        let rows = ruby_pair_census(&files, threshold, BLOCKING, [0, evaluated, evaluated, 0]);
        assert!(paired(&rows, &record(0), &record(1)), "{count}");
    }
    // Shared shingles saturate across both groups; each group's own shingles saturate alone.
    // Cross-group pairs overlap 1/5 and never reach the gate, so only per-group fallbacks insert.
    let definition = |index, group, shingle| {
        let calls = format!("g{group}a(); g{group}b(); g{group}c(); open(); s{group}x{index}()");
        format!("Given('{index:03} {shingle} shared') {{ {calls} }}")
    };
    let groups =
        (0..257).flat_map(|index| [definition(index, 0, "aaa"), definition(index, 1, "bbb")]);
    let files = ruby_spread("", &groups.collect::<Vec<_>>());
    let rows = ruby_pair_census(&files, threshold, BLOCKING, [0, 512, 512, 0]);
    assert!(paired(&rows, "000 aaa shared", "001 aaa shared"));
    assert!(paired(&rows, "000 bbb shared", "001 bbb shared"));
    // 78 shingles share one 256-member posting: 32,640 unique pairs, 2,545,920 if each repeat
    // were charged against the 2,000,000 proposal limit. Disjoint events insert nothing.
    let prefix: String = (0x4E00..0x4E50).filter_map(char::from_u32).collect();
    let definition = |index: u32| {
        let last = char::from_u32(0x5000 + index).unwrap();
        format!("Given('{prefix}{last}') {{ event{index}() }}")
    };
    let files = ruby_spread("", &(0..256).map(definition).collect::<Vec<_>>());
    ruby_pair_census(&files, "{}", BLOCKING, [0, 0, 0, 0]);
}

/// Matcher blocking fails closed, dropping its candidates, at each work budget: the
/// per-definition shingle limit, pinned exactly at its boundary, the event-work limit and the
/// proposal limit, each bracketed by the nearest counts on either side.
#[test]
fn ruby_matcher_blocking_fails_closed_at_every_work_budget() {
    // n distinct characters yield n - 2 shingles; the limit is 4,096 per definition.
    for (characters, truncated) in [(4_098, false), (4_099, true)] {
        let end = 0x4E00 + characters;
        let matcher: String = (0x4E00..end).filter_map(char::from_u32).collect();
        let ready = ("the parcel is ready", "open(); fill()");
        let now = ("the parcel is now ready", "open(); fill(); save()");
        let mut files = ruby_pair_file(ready, now);
        let wide = format!("Given('{matcher}') {{ open() }}");
        files.push(("wide.rb".to_owned(), wide));
        ruby_pair_census(&files, "{}", BLOCKING, ruby_single(truncated));
    }
    // 256 definitions share one posting: 32,640 pairs each charged 4 * events. 76 events cost
    // 9,922,560 and 77 cost 10,053,120, bracketing the 10,000,000 limit; disjoint events insert
    // nothing.
    for (events, truncated) in [(76, 0), (77, 1)] {
        let definition = |index| {
            let calls: String = (0..events).map(|e| format!("e{index}x{e}();")).collect();
            format!("Given('the account record {index:03} is enabled') {{ {calls} }}")
        };
        let files = ruby_spread("", &(0..256).map(definition).collect::<Vec<_>>());
        ruby_pair_census(&files, "{}", BLOCKING, [truncated, 0, 0, truncated]);
    }
    ruby_assert_proposal_budget_precedes_semantic_rejection();
}

/// Asserts that matcher blocking charges every unique proposal against the 2,000,000 limit,
/// bracketed by 1,996,800 and 2,011,136 proposals, before rejecting runtime-incompatible pairs,
/// and charges event work only for pairs that survive that rejection.
fn ruby_assert_proposal_budget_precedes_semantic_rejection() {
    // 2,048 definitions; each parity class splits them into 8 postings of 256 by three GF(2)
    // functionals of the id. 22 classes propose 1,996,800 unique pairs, 23 propose 2,011,136.
    // File-local captures make every cross-file pair runtime-incompatible. Two events per handler
    // would cost at least 6 event work per proposal, pushing the rejected pairs alone past the
    // 10,000,000 event limit if rejection came after the event charge.
    let mut state = 1_u64;
    let mut classes = Vec::new();
    while classes.len() < 23 {
        let functionals = [(); 3].map(|()| {
            state = (state * 1_103_515_245 + 12_345) % (1 << 31);
            u32::try_from((state >> 8) & 0x7FF).unwrap()
        });
        let mut basis = Vec::<u32>::new();
        for mut functional in functionals {
            for row in &basis {
                functional = functional.min(functional ^ row);
            }
            basis.extend((functional != 0).then_some(functional));
        }
        classes.extend((basis.len() == 3).then_some(functionals));
    }
    let tag = |id: u32, (class, functionals): (usize, &[u32; 3])| -> String {
        let parity = |bit: usize| ((id & functionals[bit]).count_ones() & 1) << bit;
        let group = (0..3).map(parity).sum::<u32>();
        let first = 0x4E00 + 3 * (u32::try_from(class).unwrap() * 8 + group);
        (first..first + 3).filter_map(char::from_u32).collect()
    };
    for (count, truncated) in [(22, 0), (23, 1)] {
        let definition = |id| {
            let matcher: String = classes[..count]
                .iter()
                .enumerate()
                .map(|c| tag(id, c))
                .collect();
            format!("Given('{matcher}') {{ store{id}(captured); settle{id}() }}")
        };
        let definitions: Vec<_> = (0..2_048).map(definition).collect();
        let files = ruby_spread("captured = 1", &definitions);
        ruby_pair_census(&files, "{}", BLOCKING, [truncated, 0, 0, truncated]);
    }
}

/// Pair verification stops at the 1,000,000,000 total similarity-work limit, pinned at the
/// literal length whose evidence work first crosses it on the last pair.
#[test]
fn ruby_pair_verification_fails_closed_at_the_total_similarity_budget() {
    // 32 near matchers over one handler: 496 pairs, each charged evidence work for its
    // duplicate-handler and near-duplicate-step findings. One more literal character pushes the
    // last pair past the limit; identical handlers charge no handler matrix.
    for (length, truncated) in [(58_527, 0), (58_528, 1)] {
        let literal = "x".repeat(length);
        let definition = |index| {
            let source = format!(
                "Given('the account record {index:02} is enabled') {{ open('{literal}') }}\n"
            );
            (format!("s{index:02}.rb"), source)
        };
        let files: Vec<_> = (0..32).map(definition).collect();
        let census = [truncated, 496 - truncated, 465 - truncated, truncated];
        let rows = ruby_pair_census(&files, "{}", BLOCKING, census);
        let identical = &rows.last().unwrap()["analysis"]["candidateSources"]["identicalHandler"];
        assert_eq!(identical["evaluated"], 31);
    }
}

#[test]
fn ruby_review_block_forms_preserve_call_ownership_and_effects() {
    for (left, right, duplicate) in [
        ("{ work() }", "do work() end", true),
        (
            "{ |value| store(value) }",
            "do |value| store(value) end",
            true,
        ),
        ("{ later { work() } }", "do later do work() end end", true),
        ("{ work(1) }", "do work(2) end", false),
        ("{ before(); after() }", "do after(); before() end", false),
        (
            "{ |value| store(value) }",
            "do |value| store(other) end",
            false,
        ),
        (
            "{ outer inner { work() } }",
            "{ outer inner do work() end }",
            false,
        ),
        ("{ |value| value }", "do |value| value end", false),
    ] {
        let source = format!("Given('first') {left}\nThen('second') {right}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .any(|row| row["rule"] == "duplicate-handler"),
            duplicate,
            "{source}"
        );
    }
}

#[test]
fn ruby_review_custom_parameter_overlap_requires_a_valid_witness() {
    for (pattern, other, overlap) in [
        ("/red|blue/", "/red|green/", true),
        ("/red|blue/", "/green/", false),
        ("/[a-z]+/", "/[a-z]+/", false),
    ] {
        let source = format!("ParameterType(name: 'tone', regexp: {pattern}, transformer: ->(v) {{ v }})\nParameterType(name: 'shade', regexp: {other}, transformer: ->(v) {{ v }})\nGiven('paint {{tone}}') {{ brush() }}\nGiven('paint {{shade}}') {{ spray() }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .any(|row| row["rule"] == "overlapping-matcher"),
            overlap,
            "{source}"
        );
    }
}

#[test]
fn ruby_review_handler_forms_preserve_binding_and_call_ownership() {
    let mut failures = Vec::new();
    for (prefix, left, right, duplicate) in [
        ("", "{ log_in }", "{ log_in }", true),
        ("", "{ log_in }", "{ log_out }", false),
        ("", "{ work() }", "do work() end", true),
        ("", "{ work(1) }", "do work(2) end", false),
        (
            "",
            "{ outer inner { work() } }",
            "{ outer inner do work() end }",
            false,
        ),
        ("", "{ |value| value }", "{ |value| value }", false),
        ("value = 1;", "{ value }", "{ value }", false),
        ("", "{ store(caller) }", "{ store(caller) }", false),
        ("", "{ store(caller()) }", "{ store(caller()) }", false),
        (
            "",
            "{ store(caller_locations) }",
            "{ store(caller_locations) }",
            false,
        ),
        (
            "",
            "{ store(method(:work).source_location) }",
            "{ store(method(:work).source_location) }",
            false,
        ),
        (
            "",
            "{ |caller| store(caller) }",
            "{ |caller| store(caller) }",
            true,
        ),
        (
            "caller = 'value';",
            "{ store(caller) }",
            "{ store(caller) }",
            true,
        ),
        (
            "",
            "{ caller = 'value'; store(caller) }",
            "{ caller = 'value'; store(caller) }",
            true,
        ),
        ("", "{ store('caller') }", "{ store('caller') }", true),
        (
            "",
            "{ store(method(:caller).call) }",
            "{ store(method(:caller).call) }",
            false,
        ),
        (
            "",
            "{ store(local_variables) }",
            "{ store(local_variables) }",
            false,
        ),
        ("", "{ _1 }", "{ _1 }", false),
        ("", "{ it }", "{ it }", false),
        ("", "{ store(it) }", "{ store(it) }", false),
        ("", "{ it() }", "{ it() }", true),
        ("", "{ |it| store(it) }", "{ |it| store(it) }", true),
        ("it = 'value';", "{ store(it) }", "{ store(it) }", true),
        (
            "",
            "{ it = 'value'; store(it) }",
            "{ it = 'value'; store(it) }",
            true,
        ),
        ("", "{ store('it') }", "{ store('it') }", true),
        (
            "",
            "{ later { store(caller) } }",
            "{ later { store(caller) } }",
            false,
        ),
    ] {
        let source = format!("{prefix}\nGiven('first') {left}\nThen('second') {right}");
        let (_, rows, _) = analyze(&source, &[]);
        let actual = rows
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["rule"] == "duplicate-handler");
        if actual != duplicate {
            failures.push(format!("expected {duplicate}, got {actual}: {source}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ruby_lexical_renaming_preserves_findings_and_conflicts() {
    for (left, right, duplicate) in [
        ("|value| store(value)", "|item| store(item)", true),
        (
            "value = read(); store(value)",
            "item = read(); store(item)",
            true,
        ),
        ("|a, b| store(a, b)", "|x, y| store(y, x)", false),
        (
            "|a| items.each { |a| store(a) }; store(a)",
            "|x| items.each { |y| store(y) }; store(x)",
            true,
        ),
        (
            "|a| items.each { |b| store(a) }",
            "|x| items.each { |y| store(y) }",
            false,
        ),
        (
            "items.each { |a| store(a) }; items.each { |b| store(b) }",
            "items.each { |x| store(x) }; items.each { |y| store(y) }",
            true,
        ),
        ("|a| store(a.value)", "|x| store(x.other)", false),
        (
            "|a| a.first, value = read(); store(value)",
            "|x| x.second, item = read(); store(item)",
            false,
        ),
        (
            "|a| a[key_one], value = read(); store(value)",
            "|x| x[key_two], item = read(); store(item)",
            false,
        ),
        (
            "value = 'first'; store(value)",
            "item = 'second'; store(item)",
            false,
        ),
        ("value += 1", "value += 2", false),
        ("|a| store({a: a})", "|x| store({x: x})", false),
        (
            "store(value); value = read()",
            "value = read(); store(value)",
            false,
        ),
        (
            "|a| later { a = read(); store(a) }; store(a)",
            "|x| later { y = read(); store(y) }; store(x)",
            false,
        ),
        (
            "|a| schedule(->(b) { store(a, b) })",
            "|x| schedule(->(y) { store(x, y) })",
            true,
        ),
        ("|a| store(:a)", "|x| store(:x)", false),
        (
            "|caller| store(caller())",
            "|caller| store(caller())",
            false,
        ),
        (
            "items.each { |caller| store(caller) }; store(caller)",
            "items.each { |caller| store(caller) }; store(caller)",
            false,
        ),
    ] {
        let source = format!("Given('first') {{ {left} }}; Then('second') {{ {right} }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_handler_finding(&rows, duplicate, &source);
    }
}

#[test]
fn ruby_loaded_constant_providers_preserve_registration_and_reject_mutation() {
    for (provider, use_code, interference, positive) in [
        (
            "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
            "register = Provider::GIVEN",
            "",
            true,
        ),
        (
            "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
            "register = Provider::GIVEN",
            "Provider::GIVEN = other",
            false,
        ),
        (
            "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
            "register = Provider::GIVEN",
            "Provider = other",
            false,
        ),
        (
            "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
            "register = Provider::GIVEN; register = other",
            "",
            false,
        ),
        (
            "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
            "register = Provider::GIVEN; expose(register)",
            "",
            false,
        ),
        (
            "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
            "register = Provider::GIVEN",
            "expose(Provider)",
            false,
        ),
        (
            "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
            "register = Provider::GIVEN",
            "Provider::GIVEN.define_singleton_method(:call) {}",
            false,
        ),
        (
            "module Provider; GIVEN = other.method(:Given); end",
            "register = Provider::GIVEN",
            "",
            false,
        ),
        (
            "module Provider; def Given(*args); end; GIVEN = method(:Given); end",
            "register = Provider::GIVEN",
            "",
            false,
        ),
        (
            "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
            "register = Provider::GIVEN",
            "def Given(*args); end",
            false,
        ),
    ] {
        for (provider_file, steps_file) in [("a.rb", "z.rb"), ("z.rb", "a.rb")] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(provider_file), provider).unwrap();
            fs::write(dir.path().join(steps_file), format!("require_relative '{}'\n{use_code}\nregister.call('same') {{ first() }}; register.call('same') {{ second() }}", provider_file.trim_end_matches(".rb"))).unwrap();
            fs::write(dir.path().join("interference.rb"), interference).unwrap();
            let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
            let rows = records(output.stdout);
            assert_eq!(
                rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
                positive,
                "{provider} / {use_code} / {interference}: {rows:?}"
            );
            if positive {
                assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], false);
            } else {
                assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], true);
            }
        }
    }
}

#[test]
fn ruby_provider_resolution_requires_preceding_load_edges() {
    for (entry, positive) in [
        ("require_relative 'barrel'; register = Barrel::GIVEN; register.call('same') { first() }; register.call('same') { second() }", true),
        ("register = Barrel::GIVEN; require_relative 'barrel'; register.call('same') { first() }; register.call('same') { second() }", false),
        ("register = Barrel::GIVEN; register.call('same') { first() }; register.call('same') { second() }", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("provider.rb"), "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end").unwrap();
        fs::write(dir.path().join("barrel.rb"), "require_relative 'provider'; module Barrel; GIVEN = Provider::GIVEN; end").unwrap();
        fs::write(dir.path().join("entry.rb"), entry).unwrap();
        let rows = records(run_project(dir.path(), "*.rb", &[]).stdout);
        assert_eq!(rows.iter().any(|r| r["rule"] == "duplicate-matcher"), positive, "{entry}: {rows:?}");
    }
}

#[test]
fn ruby_closed_instance_dispatch_preserves_unrelated_registrations() {
    for (class, construct, dispatch, trusted) in [
        (
            "class Router; def Given(text, &block); @value = text; end; end",
            "Router.new",
            "router.send(:Given, 'local') {}.tap { observe() }",
            false,
        ),
        (
            "class Router; def Given(text, &block); @value = text; end; end",
            "Router.new",
            "consume(router.public_send(:Given, 'local') {})",
            false,
        ),
        (
            "class Router; def Given(text, &block); @value = text; end; end",
            "Router.new",
            "later { router.__send__(:Given, 'local') {} }",
            false,
        ),
        (
            "class Router; def Given(text, &block); @value = text; end; end",
            "Router.new",
            "router.send(:Given, 'local') {}",
            true,
        ),
        (
            "class Router; def Given(text, &block); @value = text; end; end",
            "Router.new",
            "router.public_send('Given', 'local') {}",
            true,
        ),
        (
            "class Router; def Given(text, &block); @value = text; end; end",
            "unknown",
            "router.send(:Given, 'local') {}",
            false,
        ),
        (
            "class Router; def Given(text, &block); @value = text; end; end",
            "Router.new",
            "router = unknown; router.send(:Given, 'local') {}",
            false,
        ),
        (
            "class Router; def Given(text, &block); @value = text; end; end",
            "Router.new",
            "expose(router); router.send(:Given, 'local') {}",
            false,
        ),
        (
            "class Router; def Given(text, &block); @value = text; end; end",
            "Router.new",
            "router.send(dynamic, 'local') {}",
            false,
        ),
        (
            "class Router; def self.new; Cucumber; end; def Given(text, &block); end; end",
            "Router.new",
            "router.send(:Given, 'local') {}",
            false,
        ),
        (
            "class Router < Other; def Given(text, &block); end; end",
            "Router.new",
            "router.send(:Given, 'local') {}",
            false,
        ),
    ] {
        let source = format!("{class}\nrouter = {construct}\n{dispatch}\nGiven('first') {{ work() }}; Given('second') {{ work() }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_handler_finding(&rows, trusted, &source);
        assert_eq!(
            rows.as_array().unwrap().last().unwrap()["corpus"]["incomplete"],
            !trusted,
            "{source}"
        );
    }
}

#[test]
fn ruby_provider_cycles_and_lexical_constant_shadowing_do_not_gain_trust() {
    for (provider_prefix, barrel, entry) in [
        ("require_relative 'entry'", "", "require_relative 'provider'; register = Provider::GIVEN; register.call('same') { first() }; register.call('same') { second() }"),
        ("", "require_relative 'provider'; module Barrel; Provider = Other; GIVEN = Provider::GIVEN; end", "require_relative 'barrel'; register = Barrel::GIVEN; register.call('same') { first() }; register.call('same') { second() }"),
        ("", "require_relative 'provider'; module Barrel; GIVEN = Provider::GIVEN; end", "label = '長い文字列長い文字列長い文字列'; register = Barrel::GIVEN; require_relative 'barrel'; register.call('same') { first() }; register.call('same') { second() }"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("provider.rb"), format!("{provider_prefix}\nROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end")).unwrap();
        fs::write(dir.path().join("barrel.rb"), barrel).unwrap();
        fs::write(dir.path().join("entry.rb"), entry).unwrap();
        let rows = records(run_project(dir.path(), "entry.rb", &[]).stdout);
        assert!(!rows.iter().any(|r| r["rule"] == "duplicate-matcher"), "{provider_prefix} / {barrel}: {rows:?}");
        assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], true);
    }
}

#[test]
fn ruby_same_file_provider_values_require_prior_initialization() {
    let provider = "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end";
    let consumer = "register = Provider::GIVEN; register.call('same') { first() }; register.call('same') { second() }";
    for (source, positive) in [
        (format!("{provider}; {consumer}"), true),
        (format!("{consumer}; {provider}"), false),
    ] {
        let (_, rows, _) = analyze(&source, &[]);
        let rows = rows.as_array().unwrap();
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
            positive,
            "{source}"
        );
        assert_eq!(
            rows.last().unwrap()["corpus"]["incomplete"],
            !positive,
            "{source}"
        );
    }
}

#[test]
fn ruby_provider_shared_dependency_paths_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    for depth in 0..24 {
        for branch in ["a", "b"] {
            let source = if depth < 23 {
                format!(
                    "require_relative 'a{}'; require_relative 'b{}'",
                    depth + 1,
                    depth + 1
                )
            } else {
                String::new()
            };
            fs::write(dir.path().join(format!("{branch}{depth}.rb")), source).unwrap();
        }
    }
    fs::write(
        dir.path().join("provider.rb"),
        "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
    )
    .unwrap();
    fs::write(dir.path().join("entry.rb"), "require_relative 'a0'; register = Provider::GIVEN; register.call('unknown') { first() }; Given('same') { first() }; Given('same') { second() }").unwrap();
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(dir.path())
        .args([
            "--definitions",
            "*.rb",
            "--reporters",
            "jsonl",
            "--no-metrics",
            "--fail-on-incomplete",
        ])
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .unwrap();
    let rows = records(output.stdout);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 2);
    assert!(rows.iter().any(|r| r["rule"] == "duplicate-matcher"));
}

/// Selections over the provider-proof budget keep direct findings and stay complete.
#[test]
fn ruby_provider_budget_exhaustion_preserves_direct_findings() {
    for (files, bytes) in [(1025, 0), (9, 7_500_000)] {
        let dir = tempfile::tempdir().unwrap();
        let source = format!("#{}\n", "x".repeat(bytes));
        for index in 0..files {
            fs::write(dir.path().join(format!("a{index}.rb")), &source).unwrap();
        }
        fs::write(
            dir.path().join("steps.rb"),
            "Given('same') { first() }; Given('same') { second() }",
        )
        .unwrap();
        let output = run_project(dir.path(), "*.rb", &["--fail-on-incomplete"]);
        let rows = records(output.stdout);
        assert_eq!(
            rows.last().unwrap()["summary"]["definitionsAnalyzed"],
            2,
            "count={files}, bytes={bytes}"
        );
        assert!(rows.iter().any(|r| r["rule"] == "duplicate-matcher"));
        // Selected files never charge the load graph, so there is no graph error, and the
        // unavailable provider proofs cost nothing because every registration is direct. Under
        // `--fail-on-incomplete` the exit is 1 from the duplicate-matcher error, not 2.
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("source graph exceeds"), "{stderr}");
        assert!(!stderr.contains("proof limit"), "{stderr}");
        assert_eq!(output.status.code(), Some(1), "{stderr}");
    }
}

/// Checks discovery and completeness across bounded large provider suites.
#[test]
fn ruby_provider_scaling_preserves_final_outcomes() {
    for shape in [
        "aliases",
        "single-line",
        "exports",
        "constructors",
        "modules",
        "loads",
    ] {
        let count = 6000;
        let dir = tempfile::tempdir().unwrap();
        let mut source = match shape {
            "exports" => "ROOT_GIVEN = method(:Given); module Provider;\n".to_owned(),
            "constructors" => "class Foreign; def helper; end; end\n".to_owned(),
            "loads" => {
                for depth in 0..50 {
                    fs::write(
                        dir.path().join(format!("p{depth}.rb")),
                        format!("require_relative 'p{}'", depth + 1),
                    )
                    .unwrap();
                }
                fs::write(
                    dir.path().join("p50.rb"),
                    "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
                )
                .unwrap();
                "require_relative 'p0'\n".to_owned()
            }
            _ => "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end\n"
                .to_owned(),
        };
        for index in 0..count {
            source.push_str(&match shape {
                "exports" => format!("C{index} = ::ROOT_GIVEN\n"),
                "constructors" => format!("object_{index} = Foreign.new\n"),
                "modules" => format!("module Provider{index}; GIVEN = ::ROOT_GIVEN; end\n"),
                _ => format!("capture_{index} = Provider::GIVEN\n"),
            });
        }
        if shape == "exports" {
            source.push_str("end\n");
        }
        source.push_str("Given('same') { first() }; Given('same') { second() }");
        if shape == "single-line" {
            source = source.replace('\n', "; ");
        }
        fs::write(dir.path().join("steps.rb"), source).unwrap();
        let output = Command::cargo_bin("cuke-dedup")
            .unwrap()
            .arg(dir.path())
            .args([
                "--definitions",
                "*.rb",
                "--reporters",
                "jsonl",
                "--no-metrics",
            ])
            .timeout(std::time::Duration::from_secs(60))
            .output()
            .unwrap();
        assert!(
            !output.stdout.is_empty(),
            "{shape}: status={:?}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.status.code(), Some(1), "{shape}");
        let rows = records(output.stdout);
        assert_discovery(&rows, 2, false, shape);
        assert!(
            rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
            "{shape}"
        );
    }
}

#[test]
fn ruby_provider_alias_order_scope_and_parse_errors_do_not_grant_trust() {
    for origin in ["method(:Given)", "Provider::GIVEN"] {
        for (before, after, expected) in [
            ("", "register.call('same') { first() }; register.call('same') { second() }", 4),
            ("register.call('same') { first() };", "register.call('same') { second() }", 0),
            ("", "later { register.call('same') { first() }; register.call('same') { second() } }", 0),
            ("", "def delayed; register.call('same') { first() }; register.call('same') { second() }; end", 0),
            ("", "register.call('same') { first() }; register.call('same') { second() }; value = )", 0),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join("provider.rb"), "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end").unwrap();
            fs::write(dir.path().join("steps.rb"), format!("require_relative 'provider'\nGiven('control') {{ first() }}; Given('control') {{ second() }}\n{before} register = {origin}; {after}")).unwrap();
            let rows = project_records(dir.path());
            let expected = if origin == "Provider::GIVEN" && after.ends_with("value = )") { 2 } else { expected };
            assert_discovery(&rows, expected, expected != 4, &format!("{origin}: {before} / {after}"));
            assert_eq!(rows.iter().any(|r| r["rule"] == "duplicate-matcher"), expected > 0);
            if expected == 2 { assert_eq!(rows[0]["evidence"]["comparison"]["leftMatcher"], "control"); }
        }
    }
}

#[test]
fn ruby_instance_proofs_require_parseable_consumers() {
    for (suffix, count) in [("", 2), ("value = )", 0)] {
        let source = format!("class Foreign; def Given; end; end\nforeign = Foreign.new\nforeign.send(:Given)\nGiven('control') {{ first() }}; Given('control') {{ second() }}\n{suffix}");
        let (_, rows, _) = analyze(&source, &[]);
        let rows = rows.as_array().unwrap();
        assert_discovery(rows, count, count == 0, &source);
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
            count == 2
        );
    }
}

#[test]
fn ruby_custom_factories_cannot_establish_instance_isolation() {
    for name in ["new", "allocate"] {
        let method = format!("def {name}; unknown_factory(); end");
        for (factory, trusted) in [
            (String::new(), true),
            (method.clone(), true),
            (format!("def self.{name}; unknown_factory(); end"), false),
            (format!("class << self; {method}; end"), false),
            (
                format!("class << self; if enabled?; {method}; end; end"),
                false,
            ),
        ] {
            let source = format!("class Router; {factory}; def Given(text, &block); @value = text; end; end\nrouter = Router.new\nrouter.send(:Given, 'local') {{}}\nGiven('first') {{ work() }}; Given('second') {{ work() }}");
            let (_, rows, _) = analyze(&source, &[]);
            assert_discovery(
                rows.as_array().unwrap(),
                if trusted { 2 } else { 0 },
                !trusted,
                &source,
            );
            assert_handler_finding(&rows, trusted, &source);
        }
    }
}

#[test]
fn ruby_provider_captures_require_main_receiver_and_initialized_origin() {
    for (provider, positive) in [
        ("ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end", true),
        ("ROOT_GIVEN = method(:Given); module Provider; GIVEN = ROOT_GIVEN; end", true),
        ("module Provider; GIVEN = method(:Given); end", false),
        ("class Provider; GIVEN = method(:Given); end", false),
        ("ROOT_GIVEN = other.method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end", false),
        ("module Provider; GIVEN = ::ROOT_GIVEN; end; ROOT_GIVEN = method(:Given)", false),
        ("ROOT_GIVEN = method(:Given); ROOT_GIVEN = other; module Provider; GIVEN = ::ROOT_GIVEN; end", false),
        ("later { ROOT_GIVEN = method(:Given) }; module Provider; GIVEN = ::ROOT_GIVEN; end", false),
        ("module Provider; end; Provider::GIVEN = method(:Given)", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("provider.rb"), provider).unwrap();
        fs::write(dir.path().join("steps.rb"), "require_relative 'provider'; register = Provider::GIVEN; register.call('same') { first() }; register.call('same') { second() }").unwrap();
        let rows = project_records(dir.path());
        assert_discovery(&rows, if positive { 2 } else { 0 }, !positive, provider);
        assert_eq!(rows.iter().any(|r| r["rule"] == "duplicate-matcher"), positive, "{provider}");
    }
}

#[test]
fn ruby_instance_identity_counts_ineligible_and_qualified_reopenings() {
    for (reopening, trusted) in [
        ("", true),
        (
            "class Unrelated; def send(*args); unknown(); end; end",
            true,
        ),
        ("class Router; def send(*args); unknown(); end; end", false),
        (
            "class ::Router; def send(*args); unknown(); end; end",
            false,
        ),
        (
            "class Router < Object; def send(*args); unknown(); end; end",
            false,
        ),
        (
            "if condition; class ::Router; def send(*args); unknown(); end; end; end",
            false,
        ),
        (
            "module Outer; class ::Router; def send(*args); unknown(); end; end; end",
            false,
        ),
    ] {
        let source = format!("class Router; def Given(*args); end; end; {reopening}\nrouter = Router.new; router.send(:Given, 'local') {{}}; Given('first') {{ work() }}; Given('second') {{ work() }}");
        let (_, rows, _) = analyze(&source, &[]);
        let rows = rows.as_array().unwrap();
        assert_discovery(rows, if trusted { 2 } else { 0 }, !trusted, &source);
        assert_handler_finding(&serde_json::Value::Array(rows.to_vec()), trusted, &source);
    }
}

/// Checks comparison and discovery together so uncertainty cannot silently look complete.
fn assert_handler_outcome(source: &str, definitions: usize, trusted: bool) {
    let (_, rows, _) = analyze(source, &[]);
    assert_discovery(rows.as_array().unwrap(), definitions, !trusted, source);
    assert_handler_finding(&rows, trusted, source);
}

/// Enclosing scopes are rejected before handler fingerprinting can grant equivalence.
#[test]
fn ruby_nested_registration_scopes_never_reach_handler_comparison() {
    for (before, after, trusted) in [
        ("", "", true),
        ("x = 1; def define;", "end", false),
        ("class Host;", "end", false),
        ("module Host;", "end", false),
        ("%w[a b].each do |x|", "end", false),
        ("[1].each { |x|", "}", false),
        ("factory = ->(x) {", "}", false),
        ("class << self;", "end", false),
    ] {
        let source = format!(
            "{before} Given('first') {{ consume(x) }}; Then('second') {{ consume(x) }}; {after}"
        );
        assert_handler_outcome(&source, if trusted { 2 } else { 0 }, trusted);
    }
}

/// Direct constants retain registrar identity; unsupported forwarding shapes fail closed.
#[test]
fn ruby_constant_call_and_forwarding_outcomes() {
    for (extra, trusted) in [
        ("", true),
        (
            "def self.forward(text, &handler); GIVEN.call(text, &handler); end",
            true,
        ),
        ("def self.forward; GIVEN.call('fixed'); end", false),
        ("def self.forward(text); GIVEN.call(text); end", false),
        (
            "def self.forward(text, &handler); effect(); GIVEN.call(text, &handler); end",
            false,
        ),
        (
            "def self.forward(text, &handler); GIVEN.call('changed', &handler); end",
            false,
        ),
        (
            "def self.forward(text, &handler); GIVEN.call(text, &other); end",
            false,
        ),
        ("def self.forward(text, &handler); GIVEN.call; end", false),
        ("GIVEN.other", false),
        ("later { GIVEN.call('nested') {} }", false),
    ] {
        let source = format!("ROOT = method(:Given); module Provider; GIVEN = ::ROOT; {extra}; end; Provider::GIVEN.call('first') {{ work() }}; Provider::GIVEN.call('second') {{ work() }}");
        assert_handler_outcome(&source, if trusted { 2 } else { 0 }, trusted);
    }
}

/// Binding uncertainty and parameter forms are checked through final handler findings.
#[test]
fn ruby_binding_fallbacks_and_parameter_forms() {
    for (prefix, body, trusted) in [
        ("", "|*args| consume(args)", true),
        ("", "|**args| consume(args)", true),
        ("", "|&handler| consume(handler)", true),
        ("for item in []; end;", "work()", false),
        ("case value; in [x]; end;", "consume(x)", false),
        ("", "case value; in [x]; consume(x); end", false),
        ("", "case value; when 1; work(); end", true),
        ("/(?<name>x)/ =~ input;", "work()", false),
        ("", "/(?<name>x)/ =~ input", false),
        (
            "",
            "begin; work(); rescue => error; consume(error); end",
            false,
        ),
        ("", "value => [x]; consume(x)", false),
        ("", "x = 1; consume(x:)", false),
    ] {
        let source = format!("{prefix} Given('first') {{ {body} }}; Then('second') {{ {body} }}");
        assert_handler_outcome(&source, 2, trusted);
    }
}

/// Non-call constant receivers must not trigger repeated dependency-graph searches.
#[test]
fn ruby_non_registration_constant_calls_scale_with_loaded_sources() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..500 {
        let source = if index == 499 {
            "CONFIG = 1".to_owned()
        } else {
            format!("require_relative 'p{}'", index + 1)
        };
        fs::write(dir.path().join(format!("p{index}.rb")), source).unwrap();
    }
    fs::write(
        dir.path().join("steps.rb"),
        format!(
            "require_relative 'p0'; {}; Given('first') {{ work() }}; Then('second') {{ work() }}",
            "CONFIG.fetch;".repeat(2000)
        ),
    )
    .unwrap();
    let output = project_command(dir.path(), "*.rb")
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "non-call scaling exceeded its budget or failed"
    );
    let rows = records(output.stdout);
    assert_discovery(&rows, 2, false, "non-call scaling");
    assert_handler_finding(&Value::Array(rows), true, "non-call scaling");
}

/// Unicode decoding affects matcher identity, while uncertain literals never gain findings.
#[test]
fn ruby_unicode_escape_matcher_outcomes() {
    for (raw, literal, rule) in [
        (r#""the \uff21 panel""#, "the A panel", "normalized-matcher"),
        (
            r#""the \u{ff21} panel""#,
            "the A panel",
            "normalized-matcher",
        ),
        (
            r#""the \u{1f600 41} panel""#,
            "the 😀A panel",
            "duplicate-matcher",
        ),
        (r#""the \u0041 panel""#, "the A panel", "duplicate-matcher"),
        (r#"'the \u0041 panel'"#, "the A panel", "none"),
        (r#""the \\u0041 panel""#, "the A panel", "none"),
    ] {
        let source = format!("Given({raw}) {{ first() }}; Then('{literal}') {{ second() }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), 2, false, &source);
        for candidate in ["normalized-matcher", "duplicate-matcher"] {
            assert_eq!(
                rows.as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["rule"] == candidate),
                rule == candidate,
                "{source}"
            );
        }
    }
    for raw in [
        r#""\uD800""#,
        r#""\u{110000}""#,
        r#""\u123""#,
        r#""\u{41""#,
        r#""\u{zz}""#,
        r#""\u0041#{value}""#,
    ] {
        let (_, rows, _) = analyze(
            &format!("Given({raw}) {{ first() }}; Then('A') {{ second() }}"),
            &[],
        );
        assert_eq!(
            rows.as_array().unwrap().last().unwrap()["corpus"]["incomplete"],
            true
        );
        assert!(!rows
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["rule"] == "normalized-matcher" || r["rule"] == "duplicate-matcher"));
    }
}

/// Case folding must change usage without erasing the matcher flag boundary.
#[test]
fn ruby_ignorecase_literal_usage_and_uncertainty() {
    for (pattern, flags, step, complete, unused) in [
        ("a panel", "i", "A PANEL", true, 1),
        ("a panel", "i", "a panel", true, 0),
        ("a panel", "i", "unrelated", true, 2),
        ("^a panel$", "im", "A PANEL", true, 1),
        ("k", "i", "K", true, 1),
        ("s", "i", "ſ", true, 1),
        ("ss", "i", "ß", false, 1),
        ("fi", "i", "ﬁ", false, 1),
        ("FF", "i", "ﬀ", false, 1),
        ("fl", "i", "ﬂ", false, 1),
        ("st", "i", "ﬅ", false, 1),
        ("[s]s", "i", "ß", false, 1),
        ("s{2}", "i", "ß", false, 1),
        ("é", "i", "É", false, 1),
    ] {
        for literal in [
            format!("/{pattern}/{flags}"),
            format!("%r{{{pattern}}}{flags}"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("steps.rb"),
                format!("Given({literal}) {{ first() }}\nThen(/{pattern}/) {{ second() }}"),
            )
            .unwrap();
            fs::write(
                dir.path().join("test.feature"),
                format!("Feature: Case\n Scenario: Usage\n  Given {step}\n"),
            )
            .unwrap();
            let rows =
                records(run_project(dir.path(), "*.rb", &["--features", "*.feature"]).stdout);
            assert_discovery(&rows, 2, !complete, &literal);
            assert_eq!(
                rows.iter()
                    .any(|r| r["rule"] == "unused-definition" && r["primary"]["line"] == 1),
                complete && unused == 2,
                "{literal}"
            );
            assert_eq!(
                rows.iter()
                    .filter(|r| r["rule"] == "unused-definition")
                    .count(),
                unused,
                "{literal}"
            );
            assert!(
                !rows
                    .iter()
                    .any(|r| r["rule"] == "normalized-matcher" || r["rule"] == "duplicate-matcher"),
                "{literal}"
            );
        }
    }
}

/// Structural evidence abstracts only immediate literal arguments, preserving every other effect.
#[test]
fn ruby_parameterization_final_outcome_matrix() {
    for (left, right, expected) in [
        ("page.theme('dark')", "page.theme('light')", true),
        ("page.theme \"dark\"", "page.theme \"light\"", true),
        ("page.theme(1)", "page.theme(2)", true),
        ("page.theme(1.5)", "page.theme(2.5)", true),
        (
            "page.theme('dark'); page.log(1)",
            "page.theme('light'); page.log(1)",
            true,
        ),
        ("page.theme('dark')", "page.reset('light')", false),
        ("page.theme('dark')", "other.theme('light')", false),
        ("page.theme('dark')", "page&.theme('light')", false),
        ("page.theme('dark')", "page.theme('light', 1)", false),
        ("page.theme('dark')", "page.theme(2)", false),
        ("page.theme(1)", "page.theme(2.0)", false),
        (
            "page.theme('dark'); page.log(1)",
            "page.log(1); page.theme('light')",
            false,
        ),
        ("page.theme(true)", "page.theme(false)", false),
        (
            "|page| page.theme('dark')",
            "|page| page.theme('light')",
            false,
        ),
        (
            "page.theme('dark'); page.log",
            "page.theme('light'); page.log",
            true,
        ),
        (
            "page.theme('dark'); page.log()",
            "page.theme('light'); page.log()",
            true,
        ),
        ("page.theme(:dark)", "page.theme(:light)", false),
        (
            "page.theme('dark') { assert_equal(1, value) }",
            "page.theme('light') { assert_equal(2, value) }",
            false,
        ),
        (
            "page.theme('dark') rescue fallback",
            "page.theme('light') rescue fallback",
            false,
        ),
        ("page.theme(value)", "page.theme(other)", false),
        (
            "page.theme('dark' + value)",
            "page.theme('light' + value)",
            false,
        ),
        (
            "page.theme(\"dark#{value}\")",
            "page.theme(\"light#{value}\")",
            false,
        ),
        (
            "page.theme('dark') if ready",
            "page.theme('light') if ready",
            false,
        ),
        (
            "value = 'dark'; page.theme(value)",
            "value = 'light'; page.theme(value)",
            false,
        ),
        ("page.theme { 'dark' }", "page.theme { 'light' }", false),
        (
            "page.theme(-> { 'dark' })",
            "page.theme(-> { 'light' })",
            false,
        ),
        (
            "page.theme(compute('dark'))",
            "page.theme(compute('light'))",
            false,
        ),
        ("theme('dark')", "theme('light')", false),
        (
            "expect(page.theme).to eq('dark')",
            "expect(page.theme).to eq('light')",
            false,
        ),
        (
            "page.theme('dark'); assert_equal(1, value)",
            "page.theme('light'); assert_equal(2, value)",
            false,
        ),
        (
            "page.theme('dark'); binding",
            "page.theme('light'); binding",
            false,
        ),
    ] {
        for (open, close) in [("{", "}"), ("do", "end")] {
            let source = format!("Given('switches to dark theme') {open} {left}; {close}\nGiven('switches to light theme') {open} {right}; {close}");
            let (_, rows, _) = analyze(&source, &[]);
            let rows = rows.as_array().unwrap();
            assert_eq!(
                rows.last().unwrap()["summary"]["definitionsAnalyzed"],
                2,
                "{source}"
            );
            assert_parameterization_outcome(rows, expected, &source);
        }
    }
}

/// The same matcher contract applies to both adapters, including the previously missed opposite.
#[test]
fn parameterization_matcher_boundaries_are_shared() {
    for (left, right, expected) in [
        ("switches to dark theme", "switches to light theme", true),
        ("the panel is enabled", "the panel is disabled", false),
        ("enable the panel", "disable the panel", false),
        ("enables the panel", "disables the panel", false),
        ("enabling the panel", "disabling the panel", false),
        ("the panel is shown", "the panel is not shown", false),
        (
            "the AI chatbot toggle button should be visible",
            "the AI chatbot window should be open",
            false,
        ),
        ("opens a door", "calculates an invoice", false),
    ] {
        for (extension, source) in [
            ("rb", format!("Given('{left}') {{ page.theme('dark') }}\nGiven('{right}') {{ page.theme('light') }}")),
            ("ts", format!("import {{ Given }} from '@cucumber/cucumber';\nGiven('{left}', () => {{ page.theme('dark'); }});\nGiven('{right}', () => {{ page.theme('light'); }});")),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(format!("steps.{extension}")), &source).unwrap();
            let rows = records(run_project(dir.path(), &format!("*.{extension}"), &[]).stdout);
            assert_discovery(&rows, 2, false, &source);
            assert_parameterization_outcome(&rows, expected, &source);
        }
    }
}

#[test]
fn ruby_parameterization_respects_capture_context_and_suppression() {
    for (capture, extra, expected) in [
        ("", vec![], true),
        ("page = :captured", vec![], false),
        ("", vec!["--rule", "parameterization-candidate=off"], false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        for (file, value) in [("a.rb", "dark"), ("b.rb", "light")] {
            fs::write(
                dir.path().join(file),
                format!(
                    "{capture}\nGiven('switches to {value} theme') {{ page.theme('{value}') }}"
                ),
            )
            .unwrap();
        }
        let rows = records(run_project(dir.path(), "*.rb", &extra).stdout);
        assert_discovery(&rows, 2, false, capture);
        assert_parameterization_outcome(&rows, expected, &format!("{capture}: {extra:?}"));
    }
}

fn assert_parameterization_outcome(rows: &[Value], expected: bool, context: &str) {
    for (rule, required) in [
        ("parameterization-candidate", expected),
        ("duplicate-handler", false),
        ("near-duplicate-step", false),
    ] {
        assert_eq!(
            rows.iter().any(|r| r["rule"] == rule),
            required,
            "{context}: {rule}"
        );
    }
}

/// Native capture semantics and encoding identity must survive matcher normalization.
#[test]
fn ruby_native_regex_normalization_outcomes() {
    for (left, right, finding, incomplete) in [
        (r"/(?<first>\d+)/", r"/(?<second>\d+)/", true, false),
        (r"/(?'first'\d+)/", r"/(?<second>\d+)/", true, false),
        (r"/(?<first>a)(b)/", r"/(a)(?:b)/", true, false),
        (r"/(?<x>a)(b)(c)/", r"/(a)(?:b)(?:c)/", true, false),
        (r"/(a)(?<x>b)(c)/", r"/(?:a)(b)(?:c)/", true, false),
        (r"/é(a)(?<x>b)/", r"/é(?:a)(b)/", true, false),
        (r"/a\ b/", r"/a b/", true, false),
        (r"/(?<first>a)(b)/", r"/(a)(b)/", false, false),
        (r"/(?<first>a)/", r"/(?<second>a)(c)/", false, false),
        (r"/\Aa\z/", r"/a/", false, false),
        (r"/a+/", r"/a/", false, false),
        (r"/a/u", r"/a/n", false, false),
        (r"/a/n", r"/a/", false, false),
        (r"/[(?<a>)]/", r"/[(?<b>)]/", false, false),
        (r"/[a[b](?<x>)]/", r"/[a[b](?<y>)]/", false, false),
        (r"/a\\ b/", r"/a b/", false, false),
        (r"/(?<x>a)\k<x>/", r"/(?<y>a)\k<y>/", false, true),
        (r"/(?<x>a)(?<x>b)/", r"/(?<y>a)(?<z>b)/", false, true),
    ] {
        let source = format!("Given({left}) {{ first() }}; Then({right}) {{ second() }}");
        let (_, rows, _) = analyze(&source, &[]);
        let rows = rows.as_array().unwrap();
        assert_discovery(rows, 2, incomplete, &source);
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "normalized-matcher"),
            finding,
            "{source}"
        );
        assert!(
            !rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
            "{source}"
        );
    }
}

#[test]
fn ruby_native_regex_usage_and_encoding_outcomes() {
    for (literal, step, complete, used) in [
        (r"/\Avalue \w+\z/u", "value abc", true, true),
        (r"/\Avalue \w+\z/u", "value 日本", true, false),
        (r"/\Avalue\z/n", "value", true, true),
        (r"/\Avalue\z/n", "other", true, false),
        (r"/\A.\z/n", "日", true, true),
        (r"/k/in", "K", true, true),
        (r"/日本/u", "日本", true, true),
        (r"/日本/n", "日本", false, false),
        (r"/value/e", "value", false, false),
        (r"/value/un", "value", false, false),
        (r"/\A(?<name>\d+)\z/", "123", true, true),
        (r"/\A(?'name'\d+)\z/", "１２３", true, false),
        (r"/\Aa\ b\z/", "a b", true, true),
        (r"/\Aa\ b\z/", "ab", true, false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("steps.rb"),
            format!("Given({literal}) {{ first() }}\nThen({literal}) {{ second() }}"),
        )
        .unwrap();
        fs::write(
            dir.path().join("test.feature"),
            format!("Feature: Native\n Scenario: Usage\n  Given {step}\n"),
        )
        .unwrap();
        let rows = records(run_project(dir.path(), "*.rb", &["--features", "*.feature"]).stdout);
        assert_discovery(&rows, 2, !complete, literal);
        assert!(
            rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
            "{literal}"
        );
        assert_eq!(
            rows.iter()
                .filter(|r| r["rule"] == "unused-definition")
                .count(),
            if complete && !used { 2 } else { 0 },
            "{literal}"
        );
    }
}

/// Writes each `(path, contents)` pair into a fresh temporary project root.
fn ruby_project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        ruby_write(directory.path(), path, contents);
    }
    directory
}

/// Runs the binary with JSONL output over `files` written to a temporary root, with the `*.rb`
/// definitions, matcher window `window` (an inherited override is removed when `None`) and `extra`
/// arguments; returns the records and stderr, surfacing exit code and stderr when no summary is
/// produced.
fn ruby_records_run(
    files: &[(&str, &str)],
    window: Option<usize>,
    extra: &[&str],
) -> (Vec<Value>, String) {
    let directory = ruby_project(files);
    let mut command = ruby_cli(directory.path(), "*.rb");
    match window {
        Some(size) => command.env("CUKE_DEDUP_MATCHER_WINDOW", size.to_string()),
        None => command.env_remove("CUKE_DEDUP_MATCHER_WINDOW"),
    };
    let output = command
        .args(["--reporters", "jsonl", "--no-metrics"])
        .args(extra)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let rows = records(output.stdout);
    assert!(
        rows.last().is_some_and(|row| row["type"] == "summary"),
        "exit {:?}: {stderr}",
        output.status.code()
    );
    (rows, stderr)
}

/// Runs `steps` as `steps.rb` over the `usage.feature` scenario body `feature` with `extra`
/// arguments; returns the records and stderr.
fn ruby_usage_run(steps: &str, feature: &str, extra: &[&str]) -> (Vec<Value>, String) {
    let feature = format!("Feature: Usage\n{feature}");
    let arguments = [&["--features", "usage.feature"], extra].concat();
    ruby_records_run(
        &[("steps.rb", steps), ("usage.feature", &feature)],
        None,
        &arguments,
    )
}

/// Renders each `rule` finding as `path:line` of its primary followed by its related locations,
/// sorted, so assertions pin which step and definitions a finding names.
fn ruby_rule_locations(rows: &[Value], rule: &str) -> Vec<String> {
    let location = |value: &Value| format!("{}:{}", value["path"].as_str().unwrap(), value["line"]);
    let mut found: Vec<String> = rows
        .iter()
        .filter(|row| row["rule"] == rule)
        .map(|row| {
            let related = row["related"].as_array().unwrap().iter().map(location);
            std::iter::once(location(&row["primary"]))
                .chain(related)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    found.sort();
    found
}

/// Ruby `steps.rb` definitions over a `usage.feature` scenario body: complete analysis, and
/// exactly the expected `ambiguous-step` findings (feature step, then matching definitions) and
/// `unused-definition` findings (definition).
#[test]
fn ruby_usage_and_ambiguity_outcomes_for_concrete_outline_and_expression_steps() {
    const NO_TYPES: &str = "# no parameter types declared";
    let outline = |rows: &str| {
        format!(" Scenario Outline: Rows\n  Given I have <count> items\n  Then the user is here\n  Examples:\n   | count |\n{rows}")
    };
    let fallback = |types: &str| {
        format!("{types}\nGiven('I have {{quantity}} cucumber(s)') {{ count() }}\nThen('they are in my belly/stomach with {{mood}}') {{ verify() }}")
    };
    let declared = "ParameterType(name: 'quantity', regexp: /several|one/, transformer: ->(s) { s }); ParameterType(name: 'mood', regexp: /joy|grief/, transformer: ->(s) { s })";
    let wait = |types: &str| {
        format!("{types}\nGiven('I wait {{duration}}') {{ wait_duration() }}\nGiven('I wait for the page') {{ wait_for_page() }}")
    };
    let duration = |pattern: &str| {
        format!("ParameterType(name: 'duration', regexp: /{pattern}/, transformer: ->(s) {{ s }})")
    };
    let concrete = "Given('a user named {word}') { make_user() }\nGiven(/^a user named .+$/) { make_other_user() }";
    let insensitive =
        "Given('I have {int} items') { count() }\nThen(/^THE USER IS HERE$/i) { verify() }";
    let sensitive =
        "Given('I have {int} items') { count() }\nThen(/^THE USER IS HERE$/) { verify() }";
    let expression = "Given('I have {int} cucumber(s)') { count() }\nThen('they are in my belly/stomach') { verify() }";
    let cases: Vec<(String, String, &[&str], &[&str])> = vec![
        // A concrete step matched by two definitions is one ambiguity and uses both.
        (concrete.into(), " Scenario: S\n  Given a user named Ada\n".into(), &["usage.feature:3 steps.rb:1 steps.rb:2"], &[]),
        (concrete.into(), " Scenario: S\n  Given a stranger named Ada\n".into(), &[], &["steps.rb:1", "steps.rb:2"]),
        // Expanded outline rows and a Ruby `/i` regex mark usage. The original wording `EXISTS`
        // contains `st`, which the documented Ruby `i` subset excludes (Unicode ligature folding).
        (insensitive.into(), outline("   | 2 |\n"), &[], &[]),
        (sensitive.into(), outline("   | 2 |\n"), &[], &["steps.rb:2"]),
        (insensitive.into(), outline("   | two |\n"), &[], &["steps.rb:1"]),
        // Optional `(s)` and alternative `belly/stomach` text in a plain Cucumber Expression.
        (expression.into(), " Scenario: S\n  Given I have 2 cucumbers\n  Then they are in my stomach\n".into(), &[], &[]),
        (expression.into(), " Scenario: S\n  Given I have 1 cucumber\n  Then they are in my belly\n".into(), &[], &[]),
        (expression.into(), " Scenario: S\n  Given I have 2 cucumberz\n  Then they are in my liver\n".into(), &[], &["steps.rb:1", "steps.rb:2"]),
        // Declared `ParameterType`s make the custom-parameter fallback exact while keeping its
        // optional and alternative text, so a step outside that text is unused.
        (fallback(declared), " Scenario: S\n  Given I have several cucumbers\n  Then they are in my stomach with joy\n".into(), &[], &[]),
        (fallback(declared), " Scenario: S\n  Given I have one cucumber\n  Then they are in my belly with grief\n".into(), &[], &[]),
        (fallback(declared), " Scenario: S\n  Given I have several cucumberz\n  Then they are in my liver with joy\n".into(), &[], &["steps.rb:2", "steps.rb:3"]),
        // An undeclared `{duration}` cannot prove ambiguity (its usage half is vacuous: every
        // non-exact matcher counts as used); a declared type that accepts the same steps proves
        // the ambiguity, and one that rejects them is unused.
        (wait(NO_TYPES), " Scenario: S\n  Given I wait for the page\n  Given I wait 3 seconds\n".into(), &[], &[]),
        (wait(&duration(r"for the page|\d+ seconds")), " Scenario: S\n  Given I wait for the page\n  Given I wait 3 seconds\n".into(), &["usage.feature:3 steps.rb:2 steps.rb:3"], &[]),
        (wait(&duration(r"\d+ minutes")), " Scenario: S\n  Given I wait for the page\n  Given I wait 3 seconds\n".into(), &[], &["steps.rb:2"]),
    ];
    for (steps, feature, ambiguous, unused) in cases {
        let (rows, stderr) = ruby_usage_run(&steps, &feature, &[]);
        assert_eq!(stderr, "", "{steps}\n{feature}");
        assert_eq!(
            rows.last().unwrap()["corpus"]["incomplete"],
            false,
            "{steps}"
        );
        assert_eq!(
            ruby_rule_locations(&rows, "ambiguous-step"),
            ambiguous,
            "{steps}\n{feature}"
        );
        assert_eq!(
            ruby_rule_locations(&rows, "unused-definition"),
            unused,
            "{steps}\n{feature}"
        );
    }
}

/// Ruby outline ambiguities are reported once per template step and match set: repeated rows with
/// one match set collapse, disjoint match sets stay separate, and concrete steps stay per line.
#[test]
fn ruby_outline_ambiguities_group_by_template_location_and_match_set() {
    let items =
        "Given('I have {int} items') { first() }\nGiven(/^I have \\d+ items$/) { second() }";
    let values = "Given('value {int}') { first() }\nGiven(/^value \\d+$/) { second() }\nGiven(/^value 1$/) { third() }";
    let outline = |step: &str, rows: &str| {
        format!(" Scenario Outline: Rows\n  Given {step}\n  Examples:\n   | count |\n{rows}")
    };
    let cases: [(&str, String, &[&str], usize); 5] = [
        (
            items,
            outline("I have <count> items", "   | 1 |\n   | 2 |\n   | 3 |\n"),
            &["usage.feature:3 steps.rb:1 steps.rb:2"],
            3,
        ),
        // Control: the same matches from two concrete lines are two findings.
        (
            items,
            " Scenario: S\n  Given I have 1 items\n  Given I have 2 items\n".into(),
            &[
                "usage.feature:3 steps.rb:1 steps.rb:2",
                "usage.feature:4 steps.rb:1 steps.rb:2",
            ],
            2,
        ),
        // Row 1 also matches `/^value 1$/`, so the template carries two match sets.
        (
            values,
            outline("value <count>", "   | 1 |\n   | 2 |\n"),
            &[
                "usage.feature:3 steps.rb:1 steps.rb:2",
                "usage.feature:3 steps.rb:1 steps.rb:2 steps.rb:3",
            ],
            2,
        ),
        (
            values,
            outline("value <count>", "   | 2 |\n   | 3 |\n"),
            &["usage.feature:3 steps.rb:1 steps.rb:2"],
            2,
        ),
        (
            values,
            outline("value <count>", "   | 1 |\n   | 1 |\n"),
            &["usage.feature:3 steps.rb:1 steps.rb:2 steps.rb:3"],
            2,
        ),
    ];
    for (steps, feature, ambiguous, feature_steps) in cases {
        let (rows, stderr) = ruby_usage_run(steps, &feature, &[]);
        assert_eq!(stderr, "", "{feature}");
        let summary = &rows.last().unwrap()["summary"];
        assert_eq!(summary["featureStepsAnalyzed"], feature_steps, "{feature}");
        assert_eq!(
            ruby_rule_locations(&rows, "ambiguous-step"),
            ambiguous,
            "{feature}"
        );
    }
}

/// The Ruby duplication summary counts only duplication rules: error-severity `ambiguous-step` and
/// `unused-definition` findings neither add definitions nor rules, and a suite with zero
/// definitions reports 0% and passes the default 0% threshold.
#[test]
fn ruby_duplication_threshold_excludes_correctness_and_usage_findings() {
    let steps = "Given('I click save') { page.click('save') }\nGiven('I click cancel') { page.click('cancel') }\nGiven('the cart is open') { open_cart() }\nWhen('the cart is open') { show_cart() }\nThen('a user named {word}') { make_user() }\nThen(/^a user named .+$/) { make_other_user() }\nThen('never used') { nothing() }";
    let feature = " Scenario: S\n  Given I click save\n  Given I click cancel\n  Given the cart is open\n  Then a user named Ada\n";
    let errors = [
        "--rule",
        "unused-definition=error",
        "--rule",
        "parameterization-candidate=error",
    ];
    let (rows, _) = ruby_usage_run(steps, feature, &errors);
    let mut contributions: Vec<_> = rows
        .iter()
        .filter(|row| row["type"] == "finding")
        .map(|row| {
            assert_eq!(row["severity"], "error", "{row}");
            let rule = row["rule"].as_str().unwrap();
            (
                rule.to_owned(),
                row["contributesToThreshold"].as_bool().unwrap(),
            )
        })
        .collect();
    contributions.sort();
    contributions.dedup();
    assert_eq!(
        contributions,
        [
            ("ambiguous-step".to_owned(), false),
            ("duplicate-matcher".to_owned(), true),
            ("parameterization-candidate".to_owned(), true),
            ("unused-definition".to_owned(), false),
        ]
    );
    // Lines 1-4 are duplicated; the ambiguity-only lines (5, 6) and the unused definition (7)
    // would raise 4 to 7. Keyword-agnostic matching makes lines 3 and 4 ambiguous too.
    for (rule, expected) in [
        ("duplicate-matcher", &["steps.rb:3 steps.rb:4"][..]),
        ("parameterization-candidate", &["steps.rb:1 steps.rb:2"]),
        (
            "ambiguous-step",
            &[
                "usage.feature:5 steps.rb:3 steps.rb:4",
                "usage.feature:6 steps.rb:5 steps.rb:6",
            ],
        ),
        ("unused-definition", &["steps.rb:7"]),
    ] {
        assert_eq!(ruby_rule_locations(&rows, rule), expected, "{rule}");
    }
    let duplication = &rows.last().unwrap()["summary"]["duplication"];
    ruby_assert_pointers(
        duplication,
        &[
            ("/duplicatedDefinitions", 4.into()),
            ("/totalDefinitions", 7.into()),
            ("/percentage", (4_f64 / 7_f64 * 100.0).into()),
            ("/passed", false.into()),
            (
                "/rules",
                serde_json::json!(["duplicate-matcher", "parameterization-candidate"]),
            ),
        ],
    );
    let (rows, stderr) = ruby_usage_run("# no step definitions\n", feature, &[]);
    assert!(stderr.contains("produced 0 definitions"), "{stderr}");
    assert_eq!(
        rows.last().unwrap()["summary"]["duplication"],
        serde_json::json!({"threshold": 0.0, "duplicatedDefinitions": 0, "totalDefinitions": 0, "percentage": 0.0, "passed": true, "rules": []})
    );
}

#[test]
fn ruby_native_regex_flag_order_is_not_identity() {
    for (left, right) in [
        ("/panel/im", "/panel/mi"),
        ("/panel/iu", "/panel/uii"),
        ("%r{panel}im", "/panel/mi"),
    ] {
        let source = format!("Given({left}) {{ first() }}; Then({right}) {{ second() }}");
        let (_, rows, _) = analyze(&source, &[]);
        let rows = rows.as_array().unwrap();
        assert_discovery(rows, 2, false, &source);
        assert!(
            rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
            "{source}"
        );
        assert!(
            !rows.iter().any(|r| r["rule"] == "normalized-matcher"),
            "{source}"
        );
    }
}

#[test]
fn frontend_finalization_outcomes_preserve_cross_file_registry_authority() {
    for effect in [
        "def Given(*args); end",
        "eval(code)",
        "module Local; def Given(*args); end; end\nextend Local",
    ] {
        for reversed in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let names = if reversed {
                ("z.rb", "a.rb")
            } else {
                ("a.rb", "z.rb")
            };
            fs::write(
                root.path().join(names.0),
                "Given('same') { verify() }; Then('same') { verify() }",
            )
            .unwrap();
            fs::write(root.path().join(names.1), effect).unwrap();
            let output = run_project(root.path(), "*.rb", &["--fail-on-incomplete"]);
            let rows = records(output.stdout);
            assert_discovery(&rows, 0, true, effect);
            assert_eq!(output.status.code(), Some(2));
            assert!(!rows.iter().any(|row| row["rule"] == "duplicate-matcher"));
        }
    }
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("steps.rb"),
        "Given('same') { verify() }; Then('same') { verify() }",
    )
    .unwrap();
    fs::write(
        root.path().join("helper.rb"),
        "module Local; def Given(*args); end; end",
    )
    .unwrap();
    let rows = project_records(root.path());
    assert_discovery(&rows, 2, false, "closed helper");
    assert!(rows.iter().any(|row| row["rule"] == "duplicate-matcher"));
}

const ASSERTION_PROVIDER: &str = "module Assertions; def self.expect(actual); actual; end; end";

fn assertion_project(provider: &str, source: &str, trusted: bool) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("assertions.rb"), provider).unwrap();
    fs::write(
        root.path().join("steps.rb"),
        format!("require_relative 'assertions'\n{source}"),
    )
    .unwrap();
    fs::write(
        root.path().join(".cuke-dedup.json"),
        if trusted {
            r#"{"assertionModules":["./assertions"]}"#
        } else {
            "{}"
        },
    )
    .unwrap();
    root
}

/// Checks configured assertion contradictions and unconfigured ordinary-call controls.
#[test]
fn ruby_configured_assertion_evidence_preserves_value_polarity_and_ordinary_calls() {
    for trusted in [false, true] {
        for (left, right, parameterized) in [
            (
                "Assertions.expect(page).to_be('dark')",
                "Assertions.expect(page).to_be('light')",
                false,
            ),
            (
                "Assertions.expect(page).to_be('dark')",
                "Assertions.expect(other).to_be('dark')",
                false,
            ),
            (
                "Assertions.expect(page).to_be('dark')",
                "Assertions.expect(page).not.to_be('dark')",
                false,
            ),
            (
                "wrap { Assertions.expect(page).to_be('dark') }",
                "wrap { Assertions.expect(page).to_be('light') }",
                false,
            ),
            (
                "wrap { Assertions.expect(page).to_be('dark') }",
                "wrap { Assertions.expect(page).not.to_be('dark') }",
                false,
            ),
            ("store.write('dark')", "store.write('light')", true),
        ] {
            let source = format!(
                "Given('the theme is dark') {{ {left} }}; Then('the theme is light') {{ {right} }}"
            );
            let root = assertion_project(ASSERTION_PROVIDER, &source, trusted);
            let rows = records(run_project(root.path(), "steps.rb", &[]).stdout);
            assert_discovery(&rows, 2, false, &source);
            // Without trust the provider is an ordinary callable: a pair that differs only in a
            // literal, inline or inside a callback, is parameterization material; subjects
            // and polarity still separate the handlers.
            assert_parameterization_outcome(
                &rows,
                parameterized
                    || (!trusted
                        && left.contains("'dark'")
                        && left.replace("'dark'", "'light'") == right),
                &format!("trusted={trusted}: {source}"),
            );
        }
        for body in [
            "wrap { Assertions.expect(page).to_be('ready') }",
            "Assertions.expect(page).to_be_visible",
        ] {
            let source = format!("Given('one') {{ {body} }}; Then('two') {{ {body} }}");
            let root = assertion_project(ASSERTION_PROVIDER, &source, trusted);
            let rows = project_records(root.path());
            assert_discovery(&rows, 2, false, body);
            assert!(rows.iter().any(|row| row["rule"] == "duplicate-handler"));
        }
        for (expected, incomplete) in [("'ready'", false), ("UNKNOWN", trusted)] {
            let source = format!("Given('one') {{ Assertions.expect(page).to_be({expected}) }}; Then('two') {{ Assertions.expect(page).to_be({expected}) }}");
            let root = assertion_project(ASSERTION_PROVIDER, &source, trusted);
            let rows = project_records(root.path());
            assert_discovery(&rows, 2, false, &source);
            assert_eq!(
                rows.iter().any(|row| row["rule"] == "duplicate-handler"),
                !incomplete,
                "trusted={trusted}: {source}"
            );
        }
    }
}

/// Checks provider trust across imports, mutations, and source ownership.
#[test]
fn ruby_assertion_provider_authority_matrix() {
    for (provider, before, after, trusted) in [
        (ASSERTION_PROVIDER, "", "", true),
        (ASSERTION_PROVIDER, "Alias = Assertions", "", true),
        (ASSERTION_PROVIDER, "", "Alias = Assertions", true),
        (ASSERTION_PROVIDER, "module Assertions; end", "", false),
        (ASSERTION_PROVIDER, "mod.define_singleton_method(:expect) { other }", "", false),
        (ASSERTION_PROVIDER, "mod.alias_method(:expect, :other)", "", false),
        (ASSERTION_PROVIDER, "mod.remove_method(:expect)", "", false),
        ("module Assertions; def self.expect(actual); actual; end; alias_method :expect, :other; end", "", "", false),
        ("module Assertions; if active; def self.expect(actual); actual; end; end; end", "", "", false),
        ("module Assertions; class Nested; def self.expect(actual); actual; end; end; end", "", "", false),
        ("module Assertions; def self.expect(actual); def self.expect(value); value; end; actual; end; end", "", "", false),
        ("module Assertions; def self.expect(actual); self.define_singleton_method(:expect) { other }; actual; end; end", "", "", false),
        ("module Assertions; def self.expect(actual); self.send(:alias_method, :expect, :other); actual; end; end", "", "", false),
    ] {
        let source = format!("{before}\nGiven('one') {{ Assertions.expect(page).to_be(UNKNOWN) }}; Then('two') {{ Assertions.expect(page).to_be(UNKNOWN) }}\n{after}");
        let root = assertion_project(provider, &source, true);
        let rows = project_records(root.path());
        assert_discovery(&rows, 2, provider.contains("alias_method"), &format!("{provider}: {source}"));
        assert_eq!(rows.iter().any(|row| row["rule"] == "duplicate-handler"), !trusted, "{provider}: {source}");
    }
    for body in [
        "Assertions.expect.to_be('ready')",
        "Assertions.expect(page, options).to_be('ready')",
        "Assertions.expect(page).to_be()",
        "Assertions.expect(page).to_be('ready', 'extra')",
        "Assertions.expect(page).to_be_visible('extra')",
        "wrap { |value| Assertions.expect(value).to_be('ready') }",
        "Assertions.expect(page).to_be(*values)",
        "Assertions.expect(page).to_be(@expected)",
        "Assertions.expect(page).to_be($expected)",
        "Assertions.expect(page).to_be(@@expected)",
        "Assertions.expect(page).to_be(self)",
        "Assertions.expect(page).to_be(1 + 2)",
        "Assertions.expect(page).to_be([1, @expected])",
    ] {
        let source = format!("Given('one') {{ {body} }}; Then('two') {{ {body} }}");
        let root = assertion_project(ASSERTION_PROVIDER, &source, true);
        let rows = project_records(root.path());
        assert_discovery(&rows, 2, false, body);
        assert!(
            !rows.iter().any(|row| row["rule"] == "duplicate-handler"),
            "{body}"
        );
    }
    let source = "Given('one') { Assertions.expect(page).to_be(UNKNOWN) }; Then('two') { Assertions.expect(page).to_be(UNKNOWN) }\nrequire_relative 'assertions'";
    let root = assertion_project(ASSERTION_PROVIDER, source, true);
    fs::write(root.path().join("steps.rb"), source).unwrap();
    let rows = project_records(root.path());
    assert_discovery(&rows, 2, false, "load after registrations");
    assert!(rows.iter().any(|row| row["rule"] == "duplicate-handler"));
    for name in ["Object", "Kernel", "Module", "Cucumber"] {
        let provider = ASSERTION_PROVIDER.replace("Assertions", name);
        let source = format!("Given('one') {{ {name}.expect(page).to_be(UNKNOWN) }}; Then('two') {{ {name}.expect(page).to_be(UNKNOWN) }}");
        let root = assertion_project(&provider, &source, true);
        let rows = project_records(root.path());
        assert_discovery(&rows, 2, false, name);
        assert!(
            rows.iter().any(|row| row["rule"] == "duplicate-handler"),
            "{name}"
        );
    }
}

/// Checks assertion values across literal syntax and ignored comments.
#[test]
fn ruby_assertion_literal_and_comment_matrix() {
    for matcher in ["to_be", "to_equal", "equal_to", "to_have_class"] {
        for (expected, incomplete) in [
            ("'ready'", false),
            ("1", false),
            ("1.5", false),
            ("true", false),
            ("false", false),
            ("nil", false),
            (":ready", false),
            ("[1, 'ready']", false),
            ("{ready: [1, true]}", false),
            ("@expected", true),
            ("$expected", true),
            ("@@expected", true),
            ("self", true),
            ("[1, UNKNOWN]", true),
            ("1 + 2", true),
        ] {
            let body = format!("Assertions.expect(page).{matcher}({expected})");
            let source = format!("Given('one') {{ {body} }}; Then('two') {{ {body} }}");
            let root = assertion_project(ASSERTION_PROVIDER, &source, true);
            let rows = project_records(root.path());
            assert_discovery(&rows, 2, false, &body);
            assert_eq!(
                rows.iter().any(|r| r["rule"] == "duplicate-handler"),
                !incomplete,
                "{body}"
            );
        }
    }
    let body = "Assertions.expect(
# subject
page).to_be(
# expected
'ready')";
    let source = format!("Given('one') {{ {body} }}; Then('two') {{ {body} }}");
    let root = assertion_project(ASSERTION_PROVIDER, &source, true);
    let rows = project_records(root.path());
    assert_discovery(&rows, 2, false, body);
    assert!(rows.iter().any(|r| r["rule"] == "duplicate-handler"));
}

#[test]
fn ruby_unavailable_assertion_proof_preserves_independent_handlers() {
    for support in [
        "def broken(",
        "require_relative 'loop'
",
        "",
    ] {
        let root = assertion_project(
            ASSERTION_PROVIDER,
            "Given('same') { work() }; Then('same') { work() }; Then('other') { work() }",
            true,
        );
        fs::write(root.path().join("loop.rb"), support).unwrap();
        let output = run_project(root.path(), "*.rb", &["--fail-on-incomplete"]);
        let rows = records(output.stdout);
        assert_discovery(&rows, 3, !support.is_empty(), support);
        assert!(
            rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
            "{support}"
        );
        assert!(
            rows.iter().any(|r| r["rule"] == "duplicate-handler"),
            "{support}"
        );
        assert_eq!(
            output.status.code(),
            Some(if support.is_empty() { 1 } else { 2 })
        );
    }
}

/// Checks that only relevant preceding loads grant assertion authority.
#[test]
fn ruby_assertion_proof_relevance_matches_the_load_contract() {
    for (load, configured, cycle, incomplete) in [
        (
            "require_relative 'assertions'",
            "./assertions",
            false,
            false,
        ),
        ("require './assertions'", "./assertions", false, false),
        (
            "require_relative 'assertions'",
            "./ts-fixtures",
            true,
            false,
        ),
        ("require_relative 'assertions'", "./assertions", true, true),
    ] {
        let source = format!("{load}\nGiven('one') {{ Assertions.expect(page).to_be(UNKNOWN) }}; Then('two') {{ Assertions.expect(page).to_be(UNKNOWN) }}");
        let root = assertion_project(ASSERTION_PROVIDER, &source, true);
        fs::write(root.path().join("steps.rb"), &source).unwrap();
        fs::write(
            root.path().join(".cuke-dedup.json"),
            format!(r#"{{"assertionModules":["{configured}"]}}"#),
        )
        .unwrap();
        fs::write(
            root.path().join("loop.rb"),
            if cycle { "require_relative 'loop'" } else { "" },
        )
        .unwrap();
        let output = run_project(root.path(), "*.rb", &["--fail-on-incomplete"]);
        let rows = records(output.stdout);
        assert_discovery(&rows, 2, incomplete, &source);
        let trusted = configured == "./assertions" && !cycle;
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "duplicate-handler"),
            !trusted,
            "{source}: cycle={cycle}"
        );
        assert_eq!(
            output.status.code(),
            Some(if incomplete {
                2
            } else if trusted {
                0
            } else {
                1
            })
        );
    }
}

#[test]
fn ruby_reflective_provider_access_removes_assertion_authority() {
    for (effect, deferred_incomplete) in [
        ("Object.const_get(:Assertions)", false),
        ("Object.const_get('Assertions')", false),
        ("Object.const_get(name)", false),
        ("Object.const_set(:Assertions, Other)", false),
        ("Object.send(:remove_const, :Assertions)", true),
        ("Object.public_send(:const_get, 'Assertions')", true),
        ("Object.send(:send, :const_get, :Assertions)", true),
    ] {
        for deferred in [false, true] {
            let mutation = if deferred {
                format!("Given('mutation') {{ {effect} }}")
            } else {
                effect.into()
            };
            let source = format!("Given('one') {{ Assertions.expect(page).to_be(UNKNOWN) }}; Then('two') {{ Assertions.expect(page).to_be(UNKNOWN) }}\n{mutation}");
            let root = assertion_project(ASSERTION_PROVIDER, &source, true);
            let rows = project_records(root.path());
            assert_discovery(
                &rows,
                if deferred { 3 } else { 0 },
                !deferred || deferred_incomplete,
                &source,
            );
            assert_eq!(
                rows.iter().any(|r| r["rule"] == "duplicate-handler"),
                deferred,
                "{source}"
            );
        }
    }
}

#[test]
fn ruby_unsupported_assertion_chains_preserve_ordinary_handler_findings() {
    for chain in ["not(1).", "other.", &"not.".repeat(17)] {
        let body = format!("Assertions.expect(page).{chain}to_be(UNKNOWN)");
        let source = format!("Given('one') {{ {body} }}; Then('two') {{ {body} }}");
        let root = assertion_project(ASSERTION_PROVIDER, &source, true);
        let rows = project_records(root.path());
        assert_discovery(&rows, 2, false, &source);
        assert!(
            rows.iter().any(|r| r["rule"] == "duplicate-handler"),
            "{chain}"
        );
    }
}

/// Checks that ordinary effects cannot hide conflicting assertions.
#[test]
fn ruby_complete_assertion_events_retain_effect_conflicts() {
    let assertion = "Assertions.expect(page).to_be('ready')";
    for trusted in [false, true] {
        for (left, right, score) in [
            (assertion.to_owned(), assertion.to_owned(), 1.0),
            (
                format!("store.write('one'); {assertion}"),
                format!("store.write('two'); {assertion}"),
                0.0,
            ),
            (
                format!("@state = 'one'; {assertion}"),
                format!("@state = 'two'; {assertion}"),
                0.0,
            ),
            (
                format!("if ready; {assertion}; end"),
                format!("unless ready; {assertion}; end"),
                0.0,
            ),
            // Under configured evidence `rescue` is a token-bound effect, not a fail-closed
            // construct: differing rescue bodies never reach equivalence in either regime.
            (
                format!("begin; store.save; rescue; recover(); end; {assertion}"),
                format!("begin; store.save; rescue; audit(); end; {assertion}"),
                0.0,
            ),
            (
                format!("wrap {{ {assertion} }}; store.write('one')"),
                format!("wrap {{ {assertion} }}; store.write('two')"),
                0.0,
            ),
            (
                "Assertions.expect(page) { first() }.to_be('ready')".into(),
                "Assertions.expect(page) { second() }.to_be('ready')".into(),
                0.0,
            ),
            (
                assertion.into(),
                "Assertions&.expect(page).to_be('ready')".into(),
                0.0,
            ),
            (
                "Assertions.expect(page).not.to_be('ready')".into(),
                "Assertions.expect(page)&.not.to_be('ready')".into(),
                0.0,
            ),
            (
                assertion.into(),
                "Assertions.expect(page)&.to_be('ready')".into(),
                0.0,
            ),
        ] {
            let source = format!("Given('the badge is ready') {{ {left} }}; Then('the badge is  ready') {{ {right} }}");
            let root = assertion_project(ASSERTION_PROVIDER, &source, trusted);
            let rows = project_records(root.path());
            assert_discovery(&rows, 2, false, &source);
            let finding = rows
                .iter()
                .find(|row| row["rule"] == "normalized-matcher")
                .expect("matcher finding is retained");
            // Trusted handlers carry the complete-syntax stream, untrusted ones the action
            // stream; under either, conflicting operations must stay below full equivalence.
            let measured = finding["evidence"]["handlerSimilarity"].as_f64().unwrap();
            if score == 0.0 {
                assert!(
                    measured < 1.0,
                    "conflicting operations gained equivalence: trusted={trusted}: {source}"
                );
            } else {
                assert_eq!(measured, score, "trusted={trusted}: {source}");
            }
            assert!(
                !rows.iter().any(|row| row["rule"] == "duplicate-handler"
                    || row["rule"] == "near-duplicate-step"
                    || row["rule"] == "parameterization-candidate"),
                "{source}"
            );
        }
        for (body, incomplete) in [
            (assertion, false),
            ("Assertions.expect(page) { work() }.to_be('ready')", false),
            ("Assertions&.expect(page).to_be('ready')", false),
            ("Assertions.expect(page).to_be(UNKNOWN)", trusted),
        ] {
            let source = format!("Given('one') {{ {body} }}; Then('two') {{ {body} }}");
            let root = assertion_project(ASSERTION_PROVIDER, &source, trusted);
            let rows = project_records(root.path());
            assert_discovery(&rows, 2, false, &source);
            assert_handler_finding(
                &Value::Array(rows),
                !incomplete,
                &format!("trusted={trusted}: {source}"),
            );
        }
    }
    let root = assertion_project(ASSERTION_PROVIDER, "Given('the theme is dark') { store.write('dark') }; Then('the theme is light') { store.write('light') }", true);
    let rows = project_records(root.path());
    let finding = rows
        .iter()
        .find(|row| row["rule"] == "parameterization-candidate")
        .expect("whole-handler structural finding is retained");
    assert_eq!(finding["evidence"]["handlerSimilarity"], 0.95);
}

#[test]
fn ruby_local_namespace_ownership_keeps_registry_and_assertion_trust_separate() {
    for (provider, consumer, interference, positive) in [
        ("module Local; class Factory; def call(x); x; end; end; def self.api; Factory.new; end; end", "Local.api", "", true),
        ("module Local; def self.expect(x); x; end; end", "factory = Local.method(:expect); factory.call(1)", "", true),
        ("module Local; def self.expect(x); x; end; def self.api; method(:expect); end; end", "Local.api.call(1)", "", true),
        ("module Local; class Factory; def call(x); x; end; end; def self.api; Factory.new; end; end", "Local.api", "module Local; class Factory; def self.new; Object; end; end; end", false),
        ("module Local; class Factory; def call(x); x; end; end; def self.api; Factory.new; end; end", "expose(Local)", "", false),
        ("module Local; def self.expect(x); x; end; end", "Local.method(target)", "", false),
        ("module Local; def self.expect(x); x; end; end", "Local.method(:expect)", "Local = Object", false),
        ("module Local; def self.expect(x); x; end; end", "Local.method(:expect)", "module Local; def self.method(x); Object; end; end", false),
        ("module Local; def self.expect(x); Object.define_method(:Given) {}; end; end", "Local.expect(1)", "", false),
        ("module Local; class Factory < unknown; def call(x); x; end; end; def self.api; Factory.new; end; end", "Local.api", "", false),
    ] {
        for (provider_name, steps_name) in [("a.rb", "z.rb"), ("z.rb", "a.rb")] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(provider_name), provider).unwrap();
            fs::write(dir.path().join("interference.rb"), interference).unwrap();
            fs::write(dir.path().join(steps_name), format!("require_relative '{}'\n{consumer}\nGiven('same') {{ work() }}; Then('same') {{ work() }}; Given('different') {{ work() }}", provider_name.trim_end_matches(".rb"))).unwrap();
            let output = run_project(dir.path(), "*.rb", &[]);
            let rows = records(output.stdout);
            assert_discovery(&rows, if positive { 3 } else { 0 }, !positive, &format!("{provider} / {consumer} / {interference}"));
            assert_handler_finding(&Value::Array(rows.clone()), positive, consumer);
            assert_eq!(rows.iter().any(|r| r["rule"] == "duplicate-matcher"), positive);
        }
    }
}

#[test]
fn ruby_registration_aliases_use_valid_main_method_captures() {
    for (declaration, call, count) in [
        ("alias setup Given", "setup", 0),
        ("alias :setup :Given", "setup", 0),
        ("setup = method(:Given)", "setup.call", 3),
        ("SETUP = method(:Given)", "SETUP.call", 3),
    ] {
        let source = format!("{declaration}; {call}('same') {{ work() }}; {call}('same') {{ work() }}; {call}('other') {{ work() }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), count, count == 0, &source);
        assert_handler_finding(&rows, count > 0, &source);
    }
}

#[test]
fn ruby_unresolved_mutable_capture_never_gains_handler_trust_from_discovery() {
    for (capture, expected) in [
        ("captured = :fixed", true),
        ("captured = 42", true),
        ("captured = factory()", false),
        ("captured = api[runtime_key]", false),
        ("captured = original; captured.helper = replacement", false),
        ("captured = :fixed; captured = factory()", false),
        ("captured = 'fixed'; captured.replace(other)", false),
    ] {
        let source = format!("{capture}\nGiven('same') {{ use(captured) }}; Then('same') {{ use(captured) }}; Given('other') {{ use(captured) }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_eq!(
            rows.as_array().unwrap().last().unwrap()["summary"]["definitionsAnalyzed"],
            3
        );
        assert!(rows
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["rule"] == "duplicate-matcher"));
        assert_handler_finding(&rows, expected, capture);
    }
}

#[test]
fn ruby_export_alias_chains_and_namespace_forwarders_preserve_origin() {
    for (uses, interference, positive) in [
        ("register = Provider::GIVEN; renamed = register; namespace = Provider; ns_alias = namespace", "", true),
        ("register = Provider::GIVEN; renamed = register; namespace = Provider; ns_alias = namespace", "renamed = other", false),
        ("register = Provider::GIVEN; renamed = register; namespace = Provider; ns_alias = namespace", "expose(namespace)", false),
        ("register = Provider::GIVEN; renamed = register; namespace = Provider; ns_alias = namespace", "module Provider; def self.register(x, &b); end; end", false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("provider.rb"), "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; def self.register(text, &handler); GIVEN.call(text, &handler); end; end").unwrap();
        fs::write(dir.path().join("steps.rb"), format!("require_relative 'provider'\n{uses}\n{interference}\nrenamed.call('same') {{ work() }}; ns_alias.register('same') {{ work() }}; Provider.register('other') {{ work() }}")).unwrap();
        let rows = project_records(dir.path());
        assert_discovery(&rows, if positive { 3 } else { 0 }, !positive, uses);
        assert_handler_finding(&Value::Array(rows.clone()), positive, uses);
        assert_eq!(rows.iter().any(|r| r["rule"] == "duplicate-matcher"), positive);
    }
}

#[test]
fn ruby_provider_sibling_load_context_requires_every_caller_to_initialize_origin() {
    for (entry, other, positive) in [
        (
            "require_relative 'provider'; require_relative 'consumer'",
            "",
            true,
        ),
        (
            "require_relative 'consumer'; require_relative 'provider'",
            "",
            false,
        ),
        (
            "require_relative 'provider'; require_relative 'consumer'",
            "require_relative 'consumer'",
            false,
        ),
        (
            "require_relative 'provider'; require_relative 'consumer'",
            "require_relative 'provider'; require_relative 'consumer'",
            true,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        write_registration_provider_fixture(dir.path());
        fs::write(dir.path().join("entry.rb"), entry).unwrap();
        fs::write(dir.path().join("entry-other.rb"), other).unwrap();
        let output = run_project(dir.path(), "entry*.rb", &[]);
        let rows = records(output.stdout);
        assert_discovery(
            &rows,
            if positive { 2 } else { 0 },
            !positive,
            &format!("{entry} / {other}"),
        );
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
            positive
        );
    }
}

#[test]
fn ruby_named_handler_discovery_preserves_callable_identity_and_capture_timing() {
    for (declaration, handler, suffix, positive) in [
        ("handler = -> { work() }", "handler", "", true),
        ("handler = proc { work() }", "handler", "", true),
        ("def handler; work(); end", "method(:handler)", "", true),
        ("", "method(:handler)", "def handler; work(); end", false),
        ("handler = proc { work() }", "unknown", "", false),
        ("def handler; work(); end", "method", "", false),
        (
            "def handler; work(); end",
            "method(:handler, :other)",
            "",
            false,
        ),
        (
            "def handler; work(); end",
            "public_method(:handler)",
            "",
            false,
        ),
        (
            "handler = -> { work() }",
            "handler",
            "handler = replacement",
            false,
        ),
        (
            "handler = -> { work() }",
            "handler",
            "expose(handler)",
            false,
        ),
        (
            "def handler; work(); end",
            "method(runtime_name)",
            "",
            false,
        ),
        (
            "def handler; work(); end",
            "method(:handler)",
            "def handler; other(); end",
            false,
        ),
        (
            "def handler; work(); end",
            "method(:handler)",
            "undef handler",
            false,
        ),
        (
            "def handler; work(); end",
            "method(:handler)",
            "alias handler other",
            false,
        ),
        (
            "def handler; work(); end",
            "method(:handler)",
            "define_method('handler') { other() }",
            false,
        ),
        (
            "handler = -> { work() }",
            "handler",
            "class Proc; def to_proc; other(); end; end",
            false,
        ),
        (
            "def handler; work(); end",
            "method(:handler)",
            "class Method; def to_proc; other(); end; end",
            false,
        ),
        (
            "def handler; work(); end",
            "unknown.method(:handler)",
            "",
            false,
        ),
    ] {
        let source = format!("{declaration}\nGiven('same', &{handler}); Then('same', &{handler}); Given('other', &{handler})\n{suffix}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(
            rows.as_array().unwrap(),
            if positive { 3 } else { 0 },
            !positive,
            &source,
        );
        assert_handler_finding(&rows, positive, &source);
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .any(|r| r["rule"] == "duplicate-matcher"),
            positive
        );
    }
    let (_, scope_rows, _) = analyze("captured = factory(); def captured; :value; end; def left; use(captured); end; def right; use(captured); end; Given('first', &method(:left)); Then('second', &method(:right))", &[]);
    assert_discovery(
        scope_rows.as_array().unwrap(),
        2,
        false,
        "method local scope",
    );
    assert_handler_finding(&scope_rows, true, "method local scope");
    let (_, rows, _) = analyze("def left; work(:first); end; def right; work(:second); end; Given('same', &method(:left)); Then('same', &method(:right))", &[]);
    assert_discovery(rows.as_array().unwrap(), 2, false, "conflicting methods");
    assert_handler_finding(&rows, false, "conflicting methods");
}

/// Checks bound-handler discovery while preserving allocation and receiver identity.
#[test]
fn ruby_bound_instance_handlers_are_discovered_without_erasing_receiver_state() {
    for (class, constructor, definitions) in [
        ("class Bound; def initialize(value); @value = value; end; def handle; use(@value); end; end", "Bound.new(:first)", 3),
        ("class Bound; def handle; work(); end; end", "Bound.new", 3),
        ("class Bound; def handle; work(); end; end; class Bound; end", "Bound.new", 0),
        ("class Bound; alias other handle; def handle; work(); end; end", "Bound.new", 0),
        ("class Bound; def unrelated; end; alias other unrelated; def handle; work(); end; end", "Bound.new", 0),
        ("class Bound; def handle; work(); end; end; class Class; alias new allocate; end", "Bound.new", 0),
        ("class Bound; def self.new; unknown; end; def handle; work(); end; end", "Bound.new", 0),
        ("class Bound; def handle; work(); end; end; Bound.define_singleton_method(:new) { Object }", "Bound.new", 0),
        ("class Bound < unknown; def handle; work(); end; end", "Bound.new", 0),
        ("class Bound; def method(name); unknown; end; def handle; work(); end; end", "Bound.new", 0),
    ] {
        let source = format!("{class}\ninstance = {constructor}; Given('same', &instance.method(:handle)); Then('same', &instance.method(:handle)); Given('other', &instance.method(:handle))");
        let (_, rows, _) = analyze(&source, &[]);
        let comparable = definitions == 3 && !class.contains("@value");
        assert_discovery(rows.as_array().unwrap(), definitions, !comparable, &source);
        assert_handler_finding(&rows, comparable, &source);
        assert_eq!(rows.as_array().unwrap().iter().any(|r| r["rule"] == "duplicate-matcher"), definitions > 0);
    }
}

#[test]
fn ruby_closed_local_execution_discovers_steps_without_comparing_enclosing_captures() {
    for (declaration, invocation, count) in [
        ("factory = ->(value) { Given('same') { use(value) }; Then('same') { use(value) } }", "factory.call(1)", 2),
        ("factory = lambda do |; value|; value = 1; Given('same') { use(value) }; Then('same') { use(value) }; end", "factory.call()", 2),
        ("factory = proc { Given('same') { work() }; Then('same') { work() } }", "factory.call", 2),
        ("factory = ->(value) { Given('same') { use(value) } }", "factory.call()", 0),
        ("factory = ->(value) { Given('same') { use(value) } }", "factory.call(*values)", 0),
        ("factory = -> { Given('same') { work() } }", "", 0),
        ("factory = -> { Given('same') { work() } }", "factory.call; factory.call", 0),
        ("factory = -> { Given('same') { work() } }", "expose(factory); factory.call", 0),
        ("factory = -> { Given('same') { work() } }", "if ready; factory.call; end", 0),
        ("factory = -> { Given('same') { work() } }", "factory = replacement; factory.call", 0),
        ("factory = -> { effect(); Given('same') { work() } }", "factory.call", 0),
    ] {
        let source = format!("{declaration}\n{invocation}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), count, true, &source);
        assert_handler_finding(&rows, false, &source);
        assert_eq!(rows.as_array().unwrap().iter().any(|r| r["rule"] == "duplicate-matcher"), count > 1);
    }
}

#[test]
fn ruby_coverage_closed_execution_literal_and_parameter_boundaries() {
    for value in ["1.0", ":ready", "true", "false", "nil"] {
        let source = format!("factory = -> {{ value = {value}; Given('same') {{ work() }}; Then('same') {{ work() }} }}; factory.call");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), 2, true, &source);
        assert_handler_finding(&rows, false, &source);
        assert!(rows
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["rule"] == "duplicate-matcher"));
    }
    for declaration in ["->(value=1)", "->(*values)", "->(**values)"] {
        let source = format!("factory = {declaration} {{ Given('same') {{ work() }}; Then('same') {{ work() }} }}; factory.call");
        assert_resolved_handler_source(&source, 0);
    }
    for declaration in ["proc", "lambda"] {
        for first in ["Given('same', &unknown)", "Given('same')"] {
            let source = format!(
                "factory = {declaration} {{ {first}; Then('same') {{ work() }} }}; factory.call"
            );
            assert_resolved_handler_source(&source, 0);
        }
    }
}

#[test]
fn ruby_coverage_deferred_capture_writes_and_nested_declarations_are_uncertain() {
    for (prefix, body, trusted) in [
        ("value = 1", "read(value)", true),
        ("value = 1", "value += 1; read(value)", false),
        ("", "def helper; end; work()", false),
        ("", "class Other; end; work()", false),
    ] {
        let source = format!("{prefix}; Given('first') {{ {body} }}; Then('second') {{ {body} }}");
        assert_handler_outcome(&source, 2, trusted);
    }
}

/// Checks bound aliases against ambiguous or mutable receiver origins.
#[test]
fn ruby_coverage_instance_handler_aliases_require_closed_unique_receivers() {
    for (prefix, handler, expected) in [
        ("instance = Bound.new", "instance.method(:handle)", 2),
        (
            "instance = Bound.new; instance = Bound.new",
            "instance.method(:handle)",
            0,
        ),
        (
            "instance = Bound.new; expose(instance)",
            "instance.method(:handle)",
            0,
        ),
        ("", "Bound.method(:handle)", 0),
        ("instance = unknown", "instance.method(:handle)", 0),
    ] {
        let source = format!("class Bound; def handle; work(); end; end; {prefix}; Given('same', &{handler}); Then('same', &{handler})");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), expected, expected == 0, &source);
        assert_handler_finding(&rows, false, &source);
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .any(|r| r["rule"] == "duplicate-matcher"),
            expected == 2
        );
    }
}

#[test]
fn ruby_main_registration_capture_chains_do_not_trust_arbitrary_callables() {
    for (origin, suffix, expected) in [
        ("method(:Given)", "", true),
        ("method(:Given)", "renamed = other", false),
        ("method(:Given)", "expose(original)", false),
        ("other.method(:Given)", "", false),
        ("->(text, &block) { text }", "", false),
    ] {
        let source = format!("original = {origin}; renamed = original; {suffix}; renamed.call('same') {{ work() }}; renamed.call('same') {{ work() }}; renamed.call('other') {{ work() }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_eq!(
            rows.as_array().unwrap().last().unwrap()["summary"]["definitionsAnalyzed"],
            if expected { 3 } else { 0 }
        );
        assert_handler_finding(&rows, expected, &source);
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .any(|r| r["rule"] == "duplicate-matcher"),
            expected
        );
    }
}

#[test]
fn ruby_review_entry_sources_cannot_inherit_sibling_initialization() {
    let dir = tempfile::tempdir().unwrap();
    write_registration_provider_fixture(dir.path());
    fs::write(
        dir.path().join("entry.rb"),
        "require_relative 'provider'; require_relative 'consumer'",
    )
    .unwrap();
    for (pattern, count) in [("entry*.rb", 2), ("*.rb", 0)] {
        let rows = records(run_project(dir.path(), pattern, &[]).stdout);
        assert_discovery(&rows, count, count == 0, pattern);
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "duplicate-matcher"),
            count > 0
        );
    }
}

#[test]
fn ruby_review_forwarder_identity_rejects_all_replacement_routes() {
    for (replacement, count) in [
        ("", 3),
        ("Provider.define_singleton_method(:register) { |*args| }", 0),
        ("class << Provider; def register(*args); end; end", 0),
        ("expose(Provider)", 0),
        (
            "Provider.singleton_class.alias_method(:register, :other)",
            0,
        ),
    ] {
        let source = format!("ROOT_GIVEN = method(:Given); module Provider; def self.register(text, &handler); ROOT_GIVEN.call(text, &handler); end; end; {replacement}; Provider.register('same') {{ work() }}; Provider.register('same') {{ work() }}; Provider.register('other') {{ work() }}");
        assert_resolved_handler_source(&source, count);
    }
}

#[test]
fn ruby_review_closed_execution_rejects_symbol_handlers_and_extra_arguments() {
    for (registration, count) in [
        ("Given('same') { work() }; Then('same') { work() }", 2),
        (
            "Given('same', :helper) { work() }; Then('same', :helper) { work() }",
            0,
        ),
        ("Given('same', {}) { work() }", 0),
        ("Given('same', &unknown)", 0),
    ] {
        let source = format!("factory = -> {{ {registration} }}; factory.call");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), count, true, &source);
        assert_handler_finding(&rows, false, &source);
    }
}

#[test]
fn ruby_review_top_level_blocks_cannot_hide_capture_mutation() {
    for suffix in [
        "tap { captured = factory() }",
        "[1].each { captured = factory() }",
    ] {
        let source = format!("captured = :fixed; {suffix}; Given('first') {{ use(captured) }}; Then('second') {{ use(captured) }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), 2, true, &source);
        assert_handler_finding(&rows, false, &source);
    }
}

#[test]
fn ruby_review_branching_load_context_is_bounded_and_preserves_small_graphs() {
    for depth in [2, 26] {
        let dir = tempfile::tempdir().unwrap();
        write_registration_provider_fixture(dir.path());
        fs::write(
            dir.path().join("entry.rb"),
            "require_relative 'provider'; require_relative 'a0'; require_relative 'b0'",
        )
        .unwrap();
        for level in 0..depth {
            let source = if level + 1 == depth {
                "require_relative 'consumer'".to_owned()
            } else {
                format!(
                    "require_relative 'a{}'; require_relative 'b{}'",
                    level + 1,
                    level + 1
                )
            };
            for prefix in ["a", "b"] {
                fs::write(dir.path().join(format!("{prefix}{level}.rb")), &source).unwrap();
            }
        }
        let output = project_command(dir.path(), "entry.rb")
            .timeout(std::time::Duration::from_secs(30))
            .output()
            .unwrap();
        let rows = records(output.stdout);
        assert_discovery(&rows, 2, false, &format!("depth {depth}"));
        assert_eq!(
            rows.iter()
                .filter(|r| r["rule"] == "duplicate-matcher")
                .count(),
            1
        );
    }
}

#[test]
fn ruby_review_callable_kinds_preserve_arity_and_return_semantics() {
    for (left, right, equivalent) in [
        ("lambda { work() }", "lambda { work() }", true),
        ("lambda { work() }", "proc { work() }", false),
        ("lambda { return work() }", "proc { return work() }", false),
        (
            "lambda { |value| work(value) }",
            "proc { |value| work(value) }",
            false,
        ),
        ("-> { work() }", "proc { work() }", false),
    ] {
        let source = format!(
            "left = {left}; right = {right}; Given('first', &left); Then('second', &right)"
        );
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), 2, false, &source);
        assert_handler_finding(&rows, equivalent, &source);
        if !equivalent {
            assert!(
                !rows.as_array().unwrap().iter().any(|r| matches!(
                    r["rule"].as_str(),
                    Some("near-duplicate-step" | "parameterization-candidate")
                )),
                "{source}"
            );
        }
    }
}

#[test]
fn ruby_review_unrelated_conversion_methods_do_not_invalidate_registry() {
    for (class, count) in [
        (
            "class Ordinary; def to_proc; unknown; end; end; Ordinary.new",
            2,
        ),
        ("class Proc; def to_proc; unknown; end; end", 0),
        ("class Method; def to_proc; unknown; end; end", 0),
    ] {
        let source = format!("{class}; handler = proc {{ work() }}; Given('first', &handler); Then('second', &handler)");
        assert_resolved_handler_source(&source, count);
    }
}

#[test]
fn ruby_review_root_method_identity_includes_singleton_and_object_overrides() {
    for (replacement, count) in [
        ("", 2),
        ("def self.handler; other(); end", 0),
        ("class << self; def handler; other(); end; end", 0),
        ("class Object; def handler; other(); end; end", 0),
    ] {
        let source = format!("def handler; work(); end; {replacement}; Given('first', &method(:handler)); Then('second', &method(:handler))");
        assert_resolved_handler_source(&source, count);
    }
}

#[test]
fn ruby_review_exposed_conversion_owners_preserve_unrelated_registrations() {
    for (owner, receiver, count) in [
        ("Ordinary", "", 2),
        ("Ordinary", "self.", 2),
        ("Proc", "", 0),
        ("Method", "", 0),
        ("Ordinary", "Proc.", 0),
        ("Ordinary", "unknown.", 0),
    ] {
        let source = format!("class {owner}; self; def {receiver}to_proc; unknown; end; end; handler = proc {{ work() }}; Given('first', &handler); Then('second', &handler)");
        assert_resolved_handler_source(&source, count);
    }
}

#[test]
fn ruby_review_large_provider_suites_preserve_registration_recall() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("provider.rb"),
        "ROOT_GIVEN = method(:Given); module Provider; GIVEN = ::ROOT_GIVEN; end",
    )
    .unwrap();
    fs::write(dir.path().join(".cuke-dedup.json"), r#"{"rules":{"duplicate-handler":"off","near-duplicate-step":"off","parameterization-candidate":"off","unused-definition":"off"}}"#).unwrap();
    for index in 0..1000 {
        fs::write(dir.path().join(format!("steps{index}.rb")), format!("require_relative 'provider'; register = Provider::GIVEN; register.call('step {index}') {{ work({index}) }}")).unwrap();
    }
    let rows = project_records(dir.path());
    assert_discovery(&rows, 1000, false, "large provider suite");
}

/// Checks matching uncertainty behavior for named and inline assertion handlers.
#[test]
fn ruby_named_handler_assertion_uncertainty_matches_inline_outcomes() {
    for trusted in [false, true] {
        for expected in ["'ready'", "UNKNOWN"] {
            for form in ["inline", "proc", "lambda", "method"] {
                let mut source = String::new();
                for (index, matcher) in ["first", "second"].iter().enumerate() {
                    let body = format!("Assertions.expect(page).to_be({expected})");
                    source.push_str(&match form {
                        "inline" => format!("Then('{matcher}') {{ {body} }}; "),
                        "method" => format!("def handler{index}; {body}; end; Then('{matcher}', &method(:handler{index})); "),
                        _ => format!("handler{index} = {form} {{ {body} }}; Then('{matcher}', &handler{index}); "),
                    });
                }
                let root = assertion_project(ASSERTION_PROVIDER, &source, trusted);
                let rows = project_records(root.path());
                let uncertain = trusted && expected == "UNKNOWN";
                assert_discovery(&rows, 2, false, &format!("{trusted}/{form}/{expected}"));
                assert_handler_finding(&Value::Array(rows), !uncertain, &source);
            }
        }
    }
}

#[test]
fn ruby_named_handler_large_suites_charge_local_work_and_cache_method_identity() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("provider.rb"),
        "def shared_handler; work(); end",
    )
    .unwrap();
    fs::write(root.path().join(".cuke-dedup.json"), r#"{"rules":{"duplicate-handler":"off","duplicate-matcher":"off","near-duplicate-step":"off","parameterization-candidate":"off","unused-definition":"off"}}"#).unwrap();
    for index in 0..500 {
        let padding = "0;".repeat(500);
        fs::write(root.path().join(format!("steps{index}.rb")), format!("{padding}require_relative 'provider'; handler = proc {{ work() }}; Given('local {index} first', &handler); Then('local {index} second', &handler); Given('method {index} first', &method(:shared_handler)); Then('method {index} second', &method(:shared_handler))")).unwrap();
    }
    assert_discovery(
        &project_records(root.path()),
        2000,
        false,
        "local callable and shared Method scaling",
    );
}

/// Checks that named-method bindings use the defining source and scope.
#[test]
fn ruby_named_method_assertion_bindings_follow_the_body_source() {
    for expected in ["'ready'", "UNKNOWN"] {
        let root = assertion_project(ASSERTION_PROVIDER, "require_relative 'body'; Given('first', &method(:handler)); Then('second', &method(:handler))", true);
        fs::write(root.path().join("body.rb"), format!("require_relative 'assertions'; def handler; Assertions.expect(page).to_be({expected}); end")).unwrap();
        let rows = records(run_project(root.path(), "steps.rb", &[]).stdout);
        assert_discovery(&rows, 2, false, expected);
        assert_handler_finding(&Value::Array(rows), expected != "UNKNOWN", expected);
    }
}

#[test]
fn ruby_unresolved_loaders_cannot_grant_inherited_provider_initialization() {
    for loader in [
        "require_relative target",
        "def loader; require_relative 'consumer'; end",
        "if condition; require_relative 'consumer'; end",
        "load 'consumer.rb'",
        "autoload(:Consumer, 'consumer.rb')",
    ] {
        for direct in [false, true] {
            let root = tempfile::tempdir().unwrap();
            write_registration_provider_fixture(root.path());
            if direct {
                let path = root.path().join("consumer.rb");
                fs::write(
                    &path,
                    format!(
                        "require_relative 'provider'; {}",
                        fs::read_to_string(&path).unwrap()
                    ),
                )
                .unwrap();
            }
            fs::write(
                root.path().join("entry.rb"),
                format!("{loader}; require_relative 'provider'; require_relative 'consumer'"),
            )
            .unwrap();
            let rows = records(run_project(root.path(), "entry.rb", &[]).stdout);
            // Autoload independently invalidates the DSL; direct proof cannot override that uncertainty.
            assert_discovery(
                &rows,
                if direct && !loader.starts_with("autoload") {
                    2
                } else {
                    0
                },
                true,
                loader,
            );
        }
    }
}

/// Checks that unrelated handlers cannot authorize reflective assertion lookups.
#[test]
fn ruby_assertion_reflection_exemptions_do_not_depend_on_unrelated_handlers() {
    for capture in [
        "",
        "def helper; work(); end; Given('helper', &method(:helper));",
    ] {
        for expected in ["'ready'", "UNKNOWN"] {
            let source = format!("class Local; def safe; end; end; local = Local.new; local.send(:safe); {capture} Given('first') {{ Assertions.expect(page).to_be({expected}) }}; Then('second') {{ Assertions.expect(page).to_be({expected}) }}");
            let root = assertion_project(ASSERTION_PROVIDER, &source, true);
            let rows = project_records(root.path());
            assert_discovery(
                &rows,
                if capture.is_empty() { 2 } else { 3 },
                false,
                &source,
            );
            assert_handler_finding(&Value::Array(rows), expected != "UNKNOWN", &source);
        }
    }
}

#[test]
fn ruby_repeated_large_named_bodies_report_bounded_proof_exhaustion() {
    let root = tempfile::tempdir().unwrap();
    let body = "work();".repeat(2000);
    let registrations = (0..350)
        .map(|i| format!("Given('named {i}', &method(:handler));"))
        .collect::<String>();
    fs::write(root.path().join("steps.rb"), format!("def handler; {body} end; {registrations} Given('control') {{ control() }}; Then('control') {{ control() }}")).unwrap();
    fs::write(root.path().join(".cuke-dedup.json"), r#"{"rules":{"near-duplicate-step":"off","parameterization-candidate":"off","unused-definition":"off"}}"#).unwrap();
    let rows = project_records(root.path());
    assert_eq!(rows.last().unwrap()["corpus"]["incomplete"], true);
    assert_eq!(rows.last().unwrap()["summary"]["definitionsAnalyzed"], 0);
    assert_handler_finding(
        &Value::Array(rows),
        false,
        "exhausted named-body proofs cannot retain optional handler findings",
    );
}

#[test]
fn ruby_review_forwarder_helpers_preserve_independent_exports() {
    for usage in ["P.helper", "expose(P)"] {
        let source = format!("ROOT_GIVEN = method(:Given); module P; def self.register(text, &handler); ROOT_GIVEN.call(text, &handler); end; def self.helper; end; end; module Other; GIVEN = ::ROOT_GIVEN; end; {usage}; register = Other::GIVEN; register.call('first') {{ work() }}; register.call('second') {{ work() }}");
        let (_, rows, _) = analyze(&source, &[]);
        let trusted = usage == "P.helper";
        assert_discovery(
            rows.as_array().unwrap(),
            if trusted { 2 } else { 0 },
            !trusted,
            &source,
        );
        assert_handler_finding(&rows, trusted, &source);
    }
}

#[test]
fn ruby_review_capture_reads_keep_immutable_values_and_reject_mutation() {
    for (value, usage, trusted) in [
        ("1", "read(value)", true),
        ("1", "class Other; def work; end; end", true),
        ("1", "Before { read(value) }", true),
        ("1", "other = proc { read(value) }", true),
        ("1", "later { value = 2 }", false),
        ("'ready'", "Before { read(value) }", false),
    ] {
        let source = format!("value = {value}; {usage}; Given('first') {{ read(value) }}; Then('second') {{ read(value) }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), 2, !trusted, &source);
        assert_handler_finding(&rows, trusted, &source);
    }
}

#[test]
fn ruby_review_block_argument_escapes_remove_named_handler_proof() {
    for usage in ["", "expose(&handler)", "instance_exec(&handler)"] {
        let source = format!("handler = proc {{ work() }}; {usage}; Given('named first', &handler); Then('named second', &handler); Given('control first') {{ control() }}; Then('control second') {{ control() }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(
            rows.as_array().unwrap(),
            if usage.is_empty() { 4 } else { 2 },
            !usage.is_empty(),
            &source,
        );
        assert_handler_finding(&rows, true, &source);
    }
}

#[test]
fn ruby_coverage_forwarder_ownership_and_origin_boundaries() {
    for (declaration, trusted) in [
        ("module P; def self.register(text, &handler); ROOT.call(text, &handler); end; end", true),
        ("def self.register(text, &handler); ROOT.call(text, &handler); end", false),
        ("module P; def Other.register(text, &handler); ROOT.call(text, &handler); end; end", false),
        ("module Outer; module P; def self.register(text, &handler); ROOT.call(text, &handler); end; end; end", false),
        ("module P; class << self; end; def self.register(text, &handler); ROOT.call(text, &handler); end; end", false),
        ("module P; def Given; end; def self.register(text, &handler); ROOT.call(text, &handler); end; end", false),
        ("module P; def self.register(text, &handler); call(text, &handler); end; end", false),
        ("module P; def self.register(text, &handler); unknown.call(text, &handler); end; end", false),
        ("module P; def self.register(text, &handler); UNKNOWN.call(text, &handler); end; end", false),
        ("module P; def self.register(text, &handler); ROOT.call(text, &handler); end; end; module P; end", false),
    ] {
        let source = format!("ROOT = method(:Given); {declaration}; P.register('first') {{ work() }}; P.register('second') {{ work() }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), if trusted { 2 } else { 0 }, false, &source);
        assert_handler_finding(&rows, trusted, &source);
    }
}

#[test]
fn ruby_coverage_export_alias_uses_cannot_grant_unproved_registration_trust() {
    for (usage, trusted) in [
        ("", true),
        ("register.other", false),
        ("expose(register)", false),
        ("later { register.call('nested') {} }", false),
        ("renamed = register; renamed = unknown", false),
    ] {
        let source = format!("ROOT = method(:Given); module P; GIVEN = ::ROOT; end; register = P::GIVEN; {usage}; register.call('first') {{ work() }}; register.call('second') {{ work() }}");
        assert_handler_outcome(&source, if trusted { 2 } else { 0 }, trusted);
    }
}

#[test]
fn ruby_coverage_owner_rejection_preserves_direct_controls_when_safe() {
    for (prefix, count, incomplete) in [
        (
            "class Local; def safe; end; end; local = Local.new; local.safe",
            2,
            false,
        ),
        (
            "class Local; def safe; end; end; local = ::Local.new; local.safe",
            2,
            false,
        ),
        (
            "class Local < Parent; def safe; end; end; local = Local.new; local.safe",
            0,
            true,
        ),
        (
            "class Local; def send(name); end; end; local = Local.new; local.send(:safe)",
            2,
            true,
        ),
        (
            "class Object; def safe; end; end; local = Object.new; local.safe",
            2,
            false,
        ),
        (
            "class Local; def safe; end; end; Local = unknown; local = Local.new; local.safe",
            2,
            false,
        ),
        (
            "module Local; def self.safe; end; end; Local.method",
            0,
            true,
        ),
        (
            "module Outer::Inner; def self.safe; end; end; Outer::Inner.safe",
            0,
            true,
        ),
    ] {
        let source = format!("{prefix}; Given('first') {{ work() }}; Then('second') {{ work() }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), count, incomplete, &source);
        assert_handler_finding(&rows, count == 2, &source);
    }
}

#[test]
fn ruby_coverage_cross_file_handlers_preserve_source_capture_uncertainty() {
    for (body, comparable) in [("work()", true), ("use(__FILE__)", false)] {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join("provider.rb"),
            format!("def handler; {body}; end"),
        )
        .unwrap();
        fs::write(root.path().join("steps.rb"), "require_relative 'provider'; Given('first', &method(:handler)); Then('second', &method(:handler))").unwrap();
        let rows = project_records(root.path());
        assert_discovery(&rows, 2, !comparable, body);
        assert_handler_finding(&Value::Array(rows), comparable, body);
    }
}

#[test]
fn ruby_coverage_namespace_depth_does_not_remove_unrelated_direct_findings() {
    for prefix in [
        format!(
            "{} VALUE = 1; {}",
            "module Nested; ".repeat(66),
            "end; ".repeat(66)
        ),
        "module Outer; end; module Outer::Inner; VALUE = 1; end".to_owned(),
    ] {
        let source = format!("{prefix}; Given('first') {{ work() }}; Then('second') {{ work() }}");
        let (_, rows, _) = analyze(&source, &[]);
        assert_discovery(rows.as_array().unwrap(), 2, false, &source);
        assert_handler_finding(&rows, true, &source);
    }
}

#[test]
fn ruby_coverage_other_deferred_handlers_invalidate_captured_values() {
    for write in ["value = 2", "value += 1"] {
        let source = format!("value = 1; Given('writer') {{ {write} }}; Given('first') {{ read(value) }}; Then('second') {{ read(value) }}");
        assert_handler_outcome(&source, 3, false);
    }
}

// ===========================================================================================
// Ordinary action streams (K5.3): TypeScript-style call events for handlers without configured
// assertion evidence, the identical-stream near veto, untrusted `expect` policy, loader arity.
// ===========================================================================================

/// Runs a Ruby project with the near threshold the parity fixtures use and reports the handler
/// rules, corpus completeness and exit code.
fn action_rows(source: &str, extra: &[&str]) -> (Vec<String>, bool, i32) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join(".cuke-dedup.json"),
        r#"{"threshold":100,"nearDuplicateHandlerSimilarity":0.5,"rules":{"unused-definition":"off"}}"#,
    )
    .unwrap();
    fs::write(dir.path().join("steps.rb"), source).unwrap();
    let output = run_project(dir.path(), "*.rb", extra);
    let rows = records(output.stdout);
    let summary = rows.last().unwrap();
    let rules = common::active_handler_rules(&rows);
    (
        rules,
        summary["corpus"]["incomplete"].as_bool().unwrap(),
        output.status.code().unwrap(),
    )
}

/// Two near-worded registrations with the given handler bodies.
fn action_pair(first: &str, second: &str) -> String {
    format!("Given('the parcel status is verified') {{ {first} }}\nGiven('the parcel status is now verified') {{ {second} }}\n")
}

/// Asserts the handler rules of one near-worded pair and that the corpus stays complete.
fn check_action_pair(label: &str, first: &str, second: &str, expected: &[&str]) {
    let (rules, incomplete, _) = action_rows(&action_pair(first, second), &[]);
    assert_eq!(rules, expected, "{label}");
    assert!(!incomplete, "{label}");
}

const DUP: &[&str] = &["duplicate-handler", "near-duplicate-step"];
const NEAR: &[&str] = &["near-duplicate-step"];
const PARAM: &[&str] = &["parameterization-candidate"];
const NONE: &[&str] = &[];

/// Action events name the receiver-chain root and method; wording findings need ordered event
/// overlap and at least one differing event.
#[test]
fn action_streams_compare_ordinary_handlers_like_the_typescript_frontend() {
    for (label, first, second, expected) in [
        // A1: a fluent chain collapses to its root, so the direct and fluent click overlap 50%.
        (
            "direct and fluent action",
            "page.click('#save')",
            "page.locator('#save').click()",
            NEAR,
        ),
        (
            "different actions",
            "page.click('#save')",
            "page.fill('#save')",
            NONE,
        ),
        // A2: an extra call keeps the pair near until overlap drops below the threshold.
        (
            "extra wait",
            "page.goto('/login')",
            "page.goto('/login'); page.wait_for_load_state()",
            NEAR,
        ),
        (
            "three extra calls",
            "page.goto('/login')",
            "page.goto('/login'); warm(); sync(); flush()",
            NONE,
        ),
        // A3: identical callback bodies under different wrappers; different bodies under one.
        (
            "callback bodies match",
            "alpha { load(); render(); assert_rows() }",
            "beta { load(); render(); assert_rows() }",
            NEAR,
        ),
        (
            "callback bodies differ",
            "with_transaction { load_cart(); apply_discount(); save() }",
            "with_transaction { load_order(); apply_shipping(); commit() }",
            NONE,
        ),
        // A4: a bare helper read is a call, so differing helpers are one differing event.
        (
            "bare helper arguments",
            "click_button(deep_one)",
            "click_button(deep_two)",
            NEAR,
        ),
        (
            "identical bare helpers",
            "click_button(deep_one)",
            "click_button(deep_one)",
            DUP,
        ),
        // A6: a receiverless root call keeps its name as the base.
        (
            "different root calls",
            "helper('x').run",
            "other('x').run",
            NONE,
        ),
        // The root call records no event of its own, so both spellings are one sequence.
        (
            "receiverless root call with parentheses",
            "helper.run",
            "helper().run; audit()",
            NEAR,
        ),
        (
            "same root call plus flush",
            "helper(x).run",
            "helper(x).run; helper(x).flush",
            NEAR,
        ),
        // A7: ordered overlap below, at and above one half.
        (
            "one of three",
            "warm(); sync(); flush()",
            "warm(); purge(); seal()",
            NONE,
        ),
        ("one of two", "warm(); sync()", "warm(); flush()", NEAR),
        (
            "two of three",
            "warm(); sync(); flush()",
            "warm(); sync(); purge()",
            NEAR,
        ),
        // A8: order and repetition count through the ordered subsequence.
        ("reordered", "warm(); sync()", "sync(); warm()", NEAR),
        ("repeated", "warm(); warm()", "warm()", NEAR),
        // A9: control flow alone has no call anchor and identical streams are not near.
        (
            "control flow only",
            "if @flag\n  @x = 1\nend",
            "if @flag\n  @x = 2\nend",
            NONE,
        ),
        // B1/B2/B4: identical streams behind different syntax are never near.
        (
            "symbol values",
            "write_status(:ready)",
            "write_status(:rejected)",
            NONE,
        ),
        (
            "identical symbol values",
            "write_status(:ready)",
            "write_status(:ready)",
            DUP,
        ),
        (
            "local literal values",
            "value = 'ready'; store(value)",
            "value = 'idle'; store(value)",
            NONE,
        ),
        (
            "next keyword only",
            "next page.click()",
            "page.click()",
            NONE,
        ),
        // B3: straight-line literal differences stay parameterization material.
        (
            "straight-line literals",
            "thing.run('a')",
            "thing.run('b')",
            PARAM,
        ),
        // B5: a conflicting value beside an extra call overlaps 75%, as in TypeScript.
        (
            "conflict beside an extra call",
            "prepare(); write(:ready); save()",
            "prepare(); write(:rejected); save(); audit()",
            NEAR,
        ),
        // Parentheses are transparent for receivers and roots; comments inside them do not count.
        (
            "parenthesized receivers",
            "(page).click('#save')",
            "(page).fill('#save')",
            NONE,
        ),
        (
            "commented parenthesized root",
            "(page).click()",
            "(page # root\n).click()",
            DUP,
        ),
        // A statement list in parentheses is not transparent: its tokens are the chain base.
        (
            "multi-statement parenthesized receiver",
            "(warm; page).click",
            "(warm; page).click; audit()",
            NEAR,
        ),
        (
            "differing multi-statement parenthesized receiver",
            "(warm; page).click",
            "(cool; page).click; audit()",
            NONE,
        ),
        // Modeled control-flow containers and modifier forms.
        (
            "case statement",
            "case mode\nwhen 1 then warm()\nend",
            "case mode\nwhen 1 then warm()\nend\nsync()",
            NEAR,
        ),
        (
            "if modifier",
            "warm() if ready",
            "warm() if ready; sync()",
            NEAR,
        ),
        (
            "while with do",
            "while busy do warm() end",
            "while busy do warm() end; sync()",
            NEAR,
        ),
        // A callable literal is a callback only when handed to a call.
        (
            "unpassed lambda container",
            "[-> { warm(); sync() }]; first()",
            "[-> { warm(); sync() }]; second()",
            NONE,
        ),
        // Different wrappers: NEAR holds only while the shared callback bodies are recorded.
        (
            "callback inside an argument array",
            "run([-> { warm(); sync() }])",
            "other([-> { warm(); sync() }]); flush()",
            NEAR,
        ),
        (
            "callback inside an argument hash",
            "run(cb: proc { warm(); sync() })",
            "other(cb: proc { warm(); sync() }); flush()",
            NEAR,
        ),
        // Order and repetition count: sorted or deduplicated streams would make these near.
        (
            "reversed three",
            "warm(); sync(); flush()",
            "flush(); sync(); warm()",
            NONE,
        ),
        (
            "repetition weighted",
            "warm(); warm(); warm()",
            "warm()",
            NONE,
        ),
        (
            "differing control flow only",
            "if @a\n  @x = 1\nend",
            "while @b\n  @x = 1\nend",
            NONE,
        ),
        // C1/C3: untrusted expect chains keep the exact-handler policy.
        (
            "bare expect polarity",
            "expect.to eq(1)",
            "expect.not_to eq(1)",
            NONE,
        ),
        (
            "parenthesized expect polarity",
            "(expect(x)).to eq(1)",
            "(expect(x)).not_to eq(1)",
            NONE,
        ),
        (
            "untrusted polarity",
            "expect(x).to eq(1)",
            "expect(x).not_to eq(1)",
            NONE,
        ),
        (
            "identical untrusted expectation",
            "expect(x).to eq(1)",
            "expect(x).to eq(1)",
            DUP,
        ),
        (
            "untrusted values",
            "expect(parcel).to eq('ready')",
            "expect(parcel).to eq('idle')",
            NONE,
        ),
        (
            "ordinary helper named expect",
            "expect(x).run()",
            "expect(x).run(); flush()",
            NONE,
        ),
        (
            "block expectation",
            "expect { risky() }.to raise_error",
            "expect { risky() }.not_to raise_error",
            NONE,
        ),
        // U1: unmodeled constructs beside shared actions keep the exact policy.
        (
            "parameterized block",
            "items.each { |i| store(i) }",
            "items.each { |i| store(i) }; audit()",
            NONE,
        ),
        (
            "identical parameterized block",
            "items.each { |i| store(i) }",
            "items.each { |i| store(i) }",
            DUP,
        ),
        // Call-form callables carry their parameters on the block, not on the call.
        (
            "parameterized lambda callback",
            "run(lambda { |i| store(i) })",
            "run(lambda { |i| store(i) }); audit()",
            NONE,
        ),
        (
            "parameterized proc callback",
            "run(proc do |i| store(i) end)",
            "run(proc do |i| store(i) end); audit()",
            NONE,
        ),
        (
            "parameterized stabby callback",
            "run(->(i) { store(i) })",
            "run(->(i) { store(i) }); audit()",
            NONE,
        ),
        (
            "element reference",
            "store(rows[0])",
            "store(rows[0]); audit()",
            NONE,
        ),
        ("operator", "store(a + b)", "store(a + b); audit()", NONE),
        ("super", "super; store()", "super; store(); audit()", NONE),
        ("yield", "yield; store()", "yield; store(); audit()", NONE),
        (
            "interpolation",
            "store(\"#{x}\")",
            "store(\"#{x}\"); audit()",
            NONE,
        ),
        ("splat", "store(*args)", "store(*args); audit()", NONE),
        (
            "rescue",
            "begin; store(); rescue; recover(); end",
            "begin; store(); rescue; recover(); end; audit()",
            NONE,
        ),
    ] {
        check_action_pair(label, first, second, expected);
    }
}

/// `def` handlers and declared lambdas feed the action stream without naming themselves.
#[test]
fn action_streams_cover_method_handlers_and_declared_callables() {
    let methods = "def first\n  first_page = page\n  first_page.goto('/dashboard')\nend\ndef second\n  second_page = page\n  second_page.goto('/dashboard')\nend\nGiven('the parcel status is verified', &method(:first))\nGiven('the parcel status is now verified', &method(:second))\n";
    assert_eq!(action_rows(methods, &[]).0, DUP);
    for (label, first, second, expected) in [
        (
            "expanded declaration",
            "act = -> { page.goto('/a') }; act.call",
            "act = -> { page.goto('/a') }; act.call; page.wait()",
            NEAR,
        ),
        (
            "declaration never invoked",
            "act = -> { page.goto('/a') }",
            "act = -> { page.goto('/b') }",
            NONE,
        ),
        (
            "parameterized invocation",
            "act = ->(v) { page.goto(v) }; act.call('/a')",
            "act = ->(v) { page.goto(v) }; act.call('/a'); page.wait()",
            NONE,
        ),
        (
            "immediately invoked lambda",
            "(-> { page.goto('/a') }).call",
            "(-> { page.goto('/a') }).call; page.wait()",
            NEAR,
        ),
        // Executed bodies carry their calls: a different executed call separates the pair, and an
        // unexecuted declaration shared by both contributes nothing.
        (
            "expanded conflicting bodies",
            "act = -> { page.goto('/a') }; act.call",
            "act = -> { page.fill('/a') }; act.call",
            NONE,
        ),
        (
            "shared unexecuted declaration",
            "act = -> { warm(); sync() }; first()",
            "act = -> { warm(); sync() }; second()",
            NONE,
        ),
    ] {
        check_action_pair(label, first, second, expected);
    }
}

/// Constant roots keep the resolved value identity, so equal constants stay one handler.
#[test]
fn action_streams_keep_constant_root_values() {
    let source = "LEFT = 1\nRIGHT = 1\nOTHER = 2\nGiven('the parcel status is verified') { LEFT.touch() }\nGiven('the parcel status is now verified') { RIGHT.touch() }\nGiven('the archive badge is visible') { OTHER.touch() }\n";
    assert_eq!(action_rows(source, &[]).0, DUP);
}

/// An identical-stream pair never takes a candidate slot from a pair that can become near.
#[test]
fn identical_stream_decoys_do_not_consume_the_candidate_budget() {
    let source = "Given('the parcel status is verified') { write_status(:ready) }\nGiven('the parcel status is now verified') { write_status(:rejected) }\nGiven('the archive badge is visible') { page.goto('/login') }\nGiven('the archive badge is now visible') { page.goto('/login'); page.wait_for_load_state() }\n";
    let (rules, incomplete, code) = action_rows(source, &["--max-candidate-comparisons", "1"]);
    assert_eq!(rules, NEAR);
    assert!(!incomplete);
    assert_eq!(code, 0);
    let (rules, _, _) = action_rows(source, &[]);
    assert_eq!(rules, NEAR);
}

/// Loader calls without operands load nothing; any operand keeps the dependency diagnostic.
#[test]
fn loader_calls_without_operands_are_ordinary_calls() {
    for (label, body, incomplete) in [
        ("load without operands", "alpha { load(); render() }", false),
        ("require without operands", "require(); render()", false),
        (
            "require_relative without operands",
            "require_relative()",
            false,
        ),
        ("autoload without operands", "autoload()", false),
        ("comment-only operands", "load(# none\n)", false),
        ("block form", "load() { render() }", false),
        ("block argument only", "load(&blk)", false),
        ("string operand", "load('support/env.rb')", true),
        ("variable operand", "load(path)", true),
        ("splat operand", "load(*paths)", true),
        (
            "qualified receiver with operand",
            "Kernel.load('support/env.rb')",
            true,
        ),
    ] {
        let source = format!("Given('the parcel status is verified') {{ {body} }}\n");
        let (_, observed, _) = action_rows(&source, &[]);
        assert_eq!(observed, incomplete, "{label}");
    }
}

// ---------------------------------------------------------------------------------------------
// Shared-engine workflows (baselines, changed mode, exit policy, reporters) over Ruby sources.
// ---------------------------------------------------------------------------------------------

/// Two definitions with one matcher and different bodies: one `duplicate-matcher` finding.
const RUBY_DUPLICATE_PAIR: &str =
    "Given('same step') { first() }\nGiven('same step') { second() }\n";

/// Git with the hook-exported repository environment removed, so fixture commands stay sandboxed.
fn ruby_fixture_git() -> ProcessCommand {
    let mut command = ProcessCommand::new("git");
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_CEILING_DIRECTORIES",
    ] {
        command.env_remove(variable);
    }
    command
}

/// Runs a fixture Git command in `root` and returns its stdout as UTF-8, failing on a non-zero exit.
fn ruby_git(root: &Path, args: &[&str]) -> String {
    String::from_utf8(ruby_git_bytes(root, args)).unwrap()
}

/// Runs a fixture Git command in `root` and returns its raw stdout, which holds non-UTF-8 paths
/// verbatim, failing on a non-zero exit.
fn ruby_git_bytes(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = ruby_fixture_git()
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

/// Initializes a Git repository at `root` with a stable identity and a single initial commit.
fn ruby_init_repository(root: &Path) {
    ruby_init_repository_adding(root, ".");
}

/// Initializes a Git repository at `root` whose initial commit holds only `pathspec`.
fn ruby_init_repository_adding(root: &Path, pathspec: &str) {
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "test@example.com"],
        &["config", "user.name", "Test"],
        &["add", pathspec],
        &["commit", "-qm", "initial"],
    ] {
        ruby_git(root, args);
    }
}

/// Commits the index in `root` as `message` under a one-off committer identity.
fn ruby_commit(root: &Path, message: &str) {
    let identity = [
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
    ];
    ruby_git(root, &[&identity[..], &["commit", "-qm", message]].concat());
}

/// Writes `contents` at `relative` under `root`, creating parent directories.
fn ruby_write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    path.parent().map(fs::create_dir_all).unwrap().unwrap();
    fs::write(path, contents).unwrap();
}

/// Writes a `bytes`-long file of authored comment lines, valid as Ruby and as Gherkin.
fn ruby_write_sized(root: &Path, relative: &str, bytes: usize) {
    let line = "# filler line of authored text\n";
    let mut contents = line.repeat(bytes / line.len());
    contents.push_str(&"#".repeat(bytes - contents.len()));
    ruby_write(root, relative, &contents);
}

/// The binary run from `root` on `.` with the Ruby opt-in definitions `pattern`.
fn ruby_cli(root: &Path, pattern: &str) -> Command {
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .current_dir(root)
        .args([".", "--definitions", pattern]);
    command
}

/// Runs `ruby_cli` with JSONL output and returns the exit code and the parsed records.
fn ruby_jsonl(root: &Path, pattern: &str, extra: &[&str]) -> (i32, Vec<Value>) {
    let output = ruby_cli(root, pattern)
        .args(["--reporters", "jsonl", "--no-metrics"])
        .args(extra)
        .output()
        .unwrap();
    (output.status.code().unwrap(), records(output.stdout))
}

/// Counts finding records by `active` state: `(active, suppressed)`.
fn ruby_finding_states(rows: &[Value]) -> (usize, usize) {
    let findings = rows.iter().filter(|row| row["type"] == "finding");
    let active = findings.clone().filter(|row| row["active"] == true).count();
    (active, findings.count() - active)
}

/// Reads a JSON report file.
fn ruby_report(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Sums a baseline's per-fingerprint counts; `None` when any count is not an integer.
fn ruby_baseline_total(baseline: &Value) -> Option<u64> {
    let counts = baseline["fingerprints"].as_object().unwrap().values();
    counts.map(Value::as_u64).sum()
}

/// Counts a JSON report's findings by suppression: `(unsuppressed, suppressed)`.
fn ruby_suppression_states(report: &Value) -> (usize, usize) {
    let findings = report["findings"].as_array().unwrap();
    let suppressed = findings
        .iter()
        .filter(|finding| !finding["suppression"].is_null())
        .count();
    (findings.len() - suppressed, suppressed)
}

/// Asserts that each JSON pointer in `expected` resolves in `value` to the paired value.
fn ruby_assert_pointers(value: &Value, expected: &[(&str, Value)]) {
    for (pointer, want) in expected {
        assert_eq!(value.pointer(pointer), Some(want), "{pointer}");
    }
}

/// `ruby_cli` over `*.rb` in `root`, compared against the baseline at `revision`.
fn ruby_from_ref(root: &Path, revision: &str) -> Command {
    let mut command = ruby_cli(root, "*.rb");
    command.args(["--baseline-from-ref", revision]);
    command
}

/// A semantic baseline written from Ruby findings survives a move and a reversed pair order,
/// and suppresses only as many same-fingerprint findings as it counts.
#[test]
fn ruby_semantic_baseline_is_updated_moved_and_gated_by_new_findings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(root, "steps.rb", RUBY_DUPLICATE_PAIR);
    ruby_cli(root, "**/*.rb")
        .args([
            "--baseline",
            ".cuke-dedup-baseline.json",
            "--update-baseline",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("updated baseline"))
        .stderr(predicate::str::contains("1 added, 0 removed, 1 total"));
    let baseline = ruby_report(&root.join(".cuke-dedup-baseline.json"));
    assert_eq!(baseline["schemaVersion"], 3);
    assert_eq!(ruby_baseline_total(&baseline), Some(1));

    // Moved, shifted down and in reversed order: the primary and related sides swap, so the
    // comparison fingerprint must not depend on pair order or on location.
    fs::remove_file(root.join("steps.rb")).unwrap();
    ruby_write(
        root,
        "moved/renamed.rb",
        "\n\nGiven('same step') { second() }\nGiven('same step') { first() }\n",
    );
    ruby_cli(root, "**/*.rb")
        .args([
            "--baseline",
            ".cuke-dedup-baseline.json",
            "--reporters",
            "terminal",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 suppressed"))
        .stdout(predicate::str::contains(
            "Duplication: 0 of 2 definitions (0.00%), threshold 0.00% — PASS",
        ));

    // A second copy of the accepted pair yields more findings with the accepted fingerprint than
    // the baseline counts: only one is suppressed. A set-valued baseline would suppress all.
    ruby_write(
        root,
        "moved/copy.rb",
        "Given('same step') { first() }\nGiven('same step') { second() }\n",
    );
    let (code, rows) = ruby_jsonl(
        root,
        "**/*.rb",
        &["--baseline", ".cuke-dedup-baseline.json"],
    );
    assert_eq!(code, 1);
    let findings = rows
        .iter()
        .filter(|row| row["type"] == "finding")
        .collect::<Vec<_>>();
    let accepted = baseline["fingerprints"]
        .as_object()
        .unwrap()
        .keys()
        .next()
        .unwrap()
        .clone();
    let accepted_states = findings
        .iter()
        .filter(|row| row["fingerprint"].as_str() == Some(accepted.as_str()))
        .map(|row| row["active"].as_bool().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(accepted_states.len(), 2);
    assert_eq!(accepted_states.iter().filter(|active| **active).count(), 1);
    ruby_cli(root, "**/*.rb")
        .args(["--baseline", ".cuke-dedup-baseline.json", "--fail-on-new"])
        .assert()
        .code(1);
    let baseline = ruby_report(&root.join(".cuke-dedup-baseline.json"));
    assert_eq!(ruby_baseline_total(&baseline), Some(1));
}

/// Baseline updates over Ruby findings migrate old schemas, count removals without counting
/// suppressed findings, and reject future, outdated, malformed or unsafe baseline inputs.
#[test]
fn ruby_baseline_update_migrates_counts_removals_and_rejects_invalid_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(root, "steps.rb", RUBY_DUPLICATE_PAIR);
    let baseline_path = root.join("baseline.json");
    for version in [1, 2] {
        fs::write(
            &baseline_path,
            format!(r#"{{"schemaVersion":{version},"fingerprints":{{"legacy":1}}}}"#),
        )
        .unwrap();
        ruby_cli(root, "*.rb")
            .args(["--baseline", "baseline.json", "--update-baseline"])
            .assert()
            .success()
            .stderr(predicate::str::contains("updated baseline"));
        assert_eq!(ruby_report(&baseline_path)["schemaVersion"], 3);
    }
    fs::write(&baseline_path, r#"{"schemaVersion":99,"fingerprints":{}}"#).unwrap();
    ruby_cli(root, "*.rb")
        .args(["--baseline", "baseline.json", "--update-baseline"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("newer than supported"));

    ruby_write(
        root,
        "steps.rb",
        "Given('alpha') { one() }\nGiven('alpha') { two() }\nGiven('beta') { three() }\nGiven('beta') { four() }\n",
    );
    ruby_cli(root, "*.rb")
        .args(["--baseline", "nested/baseline.json", "--update-baseline"])
        .assert()
        .success()
        .stderr(predicate::str::contains("2 added, 0 removed, 2 total"));
    // The remaining finding is inline-suppressed, so it neither counts as added nor stays.
    ruby_write(
        root,
        "steps.rb",
        "# cuke-dedup:ignore duplicate-matcher -- already accepted elsewhere\nGiven('alpha') { one() }\nGiven('alpha') { two() }\n",
    );
    ruby_cli(root, "*.rb")
        .args(["--baseline", "nested/baseline.json", "--update-baseline"])
        .assert()
        .success()
        .stderr(predicate::str::contains("0 added, 2 removed, 0 total"));

    let nested = root.join("nested/baseline.json");
    for (contents, messages) in [
        (
            r#"{"schemaVersion":2,"fingerprints":{}}"#,
            vec![
                "unsupported baseline schema version `2` (expected `3`)",
                "regenerate it with --update-baseline",
            ],
        ),
        (
            r#"{"schemaVersion":99,"fingerprints":{}}"#,
            vec!["unsupported baseline schema"],
        ),
        ("not json", vec!["failed to parse baseline"]),
    ] {
        fs::write(&nested, contents).unwrap();
        let mut assertion = ruby_cli(root, "*.rb")
            .args(["--baseline", "nested/baseline.json"])
            .assert()
            .code(2);
        for message in messages {
            assertion = assertion.stderr(predicate::str::contains(message));
        }
    }

    for arguments in [
        vec!["--update-baseline"],
        vec!["--fail-on-new"],
        vec![
            "--baseline",
            "baseline.json",
            "--update-baseline",
            "--changed-since",
            "HEAD",
        ],
    ] {
        ruby_cli(root, "*.rb").args(arguments).assert().code(2);
    }
}

/// A Ruby duplicate-handler cluster keeps its baseline identity when its members move and
/// reorder, and a cluster that gains a member is a different, new finding.
#[test]
fn ruby_baseline_cluster_identity_ignores_location_and_member_order() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let members = ["alpha", "bravo", "charlie", "delta", "echo"];
    let source = |names: &[&str]| -> String {
        names
            .iter()
            .map(|name| format!("Given('{name}') {{ work() }}\n"))
            .collect()
    };
    ruby_write(root, "steps/a.rb", &source(&members));
    let (_, rows) = ruby_jsonl(root, "**/*.rb", &[]);
    let clusters: Vec<_> = rows.iter().filter(|row| row["type"] == "finding").collect();
    assert_eq!(clusters.len(), 1);
    assert_eq!(
        clusters[0]["message"],
        "5 step definitions form a connected `duplicate-handler` cluster"
    );
    ruby_cli(root, "**/*.rb")
        .args(["--baseline", "baseline.json", "--update-baseline"])
        .assert()
        .success();

    fs::remove_file(root.join("steps/a.rb")).unwrap();
    let mut reordered = members;
    reordered.reverse();
    ruby_write(
        root,
        "moved/renamed.rb",
        &format!("\n{}", source(&reordered)),
    );
    let (code, rows) = ruby_jsonl(root, "**/*.rb", &["--baseline", "baseline.json"]);
    assert_eq!((code, ruby_finding_states(&rows)), (0, (0, 1)));

    ruby_write(root, "moved/extra.rb", &source(&["foxtrot"]));
    let (code, rows) = ruby_jsonl(root, "**/*.rb", &["--baseline", "baseline.json"]);
    assert_eq!((code, ruby_finding_states(&rows)), (1, (1, 0)));
}

/// A repository directory name Git must round-trip exactly: trailing spaces off Linux.
#[cfg(not(target_os = "linux"))]
fn ruby_awkward_repository_name() -> std::path::PathBuf {
    // Trailing spaces are valid on Unix and must not be trimmed from Git's output.
    std::path::PathBuf::from(if cfg!(unix) {
        " repository "
    } else {
        "repository"
    })
}

/// A repository directory name Git must round-trip exactly: trailing spaces and a non-UTF-8 byte.
#[cfg(target_os = "linux")]
fn ruby_awkward_repository_name() -> std::path::PathBuf {
    use std::os::unix::ffi::OsStrExt;
    // Linux also permits non-UTF-8 bytes in repository roots.
    std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b" repository-\xff "))
}

/// Writes a `post-checkout` hook under `root` that fails, executable on Unix.
fn ruby_write_failing_hook(root: &Path) {
    ruby_write(root, "hooks/post-checkout", "#!/bin/sh\nexit 99\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = root.join("hooks/post-checkout");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// `--baseline-from-ref` over Ruby sources compares against history under the current policy,
/// tolerates current parse failures unless strict, and never mutates the checkout.
#[test]
fn ruby_baseline_from_ref_compares_history_without_mutating_the_checkout() {
    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path().join(ruby_awkward_repository_name());
    // Raw bytes: on Linux the repository name is not UTF-8 and appears in worktree listings.
    let git = |args: &[&str]| ruby_git_bytes(&root, args);
    fs::create_dir(&root).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "test@example.invalid"],
        &["config", "user.name", "Test"],
    ] {
        git(args);
    }
    let scope = root.join("packages/suite");
    ruby_write(&scope, "README.md", "Synthetic suite\n");
    for args in [
        &["add", "."][..],
        &["commit", "-qm", "empty suite"],
        &["tag", "empty-suite"],
    ] {
        git(args);
    }
    ruby_write(
        &scope,
        "steps.rb",
        "Given('shared') { first() }\nGiven('shared') { second() }\n",
    );
    ruby_write(
        &scope,
        "example.feature",
        "Feature: Example\n  Scenario: Example\n    Given shared\n",
    );
    // The old policy disables all rules. The current explicit overrides must govern BOTH scans.
    let rules: serde_json::Map<String, Value> = cuke_dedup::model::Rule::ALL
        .iter()
        .map(|rule| (rule.to_string(), Value::from("off")))
        .collect();
    ruby_write(
        &scope,
        ".cuke-dedup.json",
        &serde_json::json!({ "rules": rules }).to_string(),
    );
    ruby_write(&root, ".gitattributes", "*.rb filter=unsafe\n");
    git(&["add", "."]);
    git(&["commit", "-qm", "base"]);
    // These would break checkout if the snapshot inherited repository config or hooks.
    ruby_write_failing_hook(&root);
    for setting in [
        ["core.hooksPath", "hooks"],
        ["filter.unsafe.required", "true"],
        ["filter.unsafe.smudge", "nonexistent-cuke-filter"],
    ] {
        git(&[&["config"][..], &setting].concat());
    }
    let checkout_state = || {
        [
            git(&["rev-parse", "HEAD"]),
            git(&["ls-files", "--stage"]),
            git(&["worktree", "list", "--porcelain"]),
        ]
    };
    let before = checkout_state();
    fs::rename(scope.join("steps.rb"), scope.join("renamed café.rb")).unwrap();
    let warn_on_matcher = ["--rule", "duplicate-matcher=warning"];
    let run = |allowance: &str, expected: i32| {
        let mut command = ruby_from_ref(&scope, "HEAD");
        command
            .env("GIT_DIR", sandbox.path().join("not-a-repository"))
            .env("GIT_CONFIG_GLOBAL", root.join(".git/config"));
        for (key, value) in [
            ("git_config_count", "2"),
            ("gIt_cOnFiG_kEy_0", "filter.unsafe.required"),
            ("git_config_value_0", "true"),
            ("git_config_key_1", "filter.unsafe.smudge"),
            ("gIt_cOnFiG_vAlUe_1", "nonexistent-cuke-filter"),
        ] {
            command.env(key, value);
        }
        command
            .args(["--fail-on-new", allowance])
            .args(warn_on_matcher)
            .args(["--reporters", "json", "--output"])
            .arg(sandbox.path().join("reports"))
            .assert()
            .code(expected);
    };
    run("0", 0);
    ruby_from_ref(&scope, "empty-suite")
        .arg("--fail-on-new=0")
        .args(warn_on_matcher)
        .arg("--output")
        .arg(sandbox.path().join("empty-report"))
        .assert()
        .code(1);
    ruby_write(&scope, "third.rb", "Given('shared') { third() }\n");
    run("0", 1);
    run("1", 0);
    let report_path = sandbox.path().join("reports/cuke-dedup.json");
    assert_eq!(ruby_suppression_states(&ruby_report(&report_path)), (1, 1));
    // A current-source parse failure is tolerated by default, but `--fail-on-unparseable` keeps it
    // fatal even under a generous new-finding allowance.
    ruby_write(&scope, "broken.rb", "Given('bad') do\n value =");
    for (strict, code) in [(false, 0), (true, 2)] {
        let mut command = ruby_from_ref(&scope, "HEAD");
        command.args(["--fail-on-new", "99"]);
        if strict {
            command.arg("--fail-on-unparseable");
        }
        command
            .args(warn_on_matcher)
            .assert()
            .code(code)
            .stderr(predicate::str::contains(
                "Ruby source contains syntax errors",
            ));
    }
    fs::remove_file(scope.join("broken.rb")).unwrap();
    assert_eq!(checkout_state(), before);
    for (path, exists) in [
        (scope.join("renamed café.rb"), true),
        (scope.join("third.rb"), true),
        (scope.join(".cuke-dedup-baseline.json"), false),
    ] {
        assert_eq!(path.exists(), exists, "{}", path.display());
    }
    let report = ruby_report(&report_path);
    let mut primary_paths = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["primary"]["path"].as_str().unwrap_or(""));
    assert!(primary_paths.all(|path| !path.contains("checkout")));
    fs::create_dir(scope.join("new-directory")).unwrap();
    ruby_from_ref(&scope.join("new-directory"), "HEAD")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "does not contain the analysis directory",
        ));
    for (args, message) in [
        (&["--baseline-from-ref", "missing-ref"][..], "baseline"),
        (&["--baseline-from-ref=--help"], "baseline"),
        (
            &["--baseline-from-ref", "HEAD", "--baseline", "baseline.json"],
            "cannot be used",
        ),
        (
            &["--baseline-from-ref", "HEAD", "--update-baseline"],
            "cannot be used",
        ),
        (&["--fail-on-new"], "required"),
    ] {
        ruby_cli(&scope, "*.rb")
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains(message));
    }
}

/// An incomplete Ruby base revision is rejected under `--fail-on-incomplete` even when the
/// current files are valid, and is tolerated with a visible warning by default.
#[test]
fn ruby_baseline_from_ref_rejects_or_reports_incomplete_history() {
    let valid_feature = "Feature: Example\n  Scenario: Example\n    Given shared\n";
    // Control: a complete base under the same flags passes, so each rejection below is caused
    // by its base, named by `cause`.
    let directory = tempfile::tempdir().unwrap();
    ruby_write(directory.path(), "steps.rb", "Given('shared') { work() }\n");
    ruby_write(directory.path(), "example.feature", valid_feature);
    ruby_init_repository(directory.path());
    ruby_from_ref(directory.path(), "HEAD")
        .args(["--fail-on-new=0", "--fail-on-incomplete"])
        .assert()
        .code(0);
    for (bad_source, feature, cause) in [
        (
            "Given('shared') do\n value =",
            Some(valid_feature),
            "syntax errors",
        ),
        (
            "require 'missing/glue'\nGiven('shared') { work() }\n",
            Some(valid_feature),
            "dependency is unresolved",
        ),
        (
            "HELPER = 42\n",
            Some(valid_feature),
            "kept no step definitions",
        ),
        ("Given('shared') { work() }\n", None, "matched no files"),
        (
            "Given('shared') { work() }\n",
            Some("invalid Gherkin"),
            "failed to parse Gherkin feature",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        ruby_write(root, "steps.rb", bad_source);
        if let Some(feature) = feature {
            ruby_write(root, "example.feature", feature);
        }
        ruby_init_repository(root);
        ruby_write(root, "steps.rb", "Given('shared') { work() }\n");
        ruby_write(root, "example.feature", valid_feature);
        // Control: the current corpus alone is complete.
        ruby_cli(root, "*.rb")
            .arg("--fail-on-incomplete")
            .assert()
            .code(0);
        ruby_from_ref(root, "HEAD")
            .args(["--fail-on-new=0", "--fail-on-incomplete"])
            .assert()
            .code(2)
            .stderr(
                predicate::str::contains("baseline")
                    .and(predicate::str::contains("incomplete"))
                    .and(predicate::str::contains(cause)),
            );
    }

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // The base still extracts the real definition; only the unparseable sibling is lost.
    ruby_write(root, "steps.rb", "Given('shared') { work() }\n");
    ruby_write(root, "broken.rb", "Given('broken') do\n value =");
    ruby_write(root, "example.feature", valid_feature);
    ruby_init_repository(root);
    fs::remove_file(root.join("broken.rb")).unwrap();
    ruby_from_ref(root, "HEAD").assert().success().stderr(
        predicate::str::contains("baseline revision `HEAD` is incomplete")
            .and(predicate::str::contains("--fail-on-incomplete")),
    );
    ruby_from_ref(root, "HEAD")
        .arg("--fail-on-incomplete")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("incomplete"));
}

/// A Ruby base revision truncated by the structural-class limit is incomplete, and submodules
/// are refused whether real or injected or hidden by Git replacement objects.
#[test]
fn ruby_baseline_from_ref_rejects_truncated_base_and_unmaterialized_submodules() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let git = |args: &[&str]| ruby_git(root, args);
    git(&["init", "-q"]);
    // Receiver calls with literal arguments form one structural class of ten definitions.
    let source: String = (0..10)
        .map(|index| format!("Given('operation label {index}') {{ page.perform({index}) }}\n"))
        .collect();
    ruby_write(root, "steps.rb", &source);
    ruby_write(
        root,
        "example.feature",
        "Feature: Example\n  Scenario: Example\n    Given operation label 0\n",
    );
    git(&["add", "."]);
    ruby_commit(root, "base");
    ruby_write(
        root,
        "steps.rb",
        "Given('operation label 0') { page.perform(0) }\n",
    );
    ruby_from_ref(root, "HEAD")
        .args(["--max-structural-class-comparisons", "2"])
        .arg("--fail-on-incomplete")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "baseline revision `HEAD` is incomplete",
        ));
    let rev_parse = |revision: &str| git(&["rev-parse", revision]);
    let oid = rev_parse("HEAD");
    let gitlink_entry = ["--cacheinfo", "160000", oid.trim(), "vendor"];
    git(&[&["update-index", "--add"][..], &gitlink_entry].concat());
    ruby_commit(root, "gitlink");
    ruby_from_ref(root, "HEAD")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unsupported submodules"));
    let gitlink = rev_parse("HEAD");
    let gitlink_tree = rev_parse("HEAD^{tree}");
    let clean_tree = rev_parse(&format!("{}^{{tree}}", oid.trim()));
    for (original, replacement) in [
        (gitlink.trim(), oid.trim()),
        (gitlink_tree.trim(), clean_tree.trim()),
    ] {
        // A replacement must not hide a real submodule from the snapshot pre-check.
        git(&["replace", original, replacement]);
        ruby_from_ref(root, gitlink.trim())
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unsupported submodules"));
        git(&["replace", "-d", original]);
        // Conversely, a replacement must not inject a submodule into a clean base.
        git(&["replace", replacement, original]);
        ruby_from_ref(root, oid.trim()).assert().code(0);
        git(&["replace", "-d", replacement]);
    }
}

/// Changed mode decodes staged Unicode, untracked and leading-space Ruby paths relative to a
/// repository subdirectory, keeps findings that reach a changed file as primary or related
/// location, and drops findings among unchanged files only.
#[test]
fn ruby_changed_since_decodes_paths_and_keeps_findings_that_reach_changed_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(
        root,
        "steps/existing.rb",
        "Given('shared step') { existing() }\n",
    );
    ruby_init_repository(root);
    ruby_write(
        root,
        "steps/café changed.rb",
        "Given('shared step') { changed() }\n",
    );
    ruby_git(root, &["add", "steps/café changed.rb"]);
    ruby_cli(root, "**/*.rb")
        .args(["--changed-since", "HEAD"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("duplicate-matcher"))
        .stdout(predicate::str::contains("steps/café changed.rb"));
    ruby_cli(root, "**/*.rb")
        .args(["--changed-since", "HEAD", "--threshold", "100"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Duplication: 2 of 2 definitions (100.00%), threshold 100.00% — PASS",
        ));

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(
        root,
        "packages/e2e/steps/base.rb",
        "Given('tracked step') { base_tracked() }\nGiven('new step') { base_new() }\nGiven('café step') { base_unicode() }\n",
    );
    ruby_write(
        root,
        "packages/e2e/steps/träcked.rb",
        "Given('before edit') { before() }\n",
    );
    ruby_write(
        root,
        "packages/e2e/steps/old_a.rb",
        "Given('old step') { old_a() }\n",
    );
    ruby_write(
        root,
        "packages/e2e/steps/old_b.rb",
        "Given('old step') { old_b() }\n",
    );
    ruby_init_repository(root);
    ruby_write(
        root,
        "packages/e2e/steps/träcked.rb",
        "Given('tracked step') { after() }\n",
    );
    ruby_write(
        root,
        "packages/e2e/steps/ new step.rb",
        "Given('new step') { added() }\n",
    );
    ruby_write(
        root,
        "packages/e2e/steps/café step.rb",
        "Given('café step') { changed() }\n",
    );
    let subdirectory = root.join("packages/e2e");
    let matcher_findings = |extra: &[&str]| -> Vec<(String, String)> {
        let (code, rows) = ruby_jsonl(&subdirectory, "**/*.rb", extra);
        assert_eq!(code, 1);
        let mut found: Vec<_> = rows
            .iter()
            .filter(|row| row["rule"] == "duplicate-matcher")
            .map(|row| {
                (
                    row["primary"]["path"].as_str().unwrap().to_owned(),
                    row["related"][0]["path"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        found.sort();
        found
    };
    // Control: a full run also reports the pair among unchanged files.
    assert_eq!(matcher_findings(&[]).len(), 4);
    assert_eq!(
        matcher_findings(&["--changed-since", "HEAD"]),
        [
            ("steps/ new step.rb".to_owned(), "steps/base.rb".to_owned()),
            ("steps/base.rb".to_owned(), "steps/café step.rb".to_owned()),
            // The changed file is only the related location here.
            ("steps/base.rb".to_owned(), "steps/träcked.rb".to_owned()),
        ]
    );
}

/// Asserts changed mode reports the changed pair: terminal names the rule, JSONL has two active.
#[cfg(unix)]
fn ruby_assert_changed_pair_reported(root: &Path) {
    ruby_cli(root, "**/*.rb")
        .args(["--changed-since", "HEAD"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("duplicate-matcher"));
    let (code, rows) = ruby_jsonl(root, "**/*.rb", &["--changed-since", "HEAD"]);
    assert_eq!((code, ruby_finding_states(&rows)), (1, (2, 0)));
}

/// Changed mode keeps Ruby files whose names contain a newline, tracked and untracked.
#[cfg(unix)]
#[test]
fn ruby_changed_since_preserves_newlines_in_unix_names() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(
        root,
        "steps/existing.rb",
        "Given('shared step') { existing_shared() }\nGiven('tracked step') { existing_tracked() }\n",
    );
    ruby_write(
        root,
        "steps/line\nbreak.rb",
        "Given('before') { before() }\n",
    );
    ruby_init_repository(root);
    // Control: nothing changed yet, so nothing is reported.
    let (code, rows) = ruby_jsonl(root, "**/*.rb", &["--changed-since", "HEAD"]);
    assert_eq!((code, ruby_finding_states(&rows)), (0, (0, 0)));
    ruby_write(
        root,
        "steps/line\nbreak.rb",
        "Given('tracked step') { changed_tracked() }\n",
    );
    ruby_write(
        root,
        "steps/new\nline.rb",
        "Given('shared step') { changed_new() }\n",
    );
    ruby_assert_changed_pair_reported(root);
}

/// Changed mode keeps Ruby files whose names are not UTF-8, tracked and untracked.
#[cfg(target_os = "linux")]
#[test]
fn ruby_changed_since_preserves_non_utf8_linux_names() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(
        root,
        "steps/existing.rb",
        "Given('shared step') { existing_shared() }\nGiven('tracked step') { existing_tracked() }\n",
    );
    let tracked = root
        .join("steps")
        .join(OsString::from_vec(b"tracked-\xfe.rb".to_vec()));
    fs::write(&tracked, "Given('before') { before() }\n").unwrap();
    ruby_init_repository(root);
    let (code, rows) = ruby_jsonl(root, "**/*.rb", &["--changed-since", "HEAD"]);
    assert_eq!((code, ruby_finding_states(&rows)), (0, (0, 0)));
    fs::write(&tracked, "Given('tracked step') { changed_tracked() }\n").unwrap();
    fs::write(
        root.join("steps")
            .join(OsString::from_vec(b"non-utf8-\xff.rb".to_vec())),
        "Given('shared step') { changed_new() }\n",
    )
    .unwrap();
    ruby_assert_changed_pair_reported(root);
}

/// Changed mode over Ruby sources rejects unsafe or unresolvable revisions, non-repositories,
/// Git-ignored roots and an inherited Git environment instead of weakening the gate.
#[test]
fn ruby_changed_since_rejects_unsafe_revisions_roots_and_environments() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(root, "steps.rb", RUBY_DUPLICATE_PAIR);
    ruby_init_repository(root);
    for (revision, message) in [
        ("--diff-filter=X", "invalid --changed-since revision"),
        ("", "invalid --changed-since revision"),
        (" ", "invalid --changed-since revision"),
        ("--all", "invalid --changed-since revision"),
        (
            "missing-revision",
            "could not resolve --changed-since revision `missing-revision`",
        ),
    ] {
        ruby_cli(root, "*.rb")
            .arg(format!("--changed-since={revision}"))
            .assert()
            .code(2)
            .stderr(predicate::str::contains(message));
    }

    let ignored = tempfile::tempdir().unwrap();
    ruby_write(ignored.path(), ".gitignore", "ignored/\n");
    ruby_write(
        ignored.path(),
        "ignored/steps.rb",
        "Given('ignored step') { work() }\n",
    );
    ruby_init_repository_adding(ignored.path(), ".gitignore");
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(ignored.path())
        .args([
            "ignored",
            "--definitions",
            "*.rb",
            "--changed-since",
            "HEAD",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ignored by Git"));

    // The analyzed root is not a repository, so only a leaked environment could resolve one.
    let decoy = tempfile::tempdir().unwrap();
    ruby_write(decoy.path(), "sentinel.txt", "sentinel\n");
    ruby_init_repository(decoy.path());
    let outside = tempfile::tempdir().unwrap();
    ruby_write(
        outside.path(),
        "steps.rb",
        "Given('shared step') { work() }\n",
    );
    ruby_cli(outside.path(), "*.rb")
        .args(["--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "changed-files mode requires a Git repository",
        ));
    ruby_cli(outside.path(), "*.rb")
        .env("GIT_DIR", decoy.path().join(".git"))
        .env("GIT_WORK_TREE", decoy.path())
        .env("GIT_INDEX_FILE", decoy.path().join(".git/index"))
        .args(["--changed-since", "HEAD"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "changed-files mode requires a Git repository",
        ));
    assert_eq!(ruby_git(decoy.path(), &["ls-files"]), "sentinel.txt\n");
}

/// Completeness failures in unchanged Ruby sources and features still surface in changed mode:
/// unresolved registrations warn, syntax errors warn or fail under the flag, and unreadable,
/// oversized or unparseable inputs fail closed.
#[test]
fn ruby_changed_since_keeps_completeness_failures_of_unchanged_inputs() {
    let changed_run = |setup: &dyn Fn(&Path)| {
        let directory = tempfile::tempdir().unwrap();
        setup(directory.path());
        ruby_init_repository(directory.path());
        ruby_write(directory.path(), "changed.txt", "changed\n");
        directory
    };

    let unresolved = changed_run(&|root| {
        ruby_write(
            root,
            "steps.rb",
            "# a dynamic matcher cannot be resolved statically\nGiven(build_pattern) { work() }\n",
        );
    });
    ruby_cli(unresolved.path(), "*.rb")
        .args(["--changed-since", "HEAD"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "steps.rb:2:1: Ruby registration requires a static matcher and a statically resolved handler",
        ));

    let malformed = changed_run(&|root| ruby_write(root, "steps.rb", "Given('broken step') do\n"));
    ruby_cli(malformed.path(), "*.rb")
        .args(["--changed-since", "HEAD"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Ruby source contains syntax errors",
        ));
    ruby_cli(malformed.path(), "*.rb")
        .args(["--changed-since", "HEAD", "--fail-on-unparseable"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "Ruby source contains syntax errors",
        ));

    let unreadable = changed_run(&|root| fs::write(root.join("steps.rb"), [0xff]).unwrap());
    ruby_cli(unreadable.path(), "*.rb")
        .args(["--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not valid UTF-8"));

    let oversized = changed_run(&|root| {
        ruby_write_sized(root, "steps.rb", 8 * 1024 * 1024 + 1);
        ruby_write_sized(root, "features/example.feature", 8 * 1024 * 1024 + 1);
    });
    ruby_cli(oversized.path(), "*.rb")
        .args(["--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("definition source"))
        .stderr(predicate::str::contains("feature file"));

    let oversized_feature = changed_run(&|root| {
        ruby_write(root, "steps.rb", "Given('small step') { work() }\n");
        ruby_write_sized(root, "features/example.feature", 8 * 1024 * 1024 + 1);
    });
    ruby_cli(oversized_feature.path(), "*.rb")
        .args(["--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("feature file"))
        .stderr(predicate::str::contains("input limit"))
        .stderr(predicate::str::contains("definition source").not());

    let broken_feature = tempfile::tempdir().unwrap();
    ruby_write(
        broken_feature.path(),
        "steps.rb",
        "Given('a step') { work() }\n",
    );
    ruby_write(
        broken_feature.path(),
        "broken.feature",
        "Scenario: Missing feature\n  Given a step\n",
    );
    ruby_init_repository(broken_feature.path());
    ruby_cli(broken_feature.path(), "*.rb")
        .args(["--changed-since", "HEAD"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "found no tracked or untracked files",
        ))
        .stderr(predicate::str::contains("failed to parse Gherkin feature"));

    let unparsed = tempfile::tempdir().unwrap();
    ruby_write(
        unparsed.path(),
        "vendor/broken.feature",
        "Scenario: Missing feature\n  Given a step\n",
    );
    ruby_init_repository(unparsed.path());
    ruby_write(
        unparsed.path(),
        "steps/new.rb",
        "Given('new step') { work() }\n",
    );
    ruby_cli(unparsed.path(), "**/*.rb")
        .args(["--changed-since", "HEAD"])
        .args(["--rule", "unused-definition=error"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("unused-definition").not())
        .stderr(
            predicate::str::contains("no discovered feature file was parsed successfully")
                .and(predicate::str::contains("failed to parse Gherkin feature")),
        );
}

/// Exit status over Ruby findings follows rule severity, ignores the duplication threshold for
/// non-duplication errors, and honours inline, configured and explicit-config suppression policy.
#[test]
fn ruby_exit_policy_follows_severity_threshold_and_suppressions() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(root, "steps.rb", RUBY_DUPLICATE_PAIR);
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root)
        .args([
            "check",
            ".",
            "--definitions",
            "*.rb",
            "--rule",
            "duplicate-matcher=warning",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[warning]"));
    ruby_write(
        root,
        "example.feature",
        "Feature: Ambiguous\n  Scenario: One\n    Given same step\n",
    );
    ruby_cli(root, "*.rb")
        .args(["--threshold", "100"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("threshold 100.00% — PASS"))
        .stdout(predicate::str::contains("ambiguous-step"));

    // `--config` replaces the auto-discovered file, including its feature patterns and threshold.
    // The auto file also turns ambiguous-step off, a key team.json leaves unset: merging the two
    // files instead of replacing would hide the ambiguous-step finding asserted below.
    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{"threshold":0,"features":["missing/**/*.feature"],"rules":{"ambiguous-step":"off"}}"#,
    );
    ruby_write(
        root,
        "team.json",
        r#"{"threshold":100,"features":["features/**/*.feature"]}"#,
    );
    fs::remove_file(root.join("example.feature")).unwrap();
    ruby_write(
        root,
        "features/example.feature",
        "Feature: Config\n  Scenario: Explicit\n    Given same step\n",
    );
    ruby_cli(root, "*.rb")
        .args(["--config", "team.json"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("threshold 100.00% — PASS"))
        .stdout(predicate::str::contains("ambiguous-step"));
    // Control: the auto file's rule override takes effect when that file is used.
    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{"threshold":100,"features":["features/**/*.feature"],"rules":{"ambiguous-step":"off"}}"#,
    );
    ruby_cli(root, "*.rb")
        .assert()
        .stdout(predicate::str::contains("ambiguous-step").not());

    let inline = tempfile::tempdir().unwrap();
    ruby_write(
        inline.path(),
        "steps.rb",
        "# cuke-dedup:ignore duplicate-matcher -- intentionally separate setup\nGiven('same') { first() }\nGiven('same') { second() }\n",
    );
    ruby_cli(inline.path(), "*.rb")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 suppressed"));
    ruby_write(
        inline.path(),
        "broken.rb",
        "Given('unrelated') { work() }\n# cuke-dedup:ignore duplicate-handler\nGiven('broken') { work() }\n",
    );
    ruby_cli(inline.path(), "*.rb")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "broken.rb:2:1: inline suppression must use `# cuke-dedup:ignore RULE -- REASON`",
        ));

    let ambiguous = tempfile::tempdir().unwrap();
    ruby_write(
        ambiguous.path(),
        "steps.rb",
        "Given('the gauge reads {word}') { parameterized() }\nGiven('the gauge reads high') { literal() }\n",
    );
    ruby_write(
        ambiguous.path(),
        "features/gauge.feature",
        "Feature: Gauge\n  Scenario: One\n    Given the gauge reads high\n",
    );
    // At `warning` the finding is still reported and the run succeeds.
    ruby_write(
        ambiguous.path(),
        ".cuke-dedup.json",
        r#"{"rules": {"ambiguous-step": "warning"}, "reporters": ["json", "terminal"], "output": "reports"}"#,
    );
    ruby_cli(ambiguous.path(), "*.rb")
        .assert()
        .success()
        .stdout(predicate::str::contains("[warning]"))
        .stdout(predicate::str::contains("ambiguous-step"));
    // Control: at the default severity the same corpus fails.
    ruby_write(
        ambiguous.path(),
        ".cuke-dedup.json",
        r#"{"reporters": ["json", "terminal"], "output": "reports"}"#,
    );
    ruby_cli(ambiguous.path(), "*.rb")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("ambiguous-step"));
    // Suppressed: the finding is retained in the report carrying its reason, not dropped.
    ruby_write(
        ambiguous.path(),
        ".cuke-dedup.json",
        r#"{"reporters": ["json", "terminal"], "output": "reports", "suppressions": [{"rule": "ambiguous-step", "matcher": "the gauge reads high", "reason": "the literal wins at runtime"}]}"#,
    );
    ruby_cli(ambiguous.path(), "*.rb")
        .assert()
        .success()
        .stdout(predicate::str::contains("1 suppressed"));
    let report = ruby_report(&ambiguous.path().join("reports/cuke-dedup.json"));
    let ambiguity_reasons: Vec<_> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["rule"] == "ambiguous-step")
        .map(|finding| &finding["suppression"]["reason"])
        .collect();
    assert_eq!(ambiguity_reasons, ["the literal wins at runtime"]);
    assert_eq!(report["summary"]["findings"], 0);

    // `package.json` carries the Ruby definition pattern, reporters, rules and suppressions.
    let package = tempfile::tempdir().unwrap();
    ruby_write(
        package.path(),
        "package.json",
        r#"{
  "cukeDedup": {
    "definitions": ["specs/**/*.rb"],
    "features": ["specs/**/*.feature"],
    "reporters": ["json"],
    "output": "configured-reports",
    "rules": {"ambiguous-step": "off"},
    "suppressions": [{
      "rule": "duplicate-matcher",
      "matcher": "same step",
      "reason": "intentional compatibility alias"
    }]
  }
}"#,
    );
    ruby_write(package.path(), "specs/steps.rb", RUBY_DUPLICATE_PAIR);
    ruby_write(
        package.path(),
        "specs/example.feature",
        "Feature: Config\n  Scenario: One\n    Given same step\n",
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(package.path())
        .arg(".")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "configured-reports/cuke-dedup.json",
        ));
    let report = ruby_report(&package.path().join("configured-reports/cuke-dedup.json"));
    assert_eq!(report["summary"]["definitionsAnalyzed"], 2);
    assert_eq!(report["summary"]["suppressed"], 1);
    assert_eq!(
        report["findings"][0]["suppression"]["reason"],
        "intentional compatibility alias"
    );
}

/// Runs the binary on `root` with JSONL output and returns the exit code, records and stderr,
/// failing with both when no summary record was produced.
fn ruby_suppression_run(root: &Path, pattern: &str) -> (i32, Vec<Value>, String) {
    let output = ruby_cli(root, pattern)
        .args(["--reporters", "jsonl", "--no-metrics"])
        .output()
        .unwrap();
    let code = output.status.code().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let rows = records(output.stdout);
    assert!(
        rows.iter().any(|row| row["type"] == "summary"),
        "exit {code}: {stderr}"
    );
    (code, rows, stderr)
}

/// Each finding as `(rule, primary path:line, related path:line list, suppression reason)`,
/// sorted, so a test asserts which definitions each rule flagged together with its suppression
/// state.
fn ruby_suppression_rows(rows: &[Value]) -> Vec<(String, String, String, Option<String>)> {
    let location = |value: &Value| format!("{}:{}", value["path"].as_str().unwrap(), value["line"]);
    let mut findings: Vec<_> = rows
        .iter()
        .filter(|row| row["type"] == "finding")
        .map(|row| {
            (
                row["rule"].as_str().unwrap().to_owned(),
                location(&row["primary"]),
                row["related"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(location)
                    .collect::<Vec<_>>()
                    .join(","),
                row["suppression"]["reason"].as_str().map(str::to_owned),
            )
        })
        .collect();
    findings.sort();
    findings
}

/// Ruby `# cuke-dedup:ignore` directives go through the shared parser: adjacent directives each
/// suppress only their own rule on the registration below them, a pair finding takes the inline
/// reason from either of its definitions, and missing separators, unknown rules, empty reasons and reasons over 512 characters
/// are rejected with located errors while the targeted findings stay active.
#[test]
fn ruby_inline_directives_are_rule_scoped_and_reject_malformed_input() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // Rejected directives target `unused-definition`, so each rejection leaves an active finding
    // on its registration instead of a suppressed one.
    ruby_write(
        root,
        "steps.rb",
        "# directives start below line 1
# cuke-dedup:ignore duplicate-matcher -- wording fixed by an external contract
# cuke-dedup:ignore unused-definition -- exercised by a remote suite
Given('external wording') { work() }
Given('external wording') { other_work() }
Given('separate wording') { work() }
# cuke-dedup:ignore unused-definition
Then('broken directive') { other() }
# cuke-dedup:ignore unknown-rule -- not a real rule
When('unknown rule') { another() }
# cuke-dedup:ignore unused-definition --
When('empty reason') { final_action() }
",
    );
    // 513 characters exceed the limit; exactly 512 are accepted and retained in full.
    let (oversized, boundary) = ("x".repeat(513), "y".repeat(512));
    ruby_write(
        root,
        "oversized.rb",
        &format!(
            "# boundary directives start below line 1
# cuke-dedup:ignore unused-definition -- {oversized}
Given('oversized reason') {{ left() }}
# cuke-dedup:ignore unused-definition -- {boundary}
Given('boundary reason') {{ right() }}
"
        ),
    );
    // The directive sits on the pair's related definition, not its primary one.
    ruby_write(
        root,
        "related.rb",
        "Given('related left') { shared_related() }
# cuke-dedup:ignore duplicate-handler -- attached to the related definition
Given('related right') { shared_related() }
",
    );
    ruby_write(
        root,
        "usage.feature",
        "Feature: Usage\n  Scenario: None\n    Given nothing matches\n",
    );
    let (code, rows, stderr) = ruby_suppression_run(root, "*.rb");
    assert_eq!(code, 2, "{stderr}");
    let row = |rule: &str, location: &str, related: &str, reason: Option<&str>| {
        (
            rule.to_owned(),
            location.to_owned(),
            related.to_owned(),
            reason.map(str::to_owned),
        )
    };
    let unused = "unused-definition";
    assert_eq!(
        ruby_suppression_rows(&rows),
        [
            row(
                "duplicate-handler",
                "related.rb:1",
                "related.rb:3",
                Some("attached to the related definition")
            ),
            // Line 4's `duplicate-matcher` directive does not reach its handler pair with line 6.
            row("duplicate-handler", "steps.rb:4", "steps.rb:6", None),
            row(
                "duplicate-matcher",
                "steps.rb:4",
                "steps.rb:5",
                Some("wording fixed by an external contract")
            ),
            row(unused, "oversized.rb:3", "", None),
            row(unused, "oversized.rb:5", "", Some(&boundary)),
            row(unused, "related.rb:1", "", None),
            row(unused, "related.rb:3", "", None),
            row(unused, "steps.rb:10", "", None),
            row(unused, "steps.rb:12", "", None),
            row(
                unused,
                "steps.rb:4",
                "",
                Some("exercised by a remote suite")
            ),
            row(unused, "steps.rb:5", "", None),
            row(unused, "steps.rb:6", "", None),
            row(unused, "steps.rb:8", "", None),
        ]
    );
    let diagnostics: Vec<_> = stderr
        .lines()
        .filter(|line| line.starts_with("cuke-dedup:"))
        .collect();
    assert_eq!(
        diagnostics,
        [
            "cuke-dedup: oversized.rb:2:1: inline suppression reason exceeds the 512-character limit",
            "cuke-dedup: steps.rb:7:1: inline suppression must use `# cuke-dedup:ignore RULE -- REASON`",
            "cuke-dedup: steps.rb:9:1: unknown rule `unknown-rule`",
            "cuke-dedup: steps.rb:11:1: inline suppression reason must not be empty",
        ]
    );
}

/// Configured suppressions over Ruby findings: `path` is a root-relative glob that must cover
/// every definition of a pair, `path` and `matcher` must select the same definition, a trailing
/// `/` selects the directory tree, and unmatched detection uses the same selection.
#[test]
fn ruby_configured_suppressions_select_root_relative_paths_and_matchers() {
    let pair = |left: &'static str, right: &'static str| {
        [
            (left, "Given('left matcher') { work() }\n"),
            (right, "Given('right matcher') { work() }\n"),
        ]
    };
    // (files, path, matcher, suppressed, reported as matching no definition)
    let cases = [
        (
            pair("left.rb", "right.rb"),
            "left.rb",
            Some("right matcher"),
            false,
            true,
        ),
        (
            pair("left.rb", "right.rb"),
            "*.rb",
            Some("missing matcher"),
            false,
            true,
        ),
        (
            pair("left.rb", "right.rb"),
            "*.rb",
            Some("right matcher"),
            true,
            false,
        ),
        // `*` crosses `/`, so a root glob covers nested files; a bare file name does not.
        (
            pair("root.rb", "nested/steps.rb"),
            "*.rb",
            None,
            true,
            false,
        ),
        (
            pair("root.rb", "nested/steps.rb"),
            "steps.rb",
            None,
            false,
            true,
        ),
        (
            pair("legacy/left.rb", "current/right.rb"),
            "legacy/**",
            None,
            false,
            false,
        ),
        (
            pair("legacy/left.rb", "legacy/right.rb"),
            "legacy/**",
            None,
            true,
            false,
        ),
        (
            pair("legacy/nested/left.rb", "legacy/nested/right.rb"),
            "legacy/",
            None,
            true,
            false,
        ),
        (
            pair("legacy/nested/left.rb", "legacy/nested/right.rb"),
            "missing/**",
            None,
            false,
            true,
        ),
    ];
    for (index, (files, path, matcher, suppressed, unmatched)) in cases.into_iter().enumerate() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for (relative, source) in files {
            ruby_write(root, relative, source);
        }
        let reason = format!("case {index}");
        let suppression = serde_json::json!({
            "rule": "duplicate-handler", "reason": reason, "path": path, "matcher": matcher,
        });
        ruby_write(
            root,
            ".cuke-dedup.json",
            &serde_json::json!({ "suppressions": [suppression] }).to_string(),
        );
        let (_, rows, stderr) = ruby_suppression_run(root, "**/*.rb");
        let handler_rows: Vec<_> = ruby_suppression_rows(&rows)
            .into_iter()
            .filter(|(rule, ..)| rule == "duplicate-handler")
            .map(|(_, primary, related, reason)| (primary, related, reason))
            .collect();
        // Discovery sorts paths, so the lexically first file is the primary definition.
        let mut locations = files.map(|(relative, _)| format!("{relative}:1"));
        locations.sort();
        let [primary, related] = locations;
        let context = format!("case {index}: {path} {matcher:?}\n{stderr}");
        assert_eq!(
            handler_rows,
            [(primary, related, suppressed.then(|| reason.clone()))],
            "{context}"
        );
        assert_eq!(
            stderr.contains("suppression 1 for duplicate-handler matched no step definitions"),
            unmatched,
            "{context}"
        );
    }
}

/// Configured suppression lookups charge the pair work budget per suppression of the looked-up
/// rule, and unmatched-suppression validation stops at its own limit over 700 Ruby files while a
/// few unmatched suppressions are all reported.
#[test]
fn ruby_suppression_lookups_share_the_pair_work_budget() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // Long paths and 1,000 path suppressions make each pair lookup expensive; the same handler
    // body puts all 700 definitions in one identical-handler class.
    let nested = "nested/".repeat(30);
    for index in 0..700 {
        ruby_write(
            root,
            &format!("{nested}steps-{index}.rb"),
            &format!("Given('operation label {index:04}') {{ shared_implementation() }}\n"),
        );
    }
    // Controls: the same suppressions under a rule no pair lookup consults charge no pair work,
    // and three unmatched suppressions stay within the validation limit.
    // (rule, suppressions, pair budget exhausted, validation stopped)
    for (rule, count, truncated, stopped) in [
        ("duplicate-handler", 1_000, true, true),
        ("unused-definition", 1_000, false, true),
        ("unused-definition", 3, false, false),
    ] {
        let suppressions: Vec<_> = (0..count)
            .map(|index| {
                serde_json::json!({
                    "rule": rule, "reason": format!("legacy exception {index}"),
                    "path": format!("missing-{index}/**"),
                })
            })
            .collect();
        ruby_write(
            root,
            ".cuke-dedup.json",
            &serde_json::json!({ "suppressions": suppressions }).to_string(),
        );
        let (_, rows, stderr) = ruby_suppression_run(root, "**/*.rb");
        let analysis = &rows.last().unwrap()["analysis"];
        let sources = &analysis["candidateSources"];
        let context = format!("{rule} x{count}: {analysis}");
        assert_eq!(analysis["truncated"], truncated, "{context}");
        let evaluated = |source: &str| sources[source]["evaluated"].as_u64().unwrap();
        if truncated {
            // The budget runs out inside the identical-handler class, before its 699 pairs.
            assert!(evaluated("identicalHandler") < 699, "{context}");
            assert!(
                analysis["candidateComparisonsEvaluated"].as_u64().unwrap() < 1_397,
                "{context}"
            );
        } else {
            assert_eq!(
                analysis["candidateComparisonsEvaluated"], 1_397,
                "{context}"
            );
            assert_eq!(
                (evaluated("identicalHandler"), evaluated("matcherBlocking")),
                (699, 698),
                "{context}"
            );
        }
        assert_eq!(
            stderr.matches("after safety limits").count(),
            usize::from(truncated),
            "{context}\n{stderr}"
        );
        assert_eq!(
            stderr.contains("unmatched suppression validation stopped after its safety limit"),
            stopped,
            "{context}"
        );
        let unmatched = stderr.matches("matched no step definitions").count();
        if stopped {
            assert!(unmatched < count, "{context}: {unmatched}");
        } else {
            assert_eq!(unmatched, count, "{context}");
        }
    }
}

/// A library caller can bypass config decoding with an over-limit suppression reason; the
/// retained Ruby finding records the reason truncated to the 512-character bound.
#[test]
fn ruby_library_suppression_reasons_are_bounded_when_a_finding_is_retained() {
    use cuke_dedup::config::{Config, ConfigOverrides, SuppressionConfig};
    use cuke_dedup::discovery::{SourceFile, SourceLanguage};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let file = SourceFile {
        path: root.join("steps.rb"),
        language: SourceLanguage::Ruby,
    };
    let extracted = cuke_dedup::source_adapter::SOURCE_ADAPTER_REGISTRY
        .iter()
        .map(|registration| registration.adapter)
        .find(|adapter| adapter.language() == SourceLanguage::Ruby)
        .unwrap()
        .extract(RUBY_DUPLICATE_PAIR, &file)
        .unwrap();
    let mut config = Config::load(root, ConfigOverrides::default()).unwrap();
    let reason = "accepted because migration is in progress ".repeat(10_000);
    config.suppressions.push(SuppressionConfig {
        rule: cuke_dedup::model::Rule::DuplicateMatcher,
        reason: reason.clone(),
        path: Some("**".to_owned()),
        matcher: None,
    });
    let outcome =
        cuke_dedup::analysis::analyze_with_diagnostics(extracted.definitions, Vec::new(), &config)
            .unwrap();
    let suppressed: Vec<_> = outcome
        .result
        .findings
        .iter()
        .map(|finding| {
            (
                finding.rule,
                finding.suppression.as_ref().map(|s| s.reason.len()),
            )
        })
        .collect();
    assert_eq!(
        suppressed,
        [(cuke_dedup::model::Rule::DuplicateMatcher, Some(512))]
    );
    assert_eq!(
        outcome.result.findings[0]
            .suppression
            .as_ref()
            .unwrap()
            .reason,
        reason[..512]
    );
}

/// Candidate limits above the hard safety ceilings are rejected from config and CLI, and a Ruby
/// run at the ceilings is accepted.
#[test]
fn ruby_candidate_limits_cannot_exceed_hard_safety_ceilings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(root, "steps.rb", RUBY_DUPLICATE_PAIR);
    for (config, expected) in [
        (
            r#"{"maxCandidateComparisons":2000001}"#,
            "maxCandidateComparisons must not exceed the hard safety limit of 2000000",
        ),
        (
            r#"{"maxStructuralClassComparisons":250001}"#,
            "maxStructuralClassComparisons must not exceed the hard safety limit of 250000",
        ),
    ] {
        ruby_write(root, ".cuke-dedup.json", config);
        for print in [true, false] {
            let mut command = ruby_cli(root, "*.rb");
            if print {
                command.arg("--print-config");
            }
            command
                .assert()
                .code(2)
                .stderr(predicate::str::contains(expected));
        }
    }
    ruby_write(root, ".cuke-dedup.json", "{}");
    for (flag, value, expected) in [
        (
            "--max-candidate-comparisons",
            "2000001",
            "maxCandidateComparisons must not exceed the hard safety limit of 2000000",
        ),
        (
            "--max-structural-class-comparisons",
            "250001",
            "maxStructuralClassComparisons must not exceed the hard safety limit of 250000",
        ),
    ] {
        ruby_cli(root, "*.rb")
            .args([flag, value])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(expected));
    }
    let ceilings = [
        "--max-candidate-comparisons",
        "2000000",
        "--max-structural-class-comparisons",
        "250000",
    ];
    let printed = ruby_cli(root, "*.rb")
        .arg("--print-config")
        .args(ceilings)
        .assert()
        .success();
    let config: Value = serde_json::from_slice(&printed.get_output().stdout).unwrap();
    ruby_assert_pointers(
        &config,
        &[
            ("/maxCandidateComparisons", 2_000_000.into()),
            ("/maxStructuralClassComparisons", 250_000.into()),
        ],
    );
    let (code, rows) = ruby_jsonl(root, "*.rb", &ceilings);
    assert_eq!(code, 1);
    assert_eq!(rows.last().unwrap()["analysis"]["truncated"], false);
    assert!(rows.iter().any(|row| row["rule"] == "duplicate-matcher"));
}

/// Ruby structural-class truncation warns, still writes findings to every reporter, refuses to
/// update a baseline, and fails only when `--fail-on-incomplete` opts in.
#[test]
fn ruby_candidate_limits_warn_and_still_report_without_failing_the_run() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // Receiver calls with literal arguments form one structural class of ten definitions.
    let source: String = (0..10)
        .map(|index| format!("Given('operation label {index}') {{ page.perform({index}) }}\n"))
        .collect();
    ruby_write(root, "steps.rb", &source);
    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{
          "reporters": ["json", "html", "sarif"],
          "output": "reports",
          "noMetrics": true,
          "maxCandidateComparisons": 100,
          "maxStructuralClassComparisons": 2
        }"#,
    );
    ruby_cli(root, "*.rb")
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "warning: analysis is incomplete: evaluated 2 candidate definition comparisons",
        ))
        .stderr(predicate::str::contains(
            "affected structural classes start at steps.rb:1:1",
        ));
    let report = ruby_report(&root.join("reports/cuke-dedup.json"));
    assert!(report.get("metrics").is_none());
    ruby_assert_pointers(
        &report["analysis"],
        &[
            ("/truncated", true.into()),
            ("/candidateComparisonsEvaluated", 2.into()),
            ("/skippedCandidateComparisons", 43.into()),
            ("/truncatedStructuralClasses", 1.into()),
            ("/candidateSources/structuralHandler/evaluated", 2.into()),
        ],
    );
    assert!(!report["findings"].as_array().unwrap().is_empty());
    let html = fs::read_to_string(root.join("reports/cuke-dedup.html")).unwrap();
    assert!(html.contains("Analysis is incomplete: 2 candidate comparisons were evaluated"));
    assert!(html.contains(r#"class="truncation-notice" role="alert""#));
    let sarif = ruby_report(&root.join("reports/cuke-dedup.sarif"));
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["properties"]["analysis"]["truncated"],
        true
    );
    assert_eq!(
        sarif["runs"][0]["invocations"][0]["executionSuccessful"],
        false
    );
    let output = ruby_cli(root, "*.rb")
        .args(["--reporters", "jsonl"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let summary = records(output.stdout).pop().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["analysis"]["truncated"], true);
    assert!(summary.get("metrics").is_none());

    ruby_cli(root, "*.rb")
        .args([
            "--baseline",
            "incomplete-baseline.json",
            "--update-baseline",
            "--reporters",
            "json",
        ])
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "was not updated because analysis is incomplete",
        ));
    assert!(!root.join("incomplete-baseline.json").exists());
    ruby_cli(root, "*.rb")
        .args(["--reporters", "json", "--fail-on-incomplete"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "cuke-dedup: analysis is incomplete: evaluated 2 candidate definition comparisons",
        ));
}

/// Runs `source` as `steps.rb` under both candidate limits; returns the JSONL records and stderr.
fn ruby_limited_run(source: &str, candidates: usize, structural: usize) -> (Vec<Value>, String) {
    let (candidates, structural) = (candidates.to_string(), structural.to_string());
    let limits = [
        "--max-candidate-comparisons",
        &candidates,
        "--max-structural-class-comparisons",
        &structural,
    ];
    ruby_records_run(&[("steps.rb", source)], None, &limits)
}

/// Joins one registration line per index.
fn ruby_steps(count: usize, line: impl Fn(usize) -> String) -> String {
    (0..count).map(|index| line(index) + "\n").collect()
}

/// One census row: context, Ruby source, candidate limit, structural limit, expected pointers.
type RubyCensusRow<'a> = (&'a str, String, usize, usize, &'a [(&'a str, Value)]);

/// Ruby candidate buckets, source attribution and both limit boundaries match the engine census:
/// unrelated pairs are never proposed, same-matcher pairs never spend the structural limit, and
/// trivial or uncomparable handlers spend no budget.
#[test]
fn ruby_candidate_census_pins_buckets_and_limit_boundaries() {
    // `page.perform(<literal>)` handlers share one structural class, so every `same` row also
    // carries structural pairs that the matcher partition must keep out of the structural limit.
    let same = |count| ruby_steps(count, |i| format!("Given('same') {{ page.perform({i}) }}"));
    let distinct = ruby_steps(3, |i| {
        format!("Given('operation {i}') {{ page.perform({i}) }}")
    });
    let unrelated = ruby_steps(100, |i| {
        format!("Given('unique step {i}') {{ page.action_{i}(1) }}")
    });
    // One identical-handler pair inside a four-member structural class: the structural pass must
    // skip the pair the identical-handler source already holds (4 skipped, not 5).
    let overlap = ["first", "second", "third", "fourth"]
        .iter()
        .zip(["same", "same", "third", "fourth"])
        .map(|(matcher, value)| format!("Given('{matcher}') {{ page.act('{value}') }}\n"))
        .collect::<String>();
    let pair = |handler: &str| format!("Given('one') {handler}\nGiven('two') {handler}\n");
    let rows: [RubyCensusRow<'_>; 15] = [
        (
            "unrelated wording and receivers",
            unrelated,
            100,
            100,
            &[
                ("/analysis/candidateComparisonsEvaluated", 0.into()),
                ("/summary/definitionsAnalyzed", 100.into()),
            ],
        ),
        (
            "exact matcher group is a spanning chain",
            same(3),
            100,
            100,
            &[
                (
                    "/analysis/candidateSources/normalizedMatcher/evaluated",
                    2.into(),
                ),
                ("/analysis/candidateComparisonsEvaluated", 2.into()),
                ("/analysis/truncated", false.into()),
            ],
        ),
        (
            "global limit attributes skips to its source",
            same(5),
            2,
            100,
            &[
                (
                    "/analysis/candidateSources/normalizedMatcher/evaluated",
                    2.into(),
                ),
                (
                    "/analysis/candidateSources/normalizedMatcher/skipped",
                    2.into(),
                ),
                ("/analysis/candidateComparisonsEvaluated", 2.into()),
                ("/analysis/truncated", true.into()),
            ],
        ),
        (
            "structural limit is inclusive",
            distinct.clone(),
            4,
            3,
            &[
                (
                    "/analysis/candidateSources/structuralHandler/evaluated",
                    3.into(),
                ),
                ("/analysis/truncatedStructuralClasses", 0.into()),
                ("/analysis/truncated", false.into()),
            ],
        ),
        (
            "structural limit one below the class",
            distinct,
            4,
            2,
            &[
                (
                    "/analysis/candidateSources/structuralHandler/evaluated",
                    2.into(),
                ),
                (
                    "/analysis/candidateSources/structuralHandler/skipped",
                    1.into(),
                ),
                ("/analysis/truncatedStructuralClasses", 1.into()),
            ],
        ),
        // Only the pair stage is pinned: at this exact boundary the overlap pass still reports a
        // skip with nothing to overlap, so end-to-end inclusivity is not claimed here.
        (
            "global limit is inclusive at the pair stage",
            same(3),
            2,
            10,
            &[
                (
                    "/analysis/candidateSources/normalizedMatcher/evaluated",
                    2.into(),
                ),
                (
                    "/analysis/candidateSources/normalizedMatcher/skipped",
                    0.into(),
                ),
            ],
        ),
        (
            "global limit one below skips matcher pairs only",
            same(3),
            1,
            10,
            &[
                (
                    "/analysis/candidateSources/normalizedMatcher/skipped",
                    1.into(),
                ),
                (
                    "/analysis/candidateSources/structuralHandler/skipped",
                    0.into(),
                ),
                (
                    "/analysis/candidateSources/structuralHandler/evaluated",
                    0.into(),
                ),
            ],
        ),
        (
            "same-matcher pairs spare the structural limit",
            same(3),
            3,
            1,
            &[
                (
                    "/analysis/candidateSources/normalizedMatcher/evaluated",
                    2.into(),
                ),
                (
                    "/analysis/candidateSources/structuralHandler/evaluated",
                    0.into(),
                ),
                ("/analysis/truncatedStructuralClasses", 0.into()),
                ("/analysis/truncated", false.into()),
            ],
        ),
        (
            "same-matcher class reports no structural truncation",
            same(4),
            10,
            1,
            &[
                (
                    "/analysis/candidateSources/normalizedMatcher/evaluated",
                    3.into(),
                ),
                ("/analysis/skippedCandidateComparisons", 0.into()),
                ("/analysis/truncated", false.into()),
            ],
        ),
        (
            "structural pass skips pairs held by a handler group",
            overlap.clone(),
            10,
            1,
            &[
                (
                    "/analysis/candidateSources/identicalHandler/evaluated",
                    1.into(),
                ),
                (
                    "/analysis/candidateSources/structuralHandler/skipped",
                    4.into(),
                ),
                ("/analysis/skippedCandidateComparisons", 4.into()),
                ("/analysis/truncated", true.into()),
            ],
        ),
        (
            "overlap shape without the structural limit",
            overlap,
            10,
            100,
            &[
                ("/analysis/candidateComparisonsEvaluated", 6.into()),
                ("/analysis/truncated", false.into()),
            ],
        ),
        (
            "trivial handlers",
            pair("{}"),
            100,
            100,
            &[
                ("/analysis/candidateComparisonsEvaluated", 0.into()),
                ("/summary/definitionsAnalyzed", 2.into()),
            ],
        ),
        (
            "non-trivial control",
            pair("{ page.perform }"),
            100,
            100,
            &[(
                "/analysis/candidateSources/identicalHandler/evaluated",
                1.into(),
            )],
        ),
        // An optional block parameter leaves the handler uncomparable but the definition extracted.
        (
            "uncomparable handlers",
            pair("{ |value = 1| page.perform(value) }"),
            100,
            100,
            &[
                ("/analysis/candidateComparisonsEvaluated", 0.into()),
                ("/summary/definitionsAnalyzed", 2.into()),
                ("/corpus/incomplete", true.into()),
            ],
        ),
        (
            "comparable control",
            pair("{ |value| page.perform(value) }"),
            100,
            100,
            &[(
                "/analysis/candidateSources/identicalHandler/evaluated",
                1.into(),
            )],
        ),
    ];
    for (context, source, candidates, structural, expected) in rows {
        let (rows, _) = ruby_limited_run(&source, candidates, structural);
        let summary = rows.last().unwrap();
        for (pointer, want) in expected {
            assert_eq!(summary.pointer(pointer), Some(want), "{context}: {pointer}");
        }
    }

    // Past the matcher-blocking posting cap a homogeneous handler group stays linear: a spanning
    // identical-handler chain, not the 44,850 pairs of the group.
    let homogeneous = ruby_steps(300, |i| {
        format!("Given('homogeneous handler wording {i}') {{ shared_implementation() }}")
    });
    let (rows, _) = ruby_limited_run(&homogeneous, 2_000_000, 250_000);
    let analysis = &rows.last().unwrap()["analysis"];
    assert_eq!(
        analysis["candidateSources"]["identicalHandler"]["evaluated"],
        299
    );
    assert!(analysis["candidateComparisonsEvaluated"].as_u64().unwrap() <= 600);
}

/// Ruby structural truncation spends the class limit per class, keeps findings from every class,
/// reports one partial-findings diagnostic, and bounds its listed class locations.
#[test]
fn ruby_structural_truncation_keeps_later_classes_and_bounds_locations() {
    let classes = |count: usize, members: usize| {
        ruby_steps(count * members, |i| {
            let class = i / members;
            format!("Given('class {class} operation {i}') {{ receiver{class}.act({i}) }}")
        })
    };
    let (rows, _) = ruby_limited_run(&classes(2, 6), 100, 2);
    ruby_assert_pointers(
        &rows.last().unwrap()["analysis"],
        &[
            ("/truncatedStructuralClasses", 2.into()),
            ("/candidateSources/structuralHandler/evaluated", 4.into()),
        ],
    );
    let lines: Vec<u64> = rows
        .iter()
        .filter(|row| row["rule"] == "parameterization-candidate")
        .map(|row| row["primary"]["line"].as_u64().unwrap())
        .collect();
    assert!(lines.iter().any(|line| *line <= 6), "{lines:?}");
    assert!(lines.iter().any(|line| *line > 6), "{lines:?}");

    let (rows, stderr) = ruby_limited_run(&classes(1, 143), 100, 20);
    let analysis = &rows.last().unwrap()["analysis"];
    ruby_assert_pointers(
        analysis,
        &[
            ("/truncated", true.into()),
            ("/candidateComparisonsEvaluated", 20.into()),
            ("/truncatedStructuralClasses", 1.into()),
        ],
    );
    assert!(analysis["candidateSources"]["structuralHandler"]["skipped"].as_u64() > Some(0));
    assert!(rows
        .iter()
        .any(|row| row["rule"] == "parameterization-candidate"));
    // Two missing-feature warnings plus exactly one incompleteness diagnostic.
    assert_eq!(
        stderr.matches("cuke-dedup: warning:").count(),
        3,
        "{stderr}"
    );
    assert_eq!(stderr.matches("partial findings are available").count(), 1);

    let (rows, stderr) = ruby_limited_run(&classes(5, 3), 100, 1);
    assert_eq!(
        rows.last().unwrap()["analysis"]["truncatedStructuralClasses"],
        5
    );
    assert!(stderr.contains("start at steps.rb:1:1, steps.rb:4:1, steps.rb:7:1, and 2 more;"));
    let (rows, stderr) = ruby_limited_run(&classes(1, 3), 100, 1);
    assert_eq!(
        rows.last().unwrap()["analysis"]["truncatedStructuralClasses"],
        1
    );
    assert!(
        stderr.contains("start at steps.rb:1:1; partial"),
        "{stderr}"
    );
}

/// Ruby pairs whose matcher or handler similarity matrix exceeds the work ceiling are skipped
/// before verification, report truncation, and produce no similarity finding.
#[test]
fn ruby_similarity_work_limits_fail_closed_before_pair_verification() {
    // Matchers just past the 100,000,000-cell matrix ceiling (`l * r + l + r`); 1,010 characters
    // is the evaluated control.
    let matchers = |length: usize| {
        let prefix = "a".repeat(length);
        ruby_steps(2, |i| {
            let suffix = ["b", "c"][i];
            format!("Given('{prefix}{suffix}') {{ shared_implementation() }}")
        })
    };
    // 10,002 action events per handler exceed the same ceiling for the handler matrix; differing
    // final calls keep the handlers out of the identical and structural sources.
    let handlers = |events: usize| {
        let body = "page.shared_event\n".repeat(events);
        ruby_steps(2, |i| {
            let (noun, last) = [("account is", "first"), ("accounts are", "second")][i];
            format!("Given('the {noun} enabled') do\n{body}{last}\nend")
        })
    };
    for (source, kind, refused) in [
        (matchers(10_010), "identicalHandler", true),
        (matchers(1_010), "identicalHandler", false),
        (handlers(10_000), "matcherBlocking", true),
        (handlers(10), "matcherBlocking", false),
    ] {
        let (rows, stderr) = ruby_limited_run(&source, 100, 100);
        let analysis = &rows.last().unwrap()["analysis"];
        let similar = rows.iter().any(|row| {
            matches!(
                row["rule"].as_str(),
                Some("duplicate-handler" | "near-duplicate-step")
            )
        });
        assert_eq!(analysis["truncated"], refused, "{kind}");
        assert_eq!(
            analysis["candidateSources"][kind]["skipped"],
            u64::from(refused),
            "{kind}"
        );
        assert_eq!(
            analysis["candidateSources"][kind]["evaluated"],
            u64::from(!refused),
            "{kind}"
        );
        assert_eq!(similar, !refused, "{kind}");
        assert_eq!(stderr.contains("after safety limits"), refused, "{kind}");
    }
}

/// A Ruby registration with a dynamic matcher leaves the corpus incomplete: the default run
/// passes, strict mode from the CLI or config fails, and no baseline is written.
#[test]
fn ruby_dynamic_matcher_cannot_pass_strict_mode_or_create_a_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(
        root,
        "steps.rb",
        "Given('known') { work() }\nGiven(build_pattern) { work() }\n",
    );
    ruby_write(
        root,
        "suite.feature",
        "Feature: Usage\n Scenario: Known\n  Given known\n",
    );
    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{"definitions":["steps.rb"],"features":["*.feature"],"reporters":["json"],"output":"reports","noMetrics":true}"#,
    );
    let run = || {
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command.current_dir(root).arg(".");
        command
    };
    run().assert().code(0).stderr(predicate::str::contains(
        "steps.rb:2:1: Ruby registration requires a static matcher",
    ));
    let report = ruby_report(&root.join("reports/cuke-dedup.json"));
    assert_eq!(report["summary"]["definitionsAnalyzed"], 1);
    assert_eq!(report["corpus"]["incomplete"], true);
    run().arg("--fail-on-incomplete").assert().code(2);
    run()
        .args(["--baseline", "baseline.json", "--update-baseline"])
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "was not updated because analysis is incomplete",
        ));
    assert!(!root.join("baseline.json").exists());
    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{"definitions":["steps.rb"],"features":["*.feature"],"reporters":["json"],"failOnIncomplete":true}"#,
    );
    run().assert().code(2);
}

/// Ruby sources without an opting-in definition pattern leave default discovery visibly empty,
/// name the `.rb` opt-in fix, fail under `--require-definitions`, and are analyzed once opted in.
#[test]
fn ruby_default_discovery_is_visibly_empty_and_names_the_opt_in_pattern() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(
        root,
        "example.feature",
        "Feature: Empty\n  Scenario: No implementation\n    Given a missing step\n",
    );
    ruby_write(root, "steps.rb", "Given('a ruby step') { work() }\n");
    let guidance = "1 cucumber-ruby source file(s) were not analyzed: opt-in languages need a definition pattern that names their suffix (`.rb`)";
    let default = || {
        let mut command = Command::cargo_bin("cuke-dedup").unwrap();
        command.current_dir(root).arg(".");
        command
    };
    default()
        .assert()
        .success()
        .stdout(predicate::str::contains("Analyzed 0 definitions"))
        .stderr(predicate::str::contains(
            "automatic discovery found no supported step-definition source files",
        ))
        .stderr(predicate::str::contains(guidance));
    default()
        .arg("--require-definitions")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(guidance))
        .stderr(predicate::str::contains(
            "no step definitions were extracted because no definition source files were discovered",
        ));
    ruby_cli(root, "*.rb")
        .assert()
        .success()
        .stdout(predicate::str::contains("Analyzed 1 definition"))
        .stderr(predicate::str::contains("cucumber-ruby source file(s) were not analyzed").not());
}

/// Runs the binary from `root` on `.` with `--explain-discovery`, JSONL output and `args`;
/// returns the exit code, the records and every stderr line without its `cuke-dedup: ` prefix,
/// separators normalized to `/`, except the always-printed feature-pattern header.
fn ruby_explained(root: &Path, args: &[&str]) -> (i32, Vec<Value>, Vec<String>) {
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root)
        .args([".", "--explain-discovery", "--reporters", "jsonl"])
        .args(["--no-metrics"])
        .args(args)
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    let lines = stderr
        .lines()
        .map(|line| line.strip_prefix("cuke-dedup: ").expect(&stderr))
        .filter(|line| !line.starts_with("feature patterns: "))
        .map(|line| line.replace('\\', "/"))
        .collect();
    let code = output.status.code().expect(&stderr);
    (code, records(output.stdout), lines)
}

/// `directory/pattern` built at run time: a slash-star sequence inside a string literal opens a
/// comment for the duplication tokenizer.
fn ruby_glob(directory: &str, pattern: &str) -> String {
    format!("{directory}/{pattern}")
}

/// `definition <path>` explain lines for `paths`, in the given order.
fn ruby_definition_lines(paths: &[&str]) -> Vec<String> {
    paths
        .iter()
        .map(|path| format!("definition {path}"))
        .collect()
}

/// Ruby definition selection: `.gitignore` and scoped `.cuke-dedupignore` rules with negation
/// prune Ruby sources and removing them restores every source; `*` crosses `/`; braces select
/// classic and Markdown features with their parser, and the selected Ruby definitions are used by
/// both formats; definition globs, braces included, narrow the Ruby sources; an unmatched
/// definition or feature glob is reported once by name, each of several unmatched definition
/// globs is reported once beside a matched one, and a feature glob whose only match is
/// attributed to an earlier overlapping glob is not reported.
#[test]
fn ruby_discovery_applies_ignore_files_and_glob_semantics_to_ruby_sources() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let sources = [
        "features/steps/example.rb",
        "keep.generated.rb",
        "packages/second/local.rb",
        "ignored/step.rb",
        "skipped/step.rb",
        "drop.generated.rb",
        "packages/first/local.rb",
    ];
    for (index, path) in sources.iter().enumerate() {
        ruby_write(
            root,
            path,
            &format!("Given('step {index}') {{ action_{index}() }}\n"),
        );
    }
    ruby_write(root, ".gitignore", "ignored/\n");
    ruby_write(
        root,
        ".cuke-dedupignore",
        "skipped/\n*.generated.rb\n!keep.generated.rb\n",
    );
    ruby_write(root, "packages/first/.cuke-dedupignore", "local.rb\n");
    ruby_write(
        root,
        "features/a.feature",
        "Feature: A\n  Scenario: S\n    Given step 0\n",
    );
    let markdown = "# Feature: B\n\n## Scenario: S\n\n* Given step 1\n";
    ruby_write(root, "features/nested/b.feature.md", markdown);
    // Uses `step 2`: selecting this non-feature Markdown file would hide the one unused finding.
    ruby_write(root, "features/c.md", &markdown.replace("step 1", "step 2"));

    let brace = ruby_glob("features", "*.{feature,feature.md}");
    let classic = "feature features/a.feature [gherkin] via *.feature";
    let both_formats = [
        classic.to_owned(),
        format!("feature features/nested/b.feature.md [gherkin-markdown] via {brace}"),
    ];
    let selected = [
        "features/steps/example.rb",
        "keep.generated.rb",
        "packages/second/local.rb",
    ];
    let unmatched = ruby_glob("stpes", "*.rb");
    let also_unmatched = ruby_glob("support", "*.rb");
    let packages = ruby_glob("packages", "*.rb");
    let both_directories = ruby_glob("{features,packages}", "*.rb");
    // Definition globs, feature globs, explain lines, unused-definition locations. Explain lines
    // omit the `feature patterns:` header that `ruby_explained` drops.
    type Row<'a> = (Vec<&'a str>, Vec<&'a str>, Vec<String>, Vec<&'a str>);
    let rows: Vec<Row> = vec![
        (
            vec!["*.rb"],
            vec!["*.feature", &brace],
            [&both_formats[..], &ruby_definition_lines(&selected)].concat(),
            vec!["packages/second/local.rb:1"],
        ),
        (
            vec![&both_directories],
            vec!["*.feature", &brace],
            [
                &both_formats[..],
                &ruby_definition_lines(&["features/steps/example.rb", "packages/second/local.rb"]),
            ]
            .concat(),
            vec!["packages/second/local.rb:1"],
        ),
        (
            vec![&packages],
            vec!["*.feature"],
            [classic.to_owned(), "definition packages/second/local.rb".to_owned()].to_vec(),
            vec!["packages/second/local.rb:1"],
        ),
        (
            vec![&unmatched],
            vec!["*.feature"],
            vec![
                classic.to_owned(),
                format!("warning: definition pattern `{unmatched}` matched no files"),
            ],
            vec![],
        ),
        // One matched and two unmatched definition globs: one warning per unmatched glob only.
        (
            vec![&packages, &unmatched, &also_unmatched],
            vec!["*.feature"],
            vec![
                classic.to_owned(),
                "definition packages/second/local.rb".to_owned(),
                format!("warning: definition pattern `{unmatched}` matched no files"),
                format!("warning: definition pattern `{also_unmatched}` matched no files"),
            ],
            vec!["packages/second/local.rb:1"],
        ),
        // Overlap: `*a.feature` matches only `features/a.feature`, which `*.feature` selects first.
        (
            vec!["*.rb"],
            vec!["*.feature", "*a.feature"],
            [&[classic.to_owned()][..], &ruby_definition_lines(&selected)].concat(),
            vec!["keep.generated.rb:1", "packages/second/local.rb:1"],
        ),
        (
            vec!["*.rb"],
            vec!["*.feature", "*missing.feature"],
            [
                &[classic.to_owned()][..],
                &ruby_definition_lines(&selected),
                &["warning: feature pattern `*missing.feature` from .cuke-dedup.json matched no files"
                    .to_owned()],
            ]
            .concat(),
            vec!["keep.generated.rb:1", "packages/second/local.rb:1"],
        ),
    ];
    for (definitions, features, expected, unused) in rows {
        let config = serde_json::json!({ "definitions": definitions, "features": features });
        ruby_write(root, ".cuke-dedup.json", &config.to_string());
        let (code, rows, lines) = ruby_explained(root, &[]);
        assert_eq!((code, &lines), (0, &expected), "{config}");
        assert_eq!(
            ruby_rule_locations(&rows, "unused-definition"),
            unused,
            "{config}"
        );
    }

    for ignore in [
        ".gitignore",
        ".cuke-dedupignore",
        "packages/first/.cuke-dedupignore",
    ] {
        fs::remove_file(root.join(ignore)).unwrap();
    }
    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{"definitions":["*.rb"],"features":["*.feature"]}"#,
    );
    let mut every = sources.to_vec();
    every.sort_unstable();
    let (code, _, lines) = ruby_explained(root, &[]);
    let expected = [&[classic.to_owned()][..], &ruby_definition_lines(&every)].concat();
    assert_eq!((code, lines), (0, expected));
}

/// Ruby selection under exclude layers, each measured against a baseline config that admits every
/// non-hidden source outside `node_modules`: a `package.json`-only `exclude` and a standalone
/// config's `exclude` each hide their own source, a CLI `--exclude` replaces the config `exclude`
/// while the built-in `node_modules` exclusion stays until `--no-default-excludes`, and the
/// `excludeDefaults` and `includeHidden` config keys each admit exactly their one Ruby source.
#[test]
fn ruby_exclude_layers_and_config_escape_hatches_select_ruby_sources() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let sources = [
        ".hidden/e.rb",
        "cli-only/c.rb",
        "config-only/b.rb",
        "keep.rb",
        "node_modules/pkg/d.rb",
        "package-only/a.rb",
    ];
    for (index, path) in sources.iter().enumerate() {
        ruby_write(
            root,
            path,
            &format!("Given('layer {index}') {{ layer_{index}() }}\n"),
        );
    }
    ruby_write(
        root,
        "other.feature",
        "Feature: Other\n  Scenario: One\n    Given other step\n",
    );
    let baseline = [
        "cli-only/c.rb",
        "config-only/b.rb",
        "keep.rb",
        "package-only/a.rb",
    ];
    let without = |hidden: &str| -> Vec<&str> {
        baseline
            .iter()
            .copied()
            .filter(|path| !path.starts_with(hidden))
            .collect()
    };
    let adding = |added| {
        let mut admitted = [&baseline[..], &[added]].concat();
        admitted.sort_unstable();
        admitted
    };
    let selection = r#""definitions":["*.rb"],"features":["*.feature"]"#;
    let config_exclude = r#","exclude":["config-only"]"#;
    let cli_exclude = ["--exclude", "cli-only"];
    // `package.json#cukeDedup` settings, standalone config settings, CLI arguments, admitted
    // sources. A standalone config replaces the whole `package.json` block, so each layer is
    // measured with the other absent.
    type Row<'a> = (Option<&'a str>, Option<&'a str>, Vec<&'a str>, Vec<&'a str>);
    let rows: Vec<Row> = vec![
        (None, Some(""), vec![], baseline.to_vec()),
        (
            Some(r#","exclude":["package-only"]"#),
            None,
            vec![],
            without("package-only"),
        ),
        (None, Some(config_exclude), vec![], without("config-only")),
        (
            None,
            Some(config_exclude),
            cli_exclude.to_vec(),
            without("cli-only"),
        ),
        (
            None,
            Some(config_exclude),
            [&cli_exclude[..], &["--no-default-excludes"]].concat(),
            [
                "config-only/b.rb",
                "keep.rb",
                "node_modules/pkg/d.rb",
                "package-only/a.rb",
            ]
            .to_vec(),
        ),
        (
            None,
            Some(r#","excludeDefaults":false"#),
            vec![],
            adding("node_modules/pkg/d.rb"),
        ),
        (
            None,
            Some(r#","includeHidden":true"#),
            vec![],
            adding(".hidden/e.rb"),
        ),
        (
            None,
            Some(r#","excludeDefaults":false,"includeHidden":true"#),
            vec![],
            sources.to_vec(),
        ),
    ];
    for (package, standalone, args, admitted) in rows {
        let package =
            package.map(|settings| format!(r#"{{"cukeDedup":{{{selection}{settings}}}}}"#));
        let standalone = standalone.map(|settings| format!("{{{selection}{settings}}}"));
        for (path, content) in [
            ("package.json", &package),
            ("cuke-dedup.config.json", &standalone),
        ] {
            match content {
                Some(content) => ruby_write(root, path, content),
                None if root.join(path).exists() => fs::remove_file(root.join(path)).unwrap(),
                None => {}
            }
        }
        let (code, rows, lines) = ruby_explained(root, &args);
        let context = format!("{package:?} {standalone:?} {args:?}");
        let mut expected = vec!["feature other.feature [gherkin] via *.feature".to_owned()];
        expected.extend(ruby_definition_lines(&admitted));
        assert_eq!((code, lines), (0, expected), "{context}");
        let unused: Vec<String> = admitted.iter().map(|path| format!("{path}:1")).collect();
        assert_eq!(
            ruby_rule_locations(&rows, "unused-definition"),
            unused,
            "{context}"
        );
    }
}

/// Asserts explain `lines` equal `expected`. On Windows a definition reached only through a load
/// prints its verbatim (`\\?\`) canonical path, because the analysis root is normalized and the
/// load target is not, so there each expected definition matches a printed line ending in it.
fn ruby_assert_explained(lines: &[String], expected: &[String], context: &str) {
    #[cfg(windows)]
    {
        assert_eq!(lines.len(), expected.len(), "{context}: {lines:?}");
        for line in expected {
            let suffix = line
                .strip_prefix("definition ")
                .map(|path| format!("/{path}"));
            assert!(
                lines.iter().any(|printed| printed == line
                    || suffix.as_deref().is_some_and(|suffix| {
                        printed.starts_with("definition ") && printed.ends_with(suffix)
                    })),
                "{context}: `{line}` not in {lines:?}"
            );
        }
    }
    #[cfg(not(windows))]
    assert_eq!(lines, expected, "{context}");
}

/// Ruby loads cannot leave their root: `require_relative` through `..` and an explicit `./..`
/// require are refused against the analysis root, and a load-path require through `..` is refused
/// against its declared load path, each with a located unresolved-dependency warning and the
/// outside definition unanalyzed, while the same load form inside its root is followed.
#[test]
fn ruby_source_loads_cannot_escape_the_analysis_root() {
    let sandbox = tempfile::tempdir().unwrap();
    let root = sandbox.path().join("root");
    // Declared outside the analysis root, so only the load-path containment arm admits its files.
    let load_path = sandbox.path().join("lib");
    ruby_write(
        sandbox.path(),
        "outside/world.rb",
        "Given('outside step') { outside() }\n",
    );
    ruby_write(
        &load_path,
        "x/world.rb",
        "Given('load path step') { load_path() }\n",
    );
    ruby_write(
        &root,
        "inside/world.rb",
        "Given('inside step') { inside() }\n",
    );
    ruby_write(
        &root,
        "other.feature",
        "Feature: Other\n  Scenario: One\n    Given other step\n",
    );
    let loaded = load_path.canonicalize().unwrap().join("x").join("world.rb");
    let loaded = loaded.to_string_lossy().replace('\\', "/");
    let feature = "feature other.feature [gherkin] via *.feature".to_owned();
    let followed = |path: &str| {
        [
            &[feature.clone()][..],
            &ruby_definition_lines(&[path, "steps.rb"]),
        ]
        .concat()
    };
    let refused = vec![
        feature.clone(),
        "definition steps.rb".to_owned(),
        "warning: steps.rb:2:1: Ruby source dependency is unresolved".to_owned(),
        "warning: Ruby indirect step usage is unresolved; unused-definition findings are disabled"
            .to_owned(),
    ];
    // Load, whether `lib` is a declared load path, expected explain lines.
    let rows = [
        (
            "require_relative 'inside/world'",
            false,
            followed("inside/world.rb"),
        ),
        (
            "require_relative '../outside/world'",
            false,
            refused.clone(),
        ),
        (
            "require './inside/world'",
            false,
            followed("inside/world.rb"),
        ),
        ("require './../outside/world'", false, refused.clone()),
        ("require 'x/world'", true, followed(&loaded)),
        ("require 'x/../../outside/world'", true, refused),
    ];
    let config = root.join(".cuke-dedup.json");
    for (load, declared, expected) in rows {
        if declared {
            let paths = serde_json::json!({ "rubyLoadPaths": [&load_path] });
            ruby_write(&root, ".cuke-dedup.json", &paths.to_string());
        } else if config.exists() {
            fs::remove_file(&config).unwrap();
        }
        // The load sits on line 2 so a fallback location cannot match the warning.
        ruby_write(
            &root,
            "steps.rb",
            &format!("# entry\n{load}\nGiven('entry step') {{ entry() }}\n"),
        );
        let (code, rows, lines) = ruby_explained(
            &root,
            &["--definitions", "steps.rb", "--features", "*.feature"],
        );
        assert_eq!(code, 0, "{load}: {lines:?}");
        ruby_assert_explained(&lines, &expected, load);
        let summary = rows.last().unwrap();
        let definitions = expected
            .iter()
            .filter(|line| line.starts_with("definition "));
        assert_eq!(
            summary["summary"]["definitionsAnalyzed"],
            definitions.count(),
            "{load}"
        );
    }
}

/// An unreadable directory under a Ruby suite is reported as a traversal error that fails the run,
/// while the readable Ruby source is still discovered and analyzed.
#[cfg(unix)]
#[test]
fn ruby_traversal_errors_are_reported_without_dropping_readable_sources() {
    use std::os::unix::fs::PermissionsExt;

    /// Restores the locked directory's permissions on drop so a failed assertion cannot leave the
    /// temporary directory undeletable.
    struct Unlock(std::path::PathBuf);
    impl Drop for Unlock {
        /// Makes the locked directory readable and removable again.
        fn drop(&mut self) {
            if let Err(error) = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700)) {
                eprintln!("cannot unlock {}: {error}", self.0.display());
            }
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(root, "visible.rb", "Given('visible step') { visible() }\n");
    ruby_write(
        root,
        "locked/hidden.rb",
        "Given('locked step') { locked() }\n",
    );
    ruby_write(
        root,
        "other.feature",
        "Feature: Other\n  Scenario: One\n    Given other step\n",
    );
    let locked = Unlock(root.join("locked"));
    fs::set_permissions(&locked.0, fs::Permissions::from_mode(0o000)).unwrap();
    // Permission bits do not bind a privileged user such as root; with the directory still
    // readable there is no traversal error to observe.
    if fs::read_dir(&locked.0).is_ok() {
        return;
    }
    let (code, rows, lines) =
        ruby_explained(root, &["--definitions", "*.rb", "--features", "*.feature"]);

    assert_eq!(code, 2, "{lines:?}");
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(
        lines[..2],
        [
            "feature other.feature [gherkin] via *.feature",
            "definition visible.rb"
        ]
    );
    assert!(
        lines[2].starts_with("failed while walking target directory ")
            && lines[2].contains("/locked: "),
        "{lines:?}"
    );
    assert_eq!(
        ruby_rule_locations(&rows, "unused-definition"),
        ["visible.rb:1"]
    );
}

/// Feature-corpus requirements over Ruby definitions: a missing corpus disables unused findings
/// unless required, malformed features are summarized or fatal, an empty Markdown feature marks
/// the corpus incomplete, and an empty classic feature does not hide unused definitions.
#[test]
fn ruby_feature_corpus_requirements_and_empty_features_are_visible() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(root, "steps.rb", "Given('working step') { work() }\n");
    ruby_cli(root, "*.rb")
        .assert()
        .success()
        .stdout(predicate::str::contains("unused-definition").not())
        .stderr(predicate::str::contains(
            "unused-definition findings are disabled",
        ));
    ruby_cli(root, "*.rb")
        .arg("--require-features")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no feature files matched"));

    ruby_write(
        root,
        "broken.feature",
        "Scenario: Missing feature\n  Given a step\n",
    );
    ruby_cli(root, "*.rb")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("failed to parse Gherkin feature"));
    ruby_write(
        root,
        "valid.feature",
        "Feature: Valid\n  Scenario: One\n    Given working step\n",
    );
    ruby_cli(root, "*.rb")
        .arg("--require-features")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "1 discovered feature file(s) could not be parsed",
        ));

    let markdown = tempfile::tempdir().unwrap();
    let root = markdown.path();
    ruby_write(root, "steps.rb", "Given('una cuenta activa') { work() }\n");
    ruby_write(
        root,
        "features/account.feature.md",
        "# Característica: Cuenta\n\n## Escenario: Disponible\n\n* Dado una cuenta activa\n",
    );
    let assertion = ruby_cli(root, "*.rb")
        .args(["--reporters", "json,jsonl,html,sarif"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unused-definition").not())
        .stderr(predicate::str::contains(
            "Gherkin Markdown file features/account.feature.md parsed successfully but produced 0 feature steps",
        ));
    let report = ruby_report(&root.join("reports/cuke-dedup/cuke-dedup.json"));
    assert_eq!(
        report["summary"]["byRule"]["unused-definition"],
        Value::Null
    );
    ruby_assert_pointers(
        &report,
        &[
            ("/summary/definitionsAnalyzed", 1.into()),
            ("/corpus/featureFiles", 1.into()),
            ("/corpus/featureFilesParsed", 1.into()),
            ("/corpus/featureFilesWithoutSteps", 1.into()),
            ("/corpus/incomplete", true.into()),
        ],
    );
    let summary = records(assertion.get_output().stdout.clone())
        .pop()
        .unwrap();
    ruby_assert_pointers(
        &summary["corpus"],
        &[
            ("/featureFilesWithoutSteps", 1.into()),
            ("/incomplete", true.into()),
        ],
    );
    let html = fs::read_to_string(root.join("reports/cuke-dedup/cuke-dedup.html")).unwrap();
    assert!(html.contains("Corpus is incomplete"));
    assert!(html.contains("\"featureFilesWithoutSteps\":1"));
    let sarif = ruby_report(&root.join("reports/cuke-dedup/cuke-dedup.sarif"));
    ruby_assert_pointers(
        &sarif["runs"][0]["invocations"][0],
        &[
            ("/properties/corpus/featureFilesWithoutSteps", 1.into()),
            ("/executionSuccessful", false.into()),
        ],
    );
    ruby_cli(root, "*.rb")
        .args(["--reporters", "json", "--fail-on-incomplete"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "input extraction did not fully represent every discovered definition or feature file",
        ));

    let classic = tempfile::tempdir().unwrap();
    let root = classic.path();
    ruby_write(
        root,
        "steps.rb",
        "Given('used step') { used() }\nGiven('unused step') { unused() }\n",
    );
    ruby_write(
        root,
        "features/placeholder.feature",
        "Feature: Planned work\n",
    );
    ruby_write(
        root,
        "features/active.feature",
        "Feature: Active\n  Scenario: Current\n    Given used step\n",
    );
    ruby_cli(root, "*.rb")
        .args(["--reporters", "json"])
        .assert()
        .success()
        .stderr(predicate::str::contains("produced 0 feature steps").not());
    let report = ruby_report(&root.join("reports/cuke-dedup/cuke-dedup.json"));
    assert_eq!(report["summary"]["byRule"]["unused-definition"], 1);
    assert_eq!(report["corpus"]["featureFiles"], 2);
    assert_eq!(report["corpus"]["featureFilesWithoutSteps"], 0);
    assert_eq!(report["corpus"]["incomplete"], false);
}

/// Definition-side requirements over Ruby sources: zero extracted definitions can be required
/// while reports are still written, unmatched patterns and suppressions are visible, and
/// diagnostics leave the valid definitions analyzed.
#[test]
fn ruby_definition_requirements_and_source_diagnostics_are_visible() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(root, "steps.rb", "HELPER = true\n");
    ruby_write(
        root,
        "example.feature",
        "Feature: Completeness\n  Scenario: Missing\n    Given invisible step\n",
    );
    ruby_cli(root, "*.rb")
        .args([
            "--require-definitions",
            "--reporters",
            "json",
            "--output",
            "reports",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "definition extraction produced 0 definitions from 1 discovered definition source file(s)",
        ));
    assert!(root.join("reports/cuke-dedup.json").is_file());
    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{"definitions":["*.rb"],"requireDefinitions":true,"reporters":["json"],"output":"configured-report"}"#,
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root)
        .arg(".")
        .assert()
        .code(2);
    assert!(root.join("configured-report/cuke-dedup.json").is_file());

    let unmatched = tempfile::tempdir().unwrap();
    ruby_write(
        unmatched.path(),
        "steps.rb",
        "Given('present step') { work() }\n",
    );
    ruby_write(
        unmatched.path(),
        ".cuke-dedup.json",
        r#"{
  "definitions": ["missing/**/*.rb"],
  "suppressions": [{
    "rule": "duplicate-matcher",
    "matcher": "missing step",
    "reason": "migration exception"
  }]
}"#,
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(unmatched.path())
        .arg(".")
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "definition pattern `missing/**/*.rb` matched no files",
        ))
        .stderr(predicate::str::contains(
            "suppression 1 for duplicate-matcher matched no step definitions",
        ));

    let diagnostics = tempfile::tempdir().unwrap();
    ruby_write(
        diagnostics.path(),
        "steps.rb",
        "# diagnostics start below line 1\nGiven(/value (?=ahead)/) { advanced() }\nGiven('valid step') { valid() }\nGiven(\"broken \\xZZ\") { broken() }\n",
    );
    ruby_write(
        diagnostics.path(),
        "example.feature",
        "Feature: Partial\n  Scenario: Valid\n    Given valid step\n",
    );
    for (strict, code) in [(false, 0), (true, 2)] {
        let mut command = ruby_cli(diagnostics.path(), "*.rb");
        if strict {
            command.arg("--fail-on-unparseable");
        }
        command
            .assert()
            .code(code)
            .stdout(predicate::str::contains("Analyzed 2 definitions"))
            .stderr(predicate::str::contains(
                "steps.rb:2:1: Ruby regular expression is outside the supported static matching subset",
            ))
            .stderr(predicate::str::contains("Ruby source contains syntax errors"));
    }
}

/// Oversized inputs beside Ruby sources fail closed whatever the reporter, while an oversized
/// Ruby Cucumber Expression only warns and the report is still written.
#[test]
fn ruby_resource_limits_fail_closed_independently_of_reporters() {
    let over_config_limit = 1024 * 1024 + 1;
    let over_source_limit = 8 * 1024 * 1024 + 1;
    let fails_over_limit =
        |kind: &str| predicate::str::contains(kind).and(predicate::str::contains("input limit"));
    for (relative, bytes, arguments, kind) in [
        (
            "package.json",
            over_config_limit,
            &[][..],
            "package configuration",
        ),
        (
            ".cuke-dedup.json",
            over_config_limit,
            &[],
            "CukeDedup configuration",
        ),
        (
            "playwright.config.ts",
            over_config_limit,
            &[],
            "framework configuration",
        ),
        (
            "baseline.json",
            over_source_limit,
            &["--baseline", "baseline.json"],
            "baseline",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        ruby_write(directory.path(), "steps.rb", RUBY_DUPLICATE_PAIR);
        ruby_write_sized(directory.path(), relative, bytes);
        ruby_cli(directory.path(), "*.rb")
            .args(arguments)
            .assert()
            .code(2)
            .stderr(fails_over_limit(kind));
    }

    let directory = tempfile::tempdir().unwrap();
    ruby_write_sized(directory.path(), "steps.rb", over_source_limit);
    for reporter in ["terminal", "json", "jsonl", "html", "sarif"] {
        let output = tempfile::tempdir().unwrap();
        ruby_cli(directory.path(), "*.rb")
            .args(["--reporters", reporter])
            .arg("--output")
            .arg(output.path())
            .assert()
            .code(2)
            .stderr(fails_over_limit("definition source"));
    }

    let directory = tempfile::tempdir().unwrap();
    let output = tempfile::tempdir().unwrap();
    ruby_write(
        directory.path(),
        "steps.rb",
        &format!("Given('{}') {{ work() }}\n", "a".repeat(1024 * 1024 + 1)),
    );
    ruby_cli(directory.path(), "*.rb")
        .args(["--reporters", "json"])
        .arg("--output")
        .arg(output.path())
        .assert()
        .code(0)
        .stderr(
            predicate::str::contains("warning: step matcher at")
                .and(predicate::str::contains("regex resource limit")),
        );
    assert_eq!(
        ruby_report(&output.path().join("cuke-dedup.json"))["summary"]["definitionsAnalyzed"],
        1
    );
}

/// Runs `ruby_cli` with JSONL output; returns the exit code, the last record (`Null` when the run
/// wrote none) and stderr, so a run that fails before reporting still shows why.
fn ruby_summary_run(root: &Path, pattern: &str, extra: &[&str]) -> (i32, Value, String) {
    let output = ruby_cli(root, pattern)
        .args(["--reporters", "jsonl", "--no-metrics"])
        .args(extra)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let last = records(output.stdout).pop().unwrap_or(Value::Null);
    (output.status.code().unwrap_or(-1), last, stderr)
}

/// Counts every `cuke-dedup:` diagnostic line in `stderr`, warnings and strict-mode errors alike.
fn ruby_diagnostic_count(stderr: &str) -> usize {
    stderr
        .lines()
        .filter(|line| line.starts_with("cuke-dedup:"))
        .count()
}

/// The Ruby load budget of 1,024 dependency-only files is one budget across every selected
/// definition file: each file's 520 loads fit alone, and together the loads past 1,024 are not
/// followed and mark the run incomplete.
#[test]
fn ruby_load_budget_is_shared_across_selected_definition_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for group in ["a", "b"] {
        let mut entry = String::new();
        for index in 0..520 {
            ruby_write(
                root,
                &format!("support/{group}-{index}.rb"),
                &format!("LOADED_{}_{index} = 1\n", group.to_uppercase()),
            );
            entry.push_str(&format!("require_relative 'support/{group}-{index}'\n"));
        }
        entry.push_str(&format!("Given('step {group}') {{ work('{group}') }}\n"));
        ruby_write(root, &format!("steps-{group}.rb"), &entry);
    }
    ruby_write(
        root,
        "budget.feature",
        "Feature: Budget\n  Scenario: Both\n    Given step a\n    Given step b\n",
    );
    let refused =
        "Ruby source dependency was not followed: the source graph exceeds the 1,024-file limit";
    for alone in ["steps-a.rb", "steps-b.rb"] {
        let (code, summary, stderr) = ruby_summary_run(root, alone, &["--fail-on-incomplete"]);
        assert_eq!(code, 0, "{alone}: {stderr}");
        assert_eq!(ruby_diagnostic_count(&stderr), 0, "{alone}: {stderr}");
        assert_eq!(summary["corpus"]["incomplete"], false, "{alone}");
    }
    let both = "steps-a.rb,steps-b.rb";
    let (code, summary, stderr) = ruby_summary_run(root, both, &[]);
    assert_eq!(code, 0, "{stderr}");
    // 1,024 - 520 = 504 loads of `steps-b.rb` fit; lines 505..=520 are refused, one warning each,
    // plus the indirect-usage warning the incomplete graph causes.
    assert_eq!(stderr.matches(refused).count(), 16, "{stderr}");
    for line in [505, 520] {
        assert!(
            stderr.contains(&format!("steps-b.rb:{line}:1: {refused}")),
            "{stderr}"
        );
    }
    assert_eq!(ruby_diagnostic_count(&stderr), 17, "{stderr}");
    ruby_assert_pointers(
        &summary,
        &[
            ("/corpus/definitionFiles", 1026.into()),
            ("/corpus/incomplete", true.into()),
            ("/summary/definitionsAnalyzed", 2.into()),
        ],
    );
    let (strict, _, _) = ruby_summary_run(root, both, &["--fail-on-incomplete"]);
    assert_eq!(strict, 2);
}

/// The matcher-overlap candidate limit over Ruby Cucumber Expressions is visible in the report and
/// fails closed under `--fail-on-incomplete`; candidate-limit flags accept only positive integers.
#[test]
fn ruby_matcher_overlap_limit_is_visible_and_candidate_flags_must_be_positive() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(
        root,
        "steps.rb",
        "Given('value {first}') { }\nGiven('value {second}') { }\nGiven('value {third}') { }\n",
    );
    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{"threshold":100,"reporters":["json"],"output":"reports","noMetrics":true,
            "maxCandidateComparisons":1,
            "parameterTypes":{"first":"red|green","second":"red|green","third":"red|green"}}"#,
    );
    let incomplete =
        "static matcher-overlap analysis is incomplete: evaluated 1 definition comparisons";
    // `7` lifts the configured limit of 1 above the 3 pairwise comparisons: the control.
    for (extra, strict_code, evaluated, skipped) in [
        (None, 2, 1, 1),
        (Some("--max-candidate-comparisons=7"), 0, 3, 0),
    ] {
        for strict in [false, true] {
            let mut command = ruby_cli(root, "*.rb");
            command.args(extra);
            if strict {
                command.arg("--fail-on-incomplete");
            }
            let output = command.output().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            let code = if strict { strict_code } else { 0 };
            assert_eq!(output.status.code(), Some(code), "{extra:?}: {stderr}");
            assert_eq!(stderr.contains(incomplete), skipped > 0, "{stderr}");
        }
        let report = ruby_report(&root.join("reports/cuke-dedup.json"));
        ruby_assert_pointers(
            &report["analysis"],
            &[
                ("/truncated", (skipped > 0).into()),
                (
                    "/candidateSources/matcherOverlap/evaluated",
                    evaluated.into(),
                ),
                ("/candidateSources/matcherOverlap/skipped", skipped.into()),
            ],
        );
    }
    let printed = ruby_cli(root, "*.rb")
        .args([
            "--print-config",
            "--max-candidate-comparisons=7",
            "--max-structural-class-comparisons=7",
        ])
        .output()
        .unwrap();
    assert_eq!(printed.status.code(), Some(0));
    let config: Value = serde_json::from_slice(&printed.stdout).unwrap();
    ruby_assert_pointers(
        &config,
        &[
            ("/maxCandidateComparisons", 7.into()),
            ("/maxStructuralClassComparisons", 7.into()),
        ],
    );
    for flag in [
        "--max-candidate-comparisons",
        "--max-structural-class-comparisons",
    ] {
        for value in ["0", "-1", "many"] {
            ruby_cli(root, "*.rb")
                .arg(format!("{flag}={value}"))
                .assert()
                .code(2)
                .stderr(predicate::str::contains(format!(
                    "invalid value '{value}' for '{flag} <COUNT>': expected an integer greater than zero"
                )));
        }
    }
}

/// An explicitly excluded oversized Ruby file is outside the corpus, so the run stays complete
/// under `--fail-on-incomplete`; without the exclusion it fails on the input limit. A clean Ruby
/// corpus is complete, one dynamic matcher makes it incomplete, and hidden and default-excluded
/// Ruby sources each need their own discovery flag.
#[test]
fn ruby_exclusions_and_discovery_flags_define_the_analysis_corpus() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write_sized(root, "generated/oversized.rb", 8 * 1024 * 1024 + 1);
    ruby_write(root, "steps.rb", "Given('included step') { work() }\n");
    ruby_write(
        root,
        "included.feature",
        "Feature: Included\n  Scenario: One\n    Given included step\n",
    );
    for (config, code) in [(r#"{"exclude":["generated/**"]}"#, 0), ("{}", 2)] {
        ruby_write(root, ".cuke-dedup.json", config);
        let output = ruby_cli(root, "**/*.rb")
            .args(["--threshold", "100", "--fail-on-incomplete"])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(code), "{config}: {stderr}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("Analyzed 1 definition and"));
        let excluded = code == 0;
        assert_eq!(stderr.contains("input limit"), !excluded, "{stderr}");
        if excluded {
            assert_eq!(ruby_diagnostic_count(&stderr), 0, "{stderr}");
        }
    }

    // Completeness: the clean corpus and the dynamic-matcher corpus differ only in the
    // registration whose matcher cannot be resolved statically.
    let completeness = tempfile::tempdir().unwrap();
    let root = completeness.path();
    ruby_write(
        root,
        "used.feature",
        "Feature: Used\n  Scenario: One\n    Given static step\n",
    );
    for (source, incomplete) in [
        ("Given('static step') { work() }\n", false),
        (
            "Given('static step') { work() }\nname = 'dynamic step'\nGiven(name) { }\n",
            true,
        ),
    ] {
        ruby_write(root, "steps.rb", source);
        let (strict, summary, stderr) = ruby_summary_run(root, "*.rb", &["--fail-on-incomplete"]);
        assert_eq!(strict, if incomplete { 2 } else { 0 }, "{stderr}");
        assert_eq!(summary["corpus"]["incomplete"], incomplete, "{stderr}");
        assert_eq!(
            stderr.contains("steps.rb:3:1: Ruby registration requires a static matcher"),
            incomplete,
            "{stderr}"
        );
        assert_eq!(
            ruby_diagnostic_count(&stderr),
            if incomplete { 2 } else { 0 },
            "{stderr}"
        );
        let (lenient, _, stderr) = ruby_summary_run(root, "*.rb", &[]);
        assert_eq!(lenient, 0, "{stderr}");
    }

    let discovery = tempfile::tempdir().unwrap();
    let root = discovery.path();
    ruby_write(
        root,
        ".steps/hidden.rb",
        "Given('hidden step') { hidden() }\n",
    );
    ruby_write(
        root,
        "node_modules/workspace/nested.rb",
        "Given('nested step') { nested() }\n",
    );
    // A feature that uses neither definition makes each analyzed one an `unused-definition`
    // finding, which names the file the discovery flag admitted.
    ruby_write(
        root,
        "other.feature",
        "Feature: Other\n  Scenario: One\n    Given other step\n",
    );
    let hidden = ".steps/hidden.rb";
    let nested = "node_modules/workspace/nested.rb";
    for (flags, admitted) in [
        (&[][..], &[][..]),
        (&["--include-hidden"], &[hidden][..]),
        (&["--no-default-excludes"], &[nested]),
        (
            &["--include-hidden", "--no-default-excludes"],
            &[hidden, nested],
        ),
    ] {
        let (code, rows) = ruby_jsonl(root, ".steps/**/*.rb,node_modules/workspace/**/*.rb", flags);
        assert_eq!(code, 0, "{flags:?}");
        let mut paths: Vec<&str> = rows
            .iter()
            .filter(|row| row["rule"] == "unused-definition")
            .filter_map(|row| row["primary"]["path"].as_str())
            .collect();
        paths.sort_unstable();
        assert_eq!(paths, admitted, "{flags:?}");
        let summary = rows.last().unwrap();
        assert_eq!(
            summary["summary"]["definitionsAnalyzed"],
            admitted.len(),
            "{flags:?}"
        );
    }
}

/// A valid Ruby regexp whose compiled program exceeds the 1 MiB regex limit is named with the
/// actionable fix, keeps its definition in the reports, and fails under `--fail-on-incomplete`.
#[test]
fn ruby_regex_resource_limit_is_reported_and_keeps_the_definition() {
    // Ruby rejects the original `a{1000000}` (Onigmo caps a repeat at 100,000), so this uses the
    // largest repeat Ruby accepts, which still exceeds the compiled-program limit.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    // The regexp sits on line 3 so a `1:1` location fallback cannot satisfy `limit`.
    let oversized = "# Oversized repeat.\n\nGiven(/a{100000}/) { work() }\n";
    ruby_write(root, "steps.rb", oversized);
    let limit = "steps.rb:3:1 exceeds the 1048576-byte regex resource limit; simplify the matcher";
    for strict in [false, true] {
        let mut extra = vec!["--threshold", "100"];
        if strict {
            extra.push("--fail-on-incomplete");
        }
        let (code, summary, stderr) = ruby_summary_run(root, "*.rb", &extra);
        assert_eq!(code, if strict { 2 } else { 0 }, "{stderr}");
        assert_eq!(
            stderr.matches("regex resource limit").count(),
            1,
            "{stderr}"
        );
        assert!(stderr.contains(limit), "{stderr}");
        assert_eq!(summary["summary"]["definitionsAnalyzed"], 1);
    }
    // Control: a small repeat compiles, so the run carries no resource diagnostic.
    ruby_write(root, "steps.rb", "Given(/a{100}/) { work() }\n");
    let (code, _, stderr) = ruby_summary_run(root, "*.rb", &["--fail-on-incomplete"]);
    assert!(!stderr.contains("regex resource limit"), "{stderr}");
    assert_eq!(code, 0, "{stderr}");

    ruby_write(root, "steps.rb", oversized);
    ruby_write(
        root,
        "broken.feature",
        "Scenario: No feature\n  Given a step\n",
    );
    let output = tempfile::tempdir().unwrap();
    ruby_cli(root, "*.rb")
        .args(["--reporters", "json", "--output"])
        .arg(output.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains(limit))
        .stderr(predicate::str::contains("failed to parse Gherkin feature"));
    let report = ruby_report(&output.path().join("cuke-dedup.json"));
    assert_eq!(report["summary"]["definitionsAnalyzed"], 1);
}

/// Machine reports over Ruby findings: the implicit check writes every reporter with corpus and
/// metric fields, `--no-metrics` and `noMetrics` drop metrics, JSON uses the default directory,
/// JSONL streams only records, and `--reporters` before the target announces file output.
#[test]
fn ruby_machine_reports_follow_metrics_and_stream_contracts() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(
        root,
        "features/steps/checkout.rb",
        "Then('the receipt is visible') { receipt.verify_visible }\nThen('the receipt is visible') { other_receipt.verify_visible }\n",
    );
    ruby_write(
        root,
        "features/checkout.feature",
        "Feature: Checkout\n  Scenario: Receipt\n    Then the receipt is visible\n",
    );
    ruby_cli(root, "**/*.rb")
        .args(["--reporters", "terminal,json,html,sarif"])
        .args(["--output", "artifacts"])
        .assert()
        .code(1)
        .stdout(
            predicate::str::contains("duplicate-matcher").and(predicate::str::contains(
                "Analyzed 2 definitions and 1 feature step",
            )),
        );
    let report = ruby_report(&root.join("artifacts/cuke-dedup.json"));
    ruby_assert_pointers(
        &report,
        &[
            ("/schemaVersion", "3".into()),
            ("/summary/errors", 2.into()), // duplicate matcher + ambiguity
            ("/metrics/definitionFiles", 1.into()),
            ("/corpus/definitionFiles", 1.into()),
            ("/corpus/definitionFilesWithDefinitions", 1.into()),
            ("/corpus/definitionsExtracted", 2.into()),
            ("/metrics/featureFiles", 1.into()),
            ("/corpus/featureFiles", 1.into()),
            ("/corpus/featureFilesParsed", 1.into()),
            ("/metrics/filesDiscovered", 2.into()),
        ],
    );
    for phase in ["discoveryMs", "parsingMs", "analysisMs"] {
        let elapsed = report["metrics"].get(phase).and_then(Value::as_f64);
        assert!(
            elapsed.is_some_and(|value| value >= 0.0),
            "missing non-negative {phase}"
        );
    }
    let html = fs::read_to_string(root.join("artifacts/cuke-dedup.html")).unwrap();
    assert!(html.contains("\"definitionFilesWithDefinitions\":1"));
    let sarif = ruby_report(&root.join("artifacts/cuke-dedup.sarif"));
    ruby_assert_pointers(
        &sarif["runs"][0]["invocations"][0],
        &[
            (
                "/properties/corpus/definitionFilesWithDefinitions",
                1.into(),
            ),
            ("/executionSuccessful", true.into()),
        ],
    );

    // JSONL streams finding records then one summary, with nothing else on stdout.
    let streaming = ruby_cli(root, "**/*.rb")
        .args(["--reporters", "jsonl"])
        .assert()
        .code(1);
    let stdout = String::from_utf8(streaming.get_output().stdout.clone()).unwrap();
    let streamed = records(stdout.clone().into_bytes());
    let (summary, findings) = streamed.split_last().unwrap();
    assert!(!findings.is_empty());
    assert!(findings.iter().all(|record| record["type"] == "finding"));
    ruby_assert_pointers(
        summary,
        &[
            ("/type", "summary".into()),
            ("/metrics/filesDiscovered", 2.into()),
            ("/corpus/definitionFilesWithDefinitions", 1.into()),
            ("/corpus/definitionsExtracted", 2.into()),
            ("/corpus/featureFilesParsed", 1.into()),
        ],
    );
    assert!(!stdout.contains("Reports written to:"));

    let single = tempfile::tempdir().unwrap();
    let root = single.path();
    ruby_write(root, "steps.rb", "Given('one step') { work() }\n");
    ruby_cli(root, "*.rb")
        .args(["--reporters", "json", "--no-metrics"])
        .assert()
        .success();
    let report = ruby_report(&root.join("reports/cuke-dedup/cuke-dedup.json"));
    assert!(report.get("metrics").is_none());
    ruby_assert_pointers(
        &report["corpus"],
        &[
            ("/definitionFiles", 1.into()),
            ("/definitionFilesWithDefinitions", 1.into()),
            ("/definitionsExtracted", 1.into()),
        ],
    );
    let (code, rows) = ruby_jsonl(root, "*.rb", &[]);
    assert_eq!(code, 0);
    assert_eq!(rows[0]["type"], "summary");
    assert!(rows[0].get("metrics").is_none());
    assert_eq!(rows[0]["corpus"]["definitionsExtracted"], 1);

    fs::remove_dir_all(root.join("reports")).unwrap();
    let mut announced = Command::cargo_bin("cuke-dedup").unwrap();
    announced
        .current_dir(root)
        .args(["--reporters", "json", ".", "--definitions", "*.rb"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Reports written to:"))
        .stdout(predicate::str::contains("cuke-dedup.json"));
    // The documented default output directory, with metrics present unless disabled.
    let report = ruby_report(&root.join("reports/cuke-dedup/cuke-dedup.json"));
    assert!(report.get("metrics").is_some());
    assert_eq!(report["corpus"]["definitionsExtracted"], 1);

    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{"definitions":["*.rb"],"reporters":["json"],"noMetrics":true}"#,
    );
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .current_dir(root)
        .arg(".")
        .assert()
        .success();
    let report = ruby_report(&root.join("reports/cuke-dedup/cuke-dedup.json"));
    assert!(report.get("metrics").is_none());
    assert_eq!(report["corpus"]["definitionsExtracted"], 1);
}

/// A JSONL consumer that closes the pipe early ends a Ruby run cleanly.
#[cfg(unix)]
#[test]
fn ruby_closed_jsonl_consumer_is_a_clean_termination() {
    let directory = tempfile::tempdir().unwrap();
    // Distinct duplicate pairs, not one cluster: the stream must exceed the 64 KiB pipe buffer so
    // a write reaches the closed consumer.
    let definitions: String = (0..400)
        .map(|index| {
            format!("Given('step {index}') {{ alpha_{index}() }}\nGiven('step {index}') {{ beta_{index}() }}\n")
        })
        .collect();
    ruby_write(directory.path(), "steps.rb", &definitions);
    let output = ProcessCommand::new("bash")
        .args([
            "-o",
            "pipefail",
            "-c",
            "\"$CUKE_BINARY\" \"$CUKE_ROOT\" --definitions '*.rb' --reporters jsonl --threshold 100 | head -n 1 >/dev/null",
        ])
        .env("CUKE_BINARY", assert_cmd::cargo::cargo_bin!("cuke-dedup"))
        .env("CUKE_ROOT", directory.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Control: the uninterrupted stream is larger than a pipe buffer.
    let stream = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(directory.path())
        .args([
            "--definitions",
            "*.rb",
            "--reporters",
            "jsonl",
            "--threshold",
            "100",
        ])
        .output()
        .unwrap()
        .stdout;
    assert!(stream.len() > 4 * 65_536, "{} bytes", stream.len());
}

/// An unparseable Ruby file is tolerated and reported by default with its valid neighbour's
/// findings retained, fails under `--fail-on-unparseable` or `--fail-on-incomplete`, and a
/// corpus of parseable files reports no syntax incompleteness under either default or flag.
#[test]
fn ruby_unparseable_sources_are_tolerated_reported_and_gated_by_flags() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    ruby_write(
        root,
        ".cuke-dedup.json",
        r#"{"rules":{"duplicate-matcher":"warning"},"threshold":100}"#,
    );
    ruby_write(
        root,
        "valid.rb",
        "Given('an analyzed step') { first() }\nGiven('an analyzed step') { second() }\n",
    );
    // Control: only parseable files, so no syntax warning and the strict flag passes.
    ruby_cli(root, "*.rb")
        .assert()
        .success()
        .stderr(predicate::str::contains("Ruby source contains syntax errors").not());
    ruby_cli(root, "*.rb")
        .arg("--fail-on-unparseable")
        .assert()
        .success();

    ruby_write(root, "broken.rb", "Given('broken') do\n");
    ruby_cli(root, "*.rb")
        .args(["--reporters", "json"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "Ruby source contains syntax errors",
        ));
    let report = ruby_report(&root.join("reports/cuke-dedup/cuke-dedup.json"));
    assert_eq!(report["corpus"]["incomplete"], true);
    assert_eq!(report["summary"]["byRule"]["duplicate-matcher"], 1);
    ruby_cli(root, "*.rb")
        .arg("--fail-on-unparseable")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "Ruby source contains syntax errors",
        ));
    ruby_cli(root, "*.rb")
        .arg("--fail-on-incomplete")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("incomplete"));
}
