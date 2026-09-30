use super::regex_expression;

#[test]
fn local_registration_aliases_preserve_final_findings_without_granting_unknown_trust() {
    use crate::model::Rule;
    use crate::source_adapter::{SourceAdapter, SourceFile, SourceLanguage};
    let dir = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: dir.path().join("steps.rb"),
        language: SourceLanguage::Ruby,
    };
    let config = crate::config::Config::load(dir.path(), Default::default()).unwrap();
    let direct = "Given('same') { first() }; Given('same') { second() }";
    for keyword in ["Given", "When", "Then", "And", "But"] {
        for (prefix, calls, definitions, complete, duplicate) in [
            ("", direct, 2, true, true),
            (
                "given = method(:Given);",
                "given.call('same') { first() }; given.call('same') { second() }",
                2,
                true,
                true,
            ),
            (
                "given = method('Given');",
                "given.call('same') { first() }; Given('same') { second() }",
                2,
                true,
                true,
            ),
            (
                "given = method(:Given); other = method(:Then);",
                "given.call('same') { first() }; other.call('same') { second() }",
                2,
                true,
                true,
            ),
            ("given = method(:Given);", direct, 2, true, true),
            (
                "given = method(:Given);",
                "given.call('alpha') { first() }; given.call('omega') { second() }",
                2,
                true,
                false,
            ),
            (
                "given = method(:Given); given = foreign;",
                direct,
                0,
                false,
                false,
            ),
            (
                "given = method(:Given); consume(given);",
                direct,
                0,
                false,
                false,
            ),
            (
                "given = method(:Given); copy = given;",
                direct,
                0,
                false,
                false,
            ),
            (
                "given.call('early') { first() }; given = method(:Given);",
                direct,
                0,
                false,
                false,
            ),
            (
                "given = method(:Given); def nested(given); given.call('same') { first() }; end;",
                direct,
                0,
                false,
                false,
            ),
            (
                "given = method(:Given); if enabled; given.call('same') { first() }; end;",
                direct,
                0,
                false,
                false,
            ),
            (
                "given = method(:Given);",
                "Given('same') { given.call('same') { first() } }",
                0,
                false,
                false,
            ),
            (
                "given = method(:Given); given.call = replacement;",
                direct,
                0,
                false,
                false,
            ),
            (
                "given = method(:Given); given ||= replacement;",
                direct,
                0,
                false,
                false,
            ),
            (
                "given = method(:Given);",
                "given.call('same') { eval(dynamic) }",
                1,
                false,
                false,
            ),
            (
                "given = ->(text, &handler) { :ignored };",
                "given.call('same') { first() }; given.call('same') { second() }",
                0,
                true,
                false,
            ),
            ("given = foreign.method(:Given);", direct, 0, false, false),
            ("given = method(:untrusted);", direct, 0, false, false),
            (
                "def method(name); foreign; end; given = method(:Given);",
                direct,
                0,
                false,
                false,
            ),
            (
                "class Method; def call(*args); :ignored; end; end; given = method(:Given);",
                direct,
                0,
                false,
                false,
            ),
            (
                "given = method(:Given);",
                "given.call(dynamic) { first() }",
                0,
                false,
                false,
            ),
            (
                "given = method(:Given);",
                "given.call('same', &handler)",
                0,
                false,
                false,
            ),
        ] {
            let source = format!("{prefix}\n{calls}").replace("Given", keyword);
            let extraction = super::RUBY_ADAPTER.extract(&source, &file).unwrap();
            assert_eq!(extraction.definitions.len(), definitions, "{source}");
            assert_eq!(
                extraction.diagnostics.is_empty(),
                complete,
                "{source}: {:?}",
                extraction.diagnostics
            );
            let result = crate::analysis::analyze_with_step_usage(
                extraction.definitions,
                vec![],
                &config,
                &extraction.indirect_usage.unwrap(),
            )
            .unwrap()
            .result;
            assert_eq!(
                result
                    .findings
                    .iter()
                    .any(|f| f.rule == Rule::DuplicateMatcher),
                duplicate,
                "{source}"
            );
            assert!(
                !result.findings.iter().any(|f| matches!(
                    f.rule,
                    Rule::DuplicateHandler | Rule::ParameterizationCandidate
                )),
                "{source}"
            );
        }
        let source = format!("given = method(:{keyword})\ngiven.call('alpha') {{ work() }}\ngiven.call('omega') {{ work() }}");
        let extraction = super::RUBY_ADAPTER.extract(&source, &file).unwrap();
        assert!(extraction.diagnostics.is_empty());
        assert_eq!(
            extraction
                .definitions
                .iter()
                .map(|d| (d.registration.as_str(), d.location.line))
                .collect::<Vec<_>>(),
            [(keyword, 2), (keyword, 3)]
        );
        let outcome = crate::analysis::analyze_with_step_usage(
            extraction.definitions,
            vec![],
            &config,
            &extraction.indirect_usage.unwrap(),
        )
        .unwrap();
        assert!(outcome.incomplete.is_empty());
        assert!(outcome
            .result
            .findings
            .iter()
            .any(|f| f.rule == Rule::DuplicateHandler));
    }
}

