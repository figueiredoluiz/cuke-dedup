use assert_cmd::Command;
use cuke_dedup::config::{Config, ConfigOverrides};
use cuke_dedup::discovery::{SourceFile, SourceLanguage};
use cuke_dedup::ruby::RUBY;
use cuke_dedup::source_adapter::{AdapterRegistry, SourceExclusion};
use cuke_dedup::typescript::{JAVASCRIPT, TYPESCRIPT};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

/// Runs the analyzer with JSONL output and optional definition patterns, returning its captured process output.
fn run(root: &std::path::Path, definitions: Option<&str>, extra: &[&str]) -> std::process::Output {
    let mut command = Command::cargo_bin("cuke-dedup").unwrap();
    command
        .arg(root)
        .args(["--reporters", "jsonl", "--no-metrics"]);
    if let Some(pattern) = definitions {
        command.args(["--definitions", pattern]);
    }
    command.args(extra).output().unwrap()
}

/// Parses JSONL output and returns the summary record, panicking if records are invalid or the summary is absent.
fn summary(stdout: &[u8]) -> Value {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| row.get("summary").is_some())
        .unwrap()
}

/// Checks Ruby opt-in, dependency discovery, unresolved inputs, and mixed-language rejection retain expected CLI outcomes.
#[test]
fn ruby_selection_dependencies_and_mixed_domain_keep_cli_outcomes() {
    let project = tempfile::tempdir().unwrap();
    fs::write(
        project.path().join("steps.js"),
        "Given('js', () => work());",
    )
    .unwrap();
    fs::write(project.path().join("steps.rb"), "Given('ruby') { work() }").unwrap();
    let automatic = run(project.path(), None, &["--require-definitions"]);
    assert_eq!(automatic.status.code(), Some(0));
    assert_eq!(
        summary(&automatic.stdout)["summary"]["definitionsAnalyzed"],
        1
    );

    fs::create_dir(project.path().join("lib")).unwrap();
    fs::write(
        project.path().join("entry.rb"),
        "require_relative 'lib/support'",
    )
    .unwrap();
    fs::write(
        project.path().join("lib/support.rb"),
        "Given('from dependency') { work() }",
    )
    .unwrap();
    let explicit = run(project.path(), Some("entry.rb"), &["--require-definitions"]);
    assert_eq!(explicit.status.code(), Some(0));
    assert_eq!(
        summary(&explicit.stdout)["summary"]["definitionsAnalyzed"],
        1
    );

    for (source, excluded) in [
        ("require_relative 'missing'", false),
        ("require_relative 'lib/support'", true),
    ] {
        fs::write(project.path().join("entry.rb"), source).unwrap();
        let mut args = vec!["--fail-on-incomplete"];
        if excluded {
            args.extend(["--exclude", "lib/**"]);
        }
        let unresolved = run(project.path(), Some("entry.rb"), &args);
        assert_eq!(unresolved.status.code(), Some(2));
        assert_eq!(summary(&unresolved.stdout)["corpus"]["incomplete"], true);
    }

    let mixed = run(project.path(), Some("steps.rb,steps.js"), &[]);
    assert_eq!(mixed.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&mixed.stderr).contains("separate analysis runs"));
}

/// Checks the mixed-source refusal names its languages in registry order, whichever sorts first.
#[test]
fn ruby_mixed_sources_are_named_in_registry_order() {
    for (ruby, javascript) in [("a.rb", "b.js"), ("b.rb", "a.js")] {
        let project = tempfile::tempdir().unwrap();
        fs::write(project.path().join(ruby), "Given('a') { work }").unwrap();
        fs::write(project.path().join(javascript), "Given('a', () => work());").unwrap();
        let mixed = run(project.path(), Some("*.rb,*.js"), &[]);
        assert_eq!(mixed.status.code(), Some(2));
        let stderr = String::from_utf8_lossy(&mixed.stderr);
        assert!(
            stderr.contains(
                "Ruby and JS/TS sources require separate analysis runs with explicit definition \
                 and feature roots"
            ),
            "{stderr}"
        );
    }
}

