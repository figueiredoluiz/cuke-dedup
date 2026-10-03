use assert_cmd::Command;
use serde_json::Value;
use std::fs;

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
        assert!(String::from_utf8_lossy(&output.stderr).contains("source graph exceeds"));
        assert_eq!(output.status.code(), Some(2));
    }
}

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
            .timeout(std::time::Duration::from_secs(10))
            .output()
            .unwrap();
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
            assert_parameterization_outcome(
                &rows,
                parameterized,
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
            assert_discovery(&rows, 2, incomplete, &source);
            assert_eq!(
                rows.iter().any(|row| row["rule"] == "duplicate-handler"),
                !incomplete,
                "trusted={trusted}: {source}"
            );
        }
    }
}

#[test]
fn ruby_assertion_provider_authority_matrix() {
    for (provider, before, after, trusted) in [
        (ASSERTION_PROVIDER, "", "", true),
        (ASSERTION_PROVIDER, "Alias = Assertions", "", false),
        (ASSERTION_PROVIDER, "", "Alias = Assertions", false),
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
        assert_discovery(&rows, 2, trusted || provider.contains("alias_method"), &format!("{provider}: {source}"));
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
        assert_discovery(&rows, 2, true, body);
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
            assert_discovery(&rows, 2, incomplete, &body);
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

#[test]
fn ruby_assertion_proof_relevance_matches_the_load_contract() {
    for (load, configured, cycle, incomplete) in [
        ("require_relative 'assertions'", "./assertions", false, true),
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
        let trusted =
            load.starts_with("require_relative") && configured == "./assertions" && !cycle;
        assert_eq!(
            rows.iter().any(|r| r["rule"] == "duplicate-handler"),
            !trusted,
            "{source}: cycle={cycle}"
        );
        assert_eq!(output.status.code(), Some(if incomplete { 2 } else { 1 }));
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

#[test]
fn ruby_partial_assertion_events_do_not_invent_complete_handler_similarity() {
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
            assert_eq!(
                finding["evidence"]["handlerSimilarity"], score,
                "trusted={trusted}: {source}"
            );
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
            assert_discovery(&rows, 2, incomplete, &source);
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
        assert_discovery(rows.as_array().unwrap(), definitions, true, &source);
        assert_handler_finding(&rows, false, &source);
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
        assert_discovery(rows.as_array().unwrap(), expected, true, &source);
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
                assert_discovery(&rows, 2, uncertain, &format!("{trusted}/{form}/{expected}"));
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

#[test]
fn ruby_named_method_assertion_bindings_follow_the_body_source() {
    for expected in ["'ready'", "UNKNOWN"] {
        let root = assertion_project(ASSERTION_PROVIDER, "require_relative 'body'; Given('first', &method(:handler)); Then('second', &method(:handler))", true);
        fs::write(root.path().join("body.rb"), format!("require_relative 'assertions'; def handler; Assertions.expect(page).to_be({expected}); end")).unwrap();
        let rows = records(run_project(root.path(), "steps.rb", &[]).stdout);
        assert_discovery(&rows, 2, expected == "UNKNOWN", expected);
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
            assert_handler_finding(&Value::Array(rows), true, &source);
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