#[test]
fn unresolved_dependency_usage_is_exposed_by_the_direct_adapter_api() {
    use crate::source_adapter::{SourceAdapter, SourceFile, SourceLanguage};
    let extraction = super::RUBY_ADAPTER
        .extract(
            "require 'external/provider'; Given('local') { work() }",
            &SourceFile {
                path: "steps.rb".into(),
                language: SourceLanguage::Ruby,
            },
        )
        .unwrap();
    assert_eq!(extraction.definitions.len(), 1);
    assert!(extraction.indirect_usage.unwrap().unknown);
}

#[test]
fn ruby_regex_subset_preserves_line_anchors_and_ascii_digits() {
    for (pattern, input, matches) in [
        ("^value$", "other\nvalue\nlast", true),
        (r"\Avalue\z", "other\nvalue\nlast", false),
        (r"\d+", "12", true),
        (r"\d+", "１２", false),
        (r"value\/path", "value/path", true),
        (r"\A\w+\z", "alpha_19", true),
        (r"\A\w+\z", "日本", false),
        (r"\A\s\z", "\u{b}", true),
        (r"\A\s\z", "\u{c}", true),
        (r"\A\s\z", "\u{a0}", false),
    ] {
        let regex = regex::Regex::new(&regex_expression(pattern, "").unwrap()).unwrap();
        assert_eq!(regex.is_match(input), matches, "{pattern} {input}");
    }
    for (pattern, flags) in [
        ("a++a", ""),
        ("a*+a", ""),
        ("a?+a", ""),
        ("a{1,2}+a", ""),
        ("value.*", "i"),
        ("value", "x"),
        ("(?=value)", ""),
        ("[[:alpha:]]", ""),
        (r"(x)\1", ""),
    ] {
        assert!(
            regex_expression(pattern, flags).is_none(),
            "{pattern}/{flags}"
        );
    }
}

#[test]
fn public_analysis_rejects_mixed_languages_in_either_order() {
    use crate::source_adapter::{SourceAdapter, SourceFile, SourceLanguage};
    let dir = tempfile::tempdir().unwrap();
    let ruby = super::RUBY_ADAPTER
        .extract(
            "Given('same') { work() }",
            &SourceFile {
                path: dir.path().join("steps.rb"),
                language: SourceLanguage::Ruby,
            },
        )
        .unwrap()
        .definitions
        .remove(0);
    let js = crate::typescript::extract(
        "Given('same', () => work())",
        &SourceFile {
            path: dir.path().join("steps.js"),
            language: SourceLanguage::JavaScript,
        },
    )
    .unwrap()
    .remove(0);
    let config = crate::config::Config::load(dir.path(), Default::default()).unwrap();
    for pair in [vec![ruby.clone(), js.clone()], vec![js, ruby]] {
        let error = crate::analysis::analyze(pair, vec![], &config).unwrap_err();
        assert!(error.to_string().contains("separate analysis runs"));
    }
}

#[test]
fn ruby_ascii_punctuation_identity_escapes_remain_literal() {
    for punctuation in (0x21_u8..=0x7e)
        .map(char::from)
        .filter(char::is_ascii_punctuation)
    {
        for pattern in [
            format!(r"\A\{punctuation}\z"),
            format!(r"\A[\{punctuation}]\z"),
        ] {
            let converted = regex_expression(&pattern, "").unwrap();
            let regex = regex::Regex::new(&converted).unwrap();
            assert!(
                regex.is_match(&punctuation.to_string()),
                "{pattern}: {converted}"
            );
            assert!(!regex.is_match("a"), "{pattern}: {converted}");
        }
    }
}

