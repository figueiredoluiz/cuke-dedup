use super::regex_expression;

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
        ("value", "i"),
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
        ("\"a\\nb\"", Some("a\nb")),
    ] {
        assert_eq!(super::literal_string(raw).as_deref(), expected, "{raw:?}");
    }
}