/// Checks the help names each adapter's own exclusions, once, before the core's, at the root and on
/// the subcommand that inherits the option.
#[test]
fn help_names_exclusions_from_the_adapters() {
    for args in [
        &["--help"][..],
        &["-h"][..],
        &["check", "--help"][..],
        &["check", "-h"][..],
    ] {
        let output = Command::cargo_bin("cuke-dedup")
            .unwrap()
            .args(args)
            .output()
            .unwrap();
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(
            help.contains("Disable built-in exclusions such as node_modules and target\n"),
            "{help}"
        );
    }
}

/// Checks a file holding only parameter declarations words its diagnostic, with no definition to
/// take a language from.
#[test]
fn ruby_declaration_only_file_words_its_parameter_diagnostic() {
    let project = tempfile::tempdir().unwrap();
    let declaration = "ParameterType(name: 'size', regexp: /small/, transformer: ->(s) { s })\n";
    fs::write(
        project.path().join("types.rb"),
        format!("{declaration}{declaration}"),
    )
    .unwrap();
    let output = run(project.path(), Some("types.rb"), &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr
            .contains("Ruby parameter type \"size\" has an unresolved or duplicate declaration at"),
        "{stderr}"
    );
}

/// Checks NUL-byte binary exclusion belongs to the JavaScript adapter and does not reject Ruby source.
#[test]
fn source_exclusion_stays_with_javascript_adapter() {
    let project = tempfile::tempdir().unwrap();
    let js = project.path().join("steps.js");
    let ruby = project.path().join("steps.rb");
    fs::write(&js, b"const data = '\0';").unwrap();
    fs::write(&ruby, b"Given('step') { work() }\0").unwrap();
    let adapter = |language| {
        AdapterRegistry::builtin()
            .registrations()
            .iter()
            .map(|entry| entry.adapter)
            .find(|adapter| adapter.language() == language)
            .unwrap()
    };
    let js_file = SourceFile {
        path: js,
        language: JAVASCRIPT,
    };
    let ruby_file = SourceFile {
        path: ruby,
        language: RUBY,
    };
    assert_eq!(
        adapter(JAVASCRIPT).inspect_source(&js_file, &[]),
        Some(SourceExclusion::Binary)
    );
    assert_eq!(adapter(RUBY).inspect_source(&ruby_file, &[]), None);
}