#[test]
fn unresolved_owner_effects_are_reported_without_adjacent_registrations() {
    use crate::source_adapter::{
        ExtractionDiagnosticKind, SourceAdapter, SourceFile, SourceLanguage,
    };
    for source in [
        "module Ghost::Helpers; end",
        "autoload :Loader, 'provider'",
        "def Given(x); :local; end",
    ] {
        let extraction = super::RUBY_ADAPTER
            .extract(
                source,
                &SourceFile {
                    path: "support.rb".into(),
                    language: SourceLanguage::Ruby,
                },
            )
            .unwrap();
        assert!(extraction.definitions.is_empty());
        assert!(
            extraction
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.kind == ExtractionDiagnosticKind::Incomplete),
            "{source}: {:?}",
            extraction.diagnostics
        );
    }
}

#[test]
fn parameter_type_metadata_is_available_without_running_a_transformer() {
    use crate::source_adapter::{SourceAdapter, SourceFile, SourceLanguage};
    let file = SourceFile {
        path: "support.rb".into(),
        language: SourceLanguage::Ruby,
    };
    for source in [
        "ParameterType(name: 'channel', regexp: /stdout/, transformer: ->(value) { value })",
        "ParameterType({:name => 'channel', :regexp => /stdout/, :transformer => proc { |value| value }})",
        "ParameterType( # explanation\n {name: 'channel', # comment\n regexp: [ /stdout/ # element\n ], transformer: ->(value) { value }})",
    ] {
        let extraction = super::RUBY_ADAPTER.extract(source, &file).unwrap();
        assert!(extraction.diagnostics.is_empty(), "{source}: {:?}", extraction.diagnostics);
        assert_eq!(extraction.parameter_types.len(), 1);
        assert_eq!(extraction.parameter_types[0].name.as_deref(), Some("channel"));
        let pattern = extraction.parameter_types[0].expression.as_ref().unwrap();
        let matcher = regex::Regex::new(pattern).unwrap();
        assert!(matcher.is_match("stdout"));
        assert!(!matcher.is_match("other"));
    }
    let extraction = super::RUBY_ADAPTER
        .extract(
            "ParameterType(name: 'int', regexp: /stdout/, transformer: ->(value) { value })",
            &file,
        )
        .unwrap();
    assert!(!extraction.diagnostics.is_empty());
}

#[test]
fn malformed_ruby_literal_boundaries_preserve_valid_values() {
    for (raw, expected) in [
        ("", None),
        ("'", None),
        ("\"", None),
        ("'value", None),
        ("''", Some("")),
        ("\"\"", Some("")),
        ("'value'", Some("value")),
        ("\"value\"", Some("value")),
        ("'日本'", Some("日本")),
        (r#""\u{1f600 41}""#, Some("😀A")),
        (r#""\u{}""#, Some("")),
        (r#""\u{ 41 }""#, Some("A")),
        (r#""\u0041""#, Some("A")),
        (r#""\uD800""#, None),
        (r#""\u{110000}""#, None),
        (r#""\u{0000041}""#, None),
        (r#""\u12""#, None),
        (r#""\u{41""#, None),
        ("\"\\u{41\n42}\"", None),
        ("\"a\\nb\"", Some("a\nb")),
    ] {
        assert_eq!(super::literal_string(raw).as_deref(), expected, "{raw:?}");
    }
}

#[test]
fn ruby_review_extraction_is_in_source_order() {
    use crate::source_adapter::{SourceAdapter, SourceFile, SourceLanguage};
    let extraction = super::RUBY_ADAPTER.extract(
        "Given('first') { work() }\nGiven(dynamic) { work() }\nThen('last') { finish() }\nThen(other) { finish() }",
        &SourceFile { path: "steps.rb".into(), language: SourceLanguage::Ruby },
    ).unwrap();
    assert_eq!(
        extraction
            .definitions
            .iter()
            .map(|d| d.location.line)
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(
        extraction
            .diagnostics
            .iter()
            .map(|d| d.location.line)
            .collect::<Vec<_>>(),
        [2, 4]
    );
}
