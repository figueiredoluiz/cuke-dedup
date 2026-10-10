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

/// Ruby string literals decode only when complete and valid; truncated quotes and invalid escapes
/// yield no value.
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

/// Whether `router.Given(arguments)`, with an optional brace block and trailing source lines, may
/// register, together with the tree-sitter kind of its first argument.
fn receiver_call(arguments: &str, block: bool, tail: &str) -> (bool, String) {
    let block = if block { " { }" } else { "" };
    let source = format!("router.Given({arguments}){block}\n{tail}");
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(&source, None).unwrap();
    let call = super::descendants(tree.root_node())
        .into_iter()
        .find(|node| {
            node.kind() == "call"
                && node
                    .child_by_field_name("method")
                    .is_some_and(|method| super::text(method, &source) == "Given")
        })
        .unwrap();
    let first = call
        .child_by_field_name("arguments")
        .and_then(|arguments| {
            super::bindings::semantic_children(arguments)
                .first()
                .copied()
        })
        .map_or_else(String::new, |node| node.kind().to_owned());
    (super::may_register(call, &source), first)
}

/// Every expression kind tree-sitter-ruby allows in an argument list is classified by the value
/// Ruby gives it. Each row states the kind its sample parses to and whether the call may register
/// as the first argument with a block, the second argument without one, and the second with one.
/// Rows with a fixed value class (pattern 100, handler 011, `nil`/`false` 001, other 000) were
/// checked against a cucumber-ruby registrar stub; unknown values (111) fail closed.
#[test]
fn receiver_registration_follows_the_value_of_every_argument_kind() {
    let rows: &[(&str, &str, &str, &str)] = &[
        ("string", "'a'", "", "100"),
        ("chained_string", "'a' 'b'", "", "100"),
        ("character", "?a", "", "100"),
        ("regex", "/a/", "", "100"),
        ("heredoc_beginning", "<<~T", "x\nT\n", "100"),
        ("heredoc_beginning", "<<~`T`", "ls\nT\n", "111"),
        ("identifier", "__FILE__", "", "100"),
        ("simple_symbol", ":a", "", "011"),
        ("delimited_symbol", ":\"a\"", "", "011"),
        ("lambda", "-> {}", "", "011"),
        ("method", "def m1; end", "", "011"),
        ("singleton_method", "def self.m2; end", "", "011"),
        ("nil", "nil", "", "001"),
        ("false", "false", "", "001"),
        ("test_pattern", "1 in String", "", "001"),
        ("true", "true", "", "000"),
        ("integer", "1", "", "000"),
        ("unary", "-1", "", "000"),
        ("unary", "+1.5", "", "000"),
        ("float", "1.5", "", "000"),
        ("rational", "1r", "", "000"),
        ("complex", "1i", "", "000"),
        ("array", "[1]", "", "000"),
        ("string_array", "%w[a]", "", "000"),
        ("symbol_array", "%i[a]", "", "000"),
        ("hash", "{}", "", "000"),
        ("range", "1..2", "", "000"),
        ("identifier", "__LINE__", "", "000"),
        ("identifier", "__ENCODING__", "", "000"),
        ("assignment", "y = :s", "", "011"),
        ("parenthesized_statements", "(1; :s)", "", "011"),
        ("parenthesized_statements", "()", "", "001"),
        // A jump is a void value expression Ruby rejects as an argument, so the file never loads;
        // tree-sitter parses it, and it stays unknown.
        ("break", "break", "", "111"),
        ("next", "next", "", "111"),
        ("redo", "redo", "", "111"),
        ("retry", "retry", "", "111"),
        ("return", "return", "", "111"),
        // Values only runtime decides.
        ("unary", "- 1", "", "111"),
        ("unary", "!x", "", "111"),
        ("unary", "defined?(x)", "", "111"),
        ("subshell", "`true`", "", "111"),
        ("identifier", "x", "", "111"),
        ("constant", "String", "", "111"),
        ("scope_resolution", "Object::String", "", "111"),
        ("self", "self", "", "111"),
        ("instance_variable", "@i", "", "111"),
        ("class_variable", "@@c", "", "111"),
        ("global_variable", "$g", "", "111"),
        ("call", "foo.to_s", "", "111"),
        ("element_reference", "a[0]", "", "111"),
        ("binary", "'a' + 'b'", "", "111"),
        ("operator_assignment", "x += 'b'", "", "111"),
        ("conditional", "true ? 'a' : 'b'", "", "111"),
        ("if", "if true then 'a' end", "", "111"),
        ("unless", "unless false then 'a' end", "", "111"),
        ("case", "case 1 when 1 then 'a' end", "", "111"),
        ("case_match", "case 1\nin 1 then 'a'\nend", "", "111"),
        ("while", "while false do end", "", "111"),
        ("until", "until true do end", "", "111"),
        ("for", "for i in [] do end", "", "111"),
        ("begin", "begin; 'a'; end", "", "111"),
        ("class", "class C1; 'a'; end", "", "111"),
        ("module", "module M1; 'a'; end", "", "111"),
        ("singleton_class", "class << self; 'a'; end", "", "111"),
        ("super", "super", "", "111"),
        ("yield", "yield", "", "111"),
        // `match_pattern` is reachable only in parentheses (bare `1 => q` is a hash pair) and is a
        // void value expression Ruby rejects as an argument; it stays unknown.
        ("parenthesized_statements", "(1 => q)", "", "111"),
    ];
    for (kind, expression, tail, expected) in rows {
        let (first, parsed) = receiver_call(expression, true, tail);
        let (second, _) = receiver_call(&format!("'x', {expression}"), false, tail);
        let (second_block, _) = receiver_call(&format!("'x', {expression}"), true, tail);
        assert_eq!(parsed, *kind, "{expression}");
        let actual: String = [first, second, second_block]
            .iter()
            .map(|registers| if *registers { '1' } else { '0' })
            .collect();
        assert_eq!(actual, *expected, "{kind}: {expression}");
    }
}