/// Checks both Ruby load-path forms (`ruby.loadPaths` and the flat `rubyLoadPaths` alias) in a
/// standalone file and in `package.json`, the one-form-per-source rule, section validation,
/// top-level unknown keys, and that the effective config still serializes the flat key only when
/// set.
#[test]
fn ruby_configuration_keeps_flat_compatibility_key() {
    let root = tempfile::tempdir().unwrap();
    let config = Config::load(root.path(), ConfigOverrides::default()).unwrap();
    assert!(config.ruby_load_paths().is_empty());
    assert!(serde_json::to_value(&config)
        .unwrap()
        .get("rubyLoadPaths")
        .is_none());

    // (package.json `cukeDedup`, standalone `.cuke-dedup.json`) -> load paths. The standalone file
    // is read by `from_str` and the package by `from_value`; both go through the flattened keys.
    let lib = || vec![PathBuf::from("lib")];
    for (package, standalone, expected) in [
        (None, Some(r#"{"rubyLoadPaths":["lib"]}"#), lib()),
        (None, Some(r#"{"ruby":{"loadPaths":["lib"]}}"#), lib()),
        (Some(r#"{"ruby":{"loadPaths":["lib"]}}"#), None, lib()),
        (Some(r#"{"rubyLoadPaths":["lib"]}"#), None, lib()),
        // A section without `loadPaths`, or a null one, sets nothing.
        (None, Some(r#"{"ruby":{}}"#), Vec::new()),
        (None, Some(r#"{"ruby":null}"#), Vec::new()),
    ] {
        let root = tempfile::tempdir().unwrap();
        if let Some(package) = package {
            fs::write(
                root.path().join("package.json"),
                format!(r#"{{"cukeDedup":{package}}}"#),
            )
            .unwrap();
        }
        if let Some(standalone) = standalone {
            fs::write(root.path().join(".cuke-dedup.json"), standalone).unwrap();
        }
        let config = Config::load(root.path(), ConfigOverrides::default())
            .unwrap_or_else(|error| panic!("{package:?} {standalone:?}: {error:#}"));
        // An auto-discovered file that fails to parse only warns and falls back to defaults, which
        // would pass the empty rows; a parsed source leaves no warning.
        assert!(
            config.config_warnings.is_empty(),
            "{standalone:?}: {:?}",
            config.config_warnings
        );
        assert_eq!(
            config.ruby_load_paths(),
            expected.as_slice(),
            "{package:?} {standalone:?}"
        );
        let printed = serde_json::to_value(&config).unwrap();
        assert!(
            printed.get("ruby").is_none(),
            "only the flat key is printed"
        );
        assert_eq!(
            printed.get("rubyLoadPaths").is_some(),
            !expected.is_empty(),
            "{package:?} {standalone:?}"
        );
    }

    // Rejected sources, each with the error text that names the cause. Errors go through
    // `--config` so an invalid file fails the run instead of falling back with a warning.
    for (source, error) in [
        (
            r#"{"rubyLoadPaths":["lib"],"ruby":{"loadPaths":["lib"]}}"#,
            // The file is named as written or relative to the root, depending on how the root resolves.
            "config.json: set Ruby load paths with `ruby.loadPaths` or `rubyLoadPaths`, not both",
        ),
        (
            r#"{"ruby":{"loadpaths":["lib"]}}"#,
            "unknown field `loadpaths`",
        ),
        (r#"{"ruby":["lib"]}"#, "invalid type"),
        (
            r#"{"rubyLoadPath":["lib"]}"#,
            "unknown field `rubyLoadPath`",
        ),
        // A core key error keeps its position although the frontend keys are flattened in.
        ("{\n\"threshold\": \"x\"}", "line 2 column"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config.json");
        fs::write(&path, source).unwrap();
        let overrides = ConfigOverrides {
            config_file: Some(path),
            ..ConfigOverrides::default()
        };
        let message = format!(
            "{:#}",
            Config::load(root.path(), overrides).expect_err(source)
        );
        assert!(message.contains(error), "{source}: {message}");
    }
    // The conflict names `package.json` when that is the source.
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("package.json"),
        r#"{"cukeDedup":{"rubyLoadPaths":["lib"],"ruby":{"loadPaths":["lib"]}}}"#,
    )
    .unwrap();
    let message = format!(
        "{:#}",
        Config::load(root.path(), ConfigOverrides::default()).unwrap_err()
    );
    assert!(
        message.starts_with("invalid configuration in package.json#cukeDedup: "),
        "{message}"
    );
}

/// How one configuration text reads: the load paths it yields, or the parse error after
/// `failed to parse <path>: `.
type ConfigResult = Result<&'static [&'static str], &'static str>;

const NO_PATHS: ConfigResult = Ok(&[]);
const LIB: ConfigResult = Ok(&["lib"]);
const NOT_SEQUENCE: &str = "invalid type: integer `1`, expected a sequence";
const NOT_SECTION: &str = "invalid type: integer `42`, expected struct RawRubySection";

/// One config text as a file and as `package.json#cukeDedup`, in that order. Errors differ: a file
/// carries a position, and `package.json` is read through `Value`, so its last duplicate wins. The
/// rows record current text, including the internal type names serde prints; a change to one
/// should be a decision, not an accident.
const CONFIG_BEHAVIOUR: &[(&str, ConfigResult, ConfigResult)] = &[
    // An unknown key is reported at the end of the root object, first in document order.
    (
        r#"{"foo":1}"#,
        Err("unknown field `foo` at line 1 column 9"),
        Err("unknown field `foo`"),
    ),
    (
        r#"{"foo":1,"bar":2}"#,
        Err("unknown field `foo` at line 1 column 17"),
        Err("unknown field `foo`"),
    ),
    (
        "{\n  \"definitions\": [\"a\"],\n  \"foo\": 1\n}\n",
        Err("unknown field `foo` at line 4 column 1"),
        Err("unknown field `foo`"),
    ),
    (
        r#"{"definitions":["a"],"foo":1}"#,
        Err("unknown field `foo` at line 1 column 29"),
        Err("unknown field `foo`"),
    ),
    // Language keys are also checked at the end of the object.
    (
        r#"{"ruby":{"x":1}}"#,
        Err("unknown field `x`, expected `loadPaths` at line 1 column 16"),
        Err("unknown field `x`, expected `loadPaths`"),
    ),
    (
        r#"{"ruby":{"loadPaths":"lib"}}"#,
        Err(r#"invalid type: string "lib", expected a sequence at line 1 column 28"#),
        Err(r#"invalid type: string "lib", expected a sequence"#),
    ),
    (
        r#"{"ruby":[1]}"#,
        Err("invalid type: integer `1`, expected a sequence at line 1 column 12"),
        Err(NOT_SEQUENCE),
    ),
    (
        r#"{"ruby":42}"#,
        Err("invalid type: integer `42`, expected struct RawRubySection at line 1 column 11"),
        Err(NOT_SECTION),
    ),
    (
        r#"{"rubyLoadPaths":"lib"}"#,
        Err(r#"invalid type: string "lib", expected a sequence at line 1 column 23"#),
        Err(r#"invalid type: string "lib", expected a sequence"#),
    ),
    (
        r#"{"rubyLoadPaths":[1]}"#,
        Err("invalid type: integer `1`, expected path string at line 1 column 21"),
        Err("invalid type: integer `1`, expected path string"),
    ),
    // A core key is checked where it stands, so it wins over everything checked at the end.
    (
        r#"{"definitions":1}"#,
        Err("invalid type: integer `1`, expected a sequence at line 1 column 16"),
        Err(NOT_SEQUENCE),
    ),
    (
        r#"{"foo":1,"definitions":2}"#,
        Err("invalid type: integer `2`, expected a sequence at line 1 column 24"),
        Err("invalid type: integer `2`, expected a sequence"),
    ),
    (
        r#"{"rubyLoadPaths":1,"definitions":2}"#,
        Err("invalid type: integer `2`, expected a sequence at line 1 column 34"),
        Err("invalid type: integer `2`, expected a sequence"),
    ),
    // A repeated key fails in a file, and the last one wins in `package.json`.
    (
        r#"{"rubyLoadPaths":["a"],"rubyLoadPaths":["b"]}"#,
        Err("duplicate field `rubyLoadPaths` at line 1 column 45"),
        Ok(&["b"]),
    ),
    (
        r#"{"ruby":{},"ruby":{}}"#,
        Err("duplicate field `ruby` at line 1 column 21"),
        NO_PATHS,
    ),
    (
        r#"{"ruby":{"loadPaths":[],"loadPaths":["lib"]}}"#,
        Err("duplicate field `loadPaths` at line 1 column 45"),
        LIB,
    ),
    (
        r#"{"ruby":{"loadPaths":null,"loadPaths":[]}}"#,
        Err("duplicate field `loadPaths` at line 1 column 42"),
        NO_PATHS,
    ),
    (
        r#"{"definitions":[],"definitions":[]}"#,
        Err("duplicate field `definitions` at line 1 column 31"),
        NO_PATHS,
    ),
    // A language key's error wins over an unknown key, wherever each stands.
    (
        r#"{"foo":1,"ruby":42}"#,
        Err("invalid type: integer `42`, expected struct RawRubySection at line 1 column 19"),
        Err(NOT_SECTION),
    ),
    (
        r#"{"ruby":42,"foo":1}"#,
        Err("invalid type: integer `42`, expected struct RawRubySection at line 1 column 19"),
        Err(NOT_SECTION),
    ),
    // Between two language keys, document order decides.
    (
        r#"{"ruby":{"x":1},"rubyLoadPaths":1}"#,
        Err("unknown field `x`, expected `loadPaths` at line 1 column 34"),
        Err("unknown field `x`, expected `loadPaths`"),
    ),
    (
        r#"{"rubyLoadPaths":1,"ruby":{"x":1}}"#,
        Err("invalid type: integer `1`, expected a sequence at line 1 column 34"),
        Err(NOT_SEQUENCE),
    ),
    // The first value is read before the repeat is noticed, so its error wins over the duplicate.
    (
        r#"{"ruby":42,"ruby":[]}"#,
        Err("invalid type: integer `42`, expected struct RawRubySection at line 1 column 21"),
        NO_PATHS,
    ),
    (
        r#"{"definitions":1,"definitions":[]}"#,
        Err("invalid type: integer `1`, expected a sequence at line 1 column 16"),
        NO_PATHS,
    ),
    (
        r#"{"ruby":null,"ruby":[]}"#,
        Err("duplicate field `ruby` at line 1 column 23"),
        NO_PATHS,
    ),
    (
        r#"{"ruby":[],"ruby":42}"#,
        Err("duplicate field `ruby` at line 1 column 21"),
        Err(NOT_SECTION),
    ),
    // A `null` sets nothing, so it does not conflict with the other form.
    (
        r#"{"ruby":{"loadPaths":null},"rubyLoadPaths":["a"]}"#,
        Ok(&["a"]),
        Ok(&["a"]),
    ),
    (
        r#"{"ruby":null,"rubyLoadPaths":["a"]}"#,
        Ok(&["a"]),
        Ok(&["a"]),
    ),
    (r#"{"ruby":{"loadPaths":["lib"]}}"#, LIB, LIB),
    (r#"{"rubyLoadPaths":["lib"]}"#, LIB, LIB),
    (r#"{"ruby":{}}"#, NO_PATHS, NO_PATHS),
    (r#"{"ruby":null}"#, NO_PATHS, NO_PATHS),
    (r#"{"rubyLoadPaths":null}"#, NO_PATHS, NO_PATHS),
    (r#"{"rubyLoadPaths":[]}"#, NO_PATHS, NO_PATHS),
    (r#"{"ruby":{"loadPaths":[]}}"#, NO_PATHS, NO_PATHS),
    // Not an object at all.
    (
        "[]",
        Err("invalid type: sequence, expected struct RawConfig at line 1 column 0"),
        Err("invalid type: sequence, expected struct RawConfig"),
    ),
    (
        "1",
        Err("invalid type: integer `1`, expected struct RawConfig at line 1 column 1"),
        Err("invalid type: integer `1`, expected struct RawConfig"),
    ),
    (
        "null",
        Err("invalid type: null, expected struct RawConfig at line 1 column 4"),
        NO_PATHS,
    ),
    // Syntax errors come from reading the text, so `package.json`'s wrapper shifts the column. An
    // unknown key is reported before text after the object is.
    (
        "{",
        Err("EOF while parsing an object at line 1 column 1"),
        Err("EOF while parsing an object at line 1 column 15"),
    ),
    (
        r#"{"ruby":{"loadPaths":["a"],}}"#,
        Err("trailing comma at line 1 column 28"),
        Err("trailing comma at line 1 column 41"),
    ),
    (
        "{} x",
        Err("trailing characters at line 1 column 4"),
        Err("expected `,` or `}` at line 1 column 17"),
    ),
    (
        r#"{"foo":1} x"#,
        Err("unknown field `foo` at line 1 column 9"),
        Err("expected `,` or `}` at line 1 column 24"),
    ),
];

/// A temporary project root as the tool prints it: canonical, without the `\\?\` lead Windows adds.
fn printed_root(directory: &tempfile::TempDir) -> PathBuf {
    let root = directory.path().canonicalize().unwrap();
    let root = root.to_string_lossy();
    PathBuf::from(root.strip_prefix(r"\\?\").unwrap_or(&root))
}

/// Runs `--print-config` on `root`, returning the exit code, stderr and printed configuration.
fn print_config(root: &std::path::Path, extra: &[&str]) -> (i32, String, Option<Value>) {
    let output = Command::cargo_bin("cuke-dedup")
        .unwrap()
        .arg(root)
        .arg("--print-config")
        .args(extra)
        .output()
        .unwrap();
    let printed = serde_json::from_slice(&output.stdout).ok();
    (
        output.status.code().unwrap(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        printed,
    )
}

/// The load paths a printed configuration holds: none when `rubyLoadPaths` is absent.
fn printed_paths(printed: &Option<Value>) -> Vec<String> {
    printed
        .as_ref()
        .unwrap()
        .get("rubyLoadPaths")
        .map_or_else(Vec::new, |paths| {
            paths
                .as_array()
                .unwrap()
                .iter()
                .map(|path| path.as_str().unwrap().to_owned())
                .collect()
        })
}

/// Checks one `--print-config` run: the paths it yields, or the exact parse error naming `file`.
fn assert_config_result(
    root: &std::path::Path,
    file: &std::path::Path,
    extra: &[&str],
    expected: ConfigResult,
    source: &str,
) {
    let (code, stderr, printed) = print_config(root, extra);
    match expected {
        Ok(paths) => {
            assert_eq!(code, 0, "{source}: {stderr}");
            assert_eq!(printed_paths(&printed), paths, "{source}");
        }
        Err(message) => {
            assert_eq!(code, 2, "{source}");
            assert_eq!(
                stderr.trim_end(),
                format!("error: failed to parse {}: {message}", file.display()),
                "{source}"
            );
        }
    }
}

/// Checks each configuration text through `--config` and `package.json#cukeDedup` with its exact
/// text, position and precedence.
#[test]
fn configuration_errors_keep_their_text_position_and_precedence() {
    for (source, file, package) in CONFIG_BEHAVIOUR.iter().copied() {
        let directory = tempfile::tempdir().unwrap();
        let root = printed_root(&directory);
        let path = root.join("config.json");
        fs::write(&path, source).unwrap();
        assert_config_result(
            &root,
            &path,
            &["--config", path.to_str().unwrap()],
            file,
            source,
        );

        let directory = tempfile::tempdir().unwrap();
        let root = printed_root(&directory);
        let path = root.join("package.json");
        fs::write(&path, format!(r#"{{"cukeDedup":{source}}}"#)).unwrap();
        assert_config_result(&root, &path, &[], package, source);
    }
}

/// Checks an auto-discovered file that fails to parse warns with its whole text and analysis
/// continues, while one that parses leaves no warning.
#[test]
fn an_unreadable_auto_discovered_config_warns_and_analysis_continues() {
    for (source, file, _) in CONFIG_BEHAVIOUR.iter().copied() {
        let directory = tempfile::tempdir().unwrap();
        let root = printed_root(&directory);
        let path = root.join(".cuke-dedup.json");
        fs::write(&path, source).unwrap();
        let output = run(&root, None, &[]);
        assert_eq!(output.status.code(), Some(0), "{source}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        match file {
            Ok(_) => assert!(!stderr.contains("could not load"), "{source}: {stderr}"),
            Err(message) => {
                let path = path.display();
                let warning = format!("could not load auto-discovered config {path} (failed to parse {path}: {message}); trying the next configuration source");
                assert!(stderr.contains(&warning), "{source}: {stderr}");
            }
        }
    }
}

/// Checks the one-form rule comes from applying a source, so it names that source, and that it
/// holds for empty lists too, in all three sources.
#[test]
fn ruby_load_path_conflicts_name_their_source() {
    for ruby_keys in [
        r#"{"ruby":{"loadPaths":["a"]},"rubyLoadPaths":["b"]}"#,
        r#"{"ruby":{"loadPaths":[]},"rubyLoadPaths":[]}"#,
    ] {
        for (name, contents, label, extra) in [
            (
                "config.json",
                ruby_keys.to_owned(),
                "config.json",
                &["--config", "config.json"][..],
            ),
            (
                ".cuke-dedup.json",
                ruby_keys.to_owned(),
                ".cuke-dedup.json",
                &[][..],
            ),
            (
                "package.json",
                format!(r#"{{"cukeDedup":{ruby_keys}}}"#),
                "package.json#cukeDedup",
                &[][..],
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let root = printed_root(&directory);
            fs::write(root.join(name), contents).unwrap();
            let (code, stderr, _) = print_config(&root, extra);
            assert_eq!(code, 2, "{name}");
            assert_eq!(
                stderr.trim_end(),
                format!("error: invalid configuration in {label}: set Ruby load paths with `ruby.loadPaths` or `rubyLoadPaths`, not both"),
                "{name}"
            );
        }
    }
}

/// Checks the first readable source is the only one applied: a standalone file replaces
/// `package.json` whole, an unreadable one falls through to the next, and `--config` replaces the
/// rest. A `package.json` that does not parse is fatal in every case, because it is read first.
#[test]
fn the_first_readable_configuration_source_replaces_the_others() {
    let a = r#"{"rubyLoadPaths":["a"]}"#;
    let b = r#"{"rubyLoadPaths":["b"]}"#;
    let conflict = r#"{"ruby":{"loadPaths":["a"]},"rubyLoadPaths":["b"]}"#;
    let unreadable = r#"{"foo":1}"#;
    for (package, files, extra, expected) in [
        (
            a,
            &[(".cuke-dedup.json", r#"{"definitions":["x"]}"#)][..],
            &[][..],
            Vec::new(),
        ),
        (
            a,
            &[(".cuke-dedup.json", r#"{"rubyLoadPaths":null}"#)][..],
            &[][..],
            Vec::new(),
        ),
        (
            a,
            &[(".cuke-dedup.json", r#"{"ruby":{}}"#)][..],
            &[][..],
            Vec::new(),
        ),
        (a, &[(".cuke-dedup.json", b)][..], &[][..], vec!["b"]),
        (
            r#"{"ruby":{"loadPaths":["a"]}}"#,
            &[(".cuke-dedup.json", b)][..],
            &[][..],
            vec!["b"],
        ),
        // Replaced before it is applied, so its conflict never surfaces.
        (conflict, &[(".cuke-dedup.json", b)][..], &[][..], vec!["b"]),
        (
            conflict,
            &[("config.json", b)][..],
            &["--config", "config.json"][..],
            vec!["b"],
        ),
        // An unreadable file falls through to the next candidate, then to `package.json`.
        (
            a,
            &[(".cuke-dedup.json", unreadable)][..],
            &[][..],
            vec!["a"],
        ),
        (
            a,
            &[
                (".cuke-dedup.json", unreadable),
                (".config/cuke-dedup.json", b),
            ][..],
            &[][..],
            vec!["b"],
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = printed_root(&directory);
        fs::create_dir(root.join(".config")).unwrap();
        fs::write(
            root.join("package.json"),
            format!(r#"{{"cukeDedup":{package}}}"#),
        )
        .unwrap();
        for (name, contents) in files {
            fs::write(root.join(name), contents).unwrap();
        }
        let (code, stderr, printed) = print_config(&root, extra);
        assert_eq!(code, 0, "{package} {files:?}: {stderr}");
        assert_eq!(printed_paths(&printed), expected, "{package} {files:?}");
    }

    for extra in [&[][..], &["--config", ".cuke-dedup.json"][..]] {
        let directory = tempfile::tempdir().unwrap();
        let root = printed_root(&directory);
        let package = root.join("package.json");
        fs::write(&package, r#"{"cukeDedup":{"foo":1}}"#).unwrap();
        fs::write(root.join(".cuke-dedup.json"), b).unwrap();
        let (code, stderr, _) = print_config(&root, extra);
        assert_eq!(code, 2, "{extra:?}");
        assert_eq!(
            stderr.trim_end(),
            format!(
                "error: failed to parse {}: unknown field `foo`",
                package.display()
            ),
            "{extra:?}"
        );
    }
}

/// Checks `--print-config` prints the flat key first, only when it holds paths, and never `ruby`.
#[test]
fn ruby_load_path_key_prints_first_and_only_when_set() {
    for (ruby_keys, first) in [
        (r#"{"rubyLoadPaths":["lib"]}"#, "rubyLoadPaths"),
        (r#"{"ruby":{"loadPaths":["lib"]}}"#, "rubyLoadPaths"),
        (r#"{"rubyLoadPaths":[]}"#, "root"),
        (r#"{"ruby":{}}"#, "root"),
        (r#"{"definitions":["a"]}"#, "root"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join(".cuke-dedup.json"), ruby_keys).unwrap();
        let (_, _, printed) = print_config(directory.path(), &[]);
        let printed = printed.unwrap();
        let printed = printed.as_object().unwrap();
        assert_eq!(
            printed.keys().next().map(String::as_str),
            Some(first),
            "{ruby_keys}"
        );
        assert!(!printed.contains_key("ruby"), "{ruby_keys}");
    }
}

/// Whether the handler of the only definition `source` registers counts as a stub.
fn handler_is_stub(language: SourceLanguage, path: &str, source: &str) -> bool {
    let adapter = AdapterRegistry::builtin()
        .registrations()
        .iter()
        .map(|entry| entry.adapter)
        .find(|adapter| adapter.language() == language)
        .unwrap();
    let file = SourceFile {
        path: path.into(),
        language,
    };
    let mut extraction = adapter.extract(source, &file).unwrap();
    assert_eq!(extraction.definitions.len(), 1, "{source}");
    extraction.definitions.remove(0).handler.trivial
}

/// Both adapters apply one stub vocabulary: a spelling that marks a stub in one listed syntax marks
/// it in every listed syntax of both languages, and a near miss marks none. Each row is (Ruby identifier,
/// TypeScript identifier, string spelling, is a stub); the controls are the rows that are not.
#[test]
fn adapters_share_one_stub_vocabulary() {
    let rows = [
        ("pending", "pending", "PENDING", true),
        ("not_implemented", "notImplemented", "Not implemented", true),
        ("todo", "TODO", "to-do", true),
        ("pending_review", "pendingReview", "pending review", false),
        ("done", "done", "done", false),
    ];
    for (ruby, typescript, label, stub) in rows {
        for source in [
            format!("Given('a') {{ {ruby}() }}"),
            format!("Given('a') {{ raise \"{label}\" }}"),
        ] {
            assert_eq!(handler_is_stub(RUBY, "steps.rb", &source), stub, "{source}");
        }
        for source in [
            format!("Given('a', () => {typescript}());"),
            format!("Given('a', () => {{ throw new Error('{label}'); }});"),
            format!("Given('a', () => '{label}');"),
        ] {
            assert_eq!(
                handler_is_stub(TYPESCRIPT, "steps.ts", &source),
                stub,
                "{source}"
            );
        }
    }
}
