use assert_cmd::Command;
use serde_json::Value;
use std::fs;

fn run_project(root: &std::path::Path, patterns: &str, extra: &[&str]) -> std::process::Output {
    Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(root)
        .args([
            "--definitions",
            patterns,
            "--reporters",
            "jsonl",
            "--no-metrics",
        ])
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

fn assert_discovery(rows: &[Value], definitions: usize, incomplete: bool, source: &str) {
    let summary = rows.last().unwrap();
    assert_eq!(
        summary["summary"]["definitionsAnalyzed"], definitions,
        "{source}"
    );
    assert_eq!(summary["corpus"]["incomplete"], incomplete, "{source}");
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