/// Keyword pairs and a non-empty `**hash` are one trailing Hash argument, a block argument other
/// than `&nil` is a possible handler, and each splat or `...` supplies any number of arguments
/// (a literal array splat supplies its elements). Every row was checked against a cucumber-ruby
/// registrar stub; `*a` with an unknown array may register and fails closed.
#[test]
fn receiver_registration_counts_keywords_blocks_and_splats() {
    for (arguments, block, expected) in [
        ("'x', key: 1", true, false),
        ("'x', :s, key: 1", false, true),
        ("'x', nil, {}, key: 1", true, false),
        ("'x', **{}", true, true),
        ("'x', **{key: 1}", true, false),
        ("'x', **opts", true, true),
        ("'x', **nil", true, true),
        ("'x', **({})", true, true),
        ("'x', **({key: 1})", true, false),
        ("'x', **{**{}}", true, true),
        ("'x', **{**{key: 1}}", true, false),
        ("'x', **{**h}", true, true),
        ("*['x', key: 1]", true, false),
        ("*['x', :s, key: 1]", false, true),
        ("*['x', *a]", true, true),
        ("*(['x', :s])", false, true),
        ("*(['x', 1])", false, false),
        ("*['x', **h]", true, true),
        ("*['x', **h]", false, false),
        ("*['x', **{key: 1}]", true, false),
        // A pattern test is false at runtime when the value does not match, and the block registers.
        ("'x', (x in Integer)", true, true),
        ("'x', &nil", false, false),
        ("'x', &(nil)", false, false),
        ("'x', &()", false, false),
        ("'x', &(1; nil)", false, false),
        ("'x', &((nil))", false, false),
        ("'x', &(1; false)", false, true),
        ("'x', &false", false, true),
        ("'x', &handler", false, true),
        ("*nil", true, false),
        ("*()", true, false),
        ("*(1; nil)", true, false),
        ("*[]", true, false),
        ("*['x', :s]", false, true),
        ("*['x', 1]", false, false),
        ("*a", true, true),
        ("'x', *a, 1, 2, 3", true, false),
        ("*a, 'x', 'y'", true, false),
        ("'x', *a, :s, *a, {}", false, true),
    ] {
        assert_eq!(
            receiver_call(arguments, block, "").0,
            expected,
            "{arguments}"
        );
    }
}
