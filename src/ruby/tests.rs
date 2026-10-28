use super::regex_expression;
use crate::typescript::JAVASCRIPT;

/// Verify Ruby fingerprints retain value and callee differences while normalizing superficial formatting and local renames.
#[test]
fn handler_fingerprints_preserve_superficial_value_and_callee_boundaries() {
    use crate::source_adapter::{SourceAdapter, SourceFile};
    let file = SourceFile {
        path: "steps.rb".into(),
        language: crate::ruby::RUBY,
    };
    let extract = |source: &str| {
        super::RUBY_ADAPTER
            .extract(source, &file)
            .unwrap()
            .definitions
    };

    let definitions = extract(
        "Given('commented') { # note\n work() }\nGiven('plain') { work() }\nGiven('delimited') do work() end",
    );
    assert!(definitions
        .iter()
        .all(|definition| definition.handler.exact.len() == definition.handler.normalized.len()));
    assert_ne!(definitions[0].handler.exact, definitions[1].handler.exact);
    assert_eq!(
        definitions[0].handler.normalized,
        definitions[1].handler.normalized
    );
    assert_ne!(definitions[1].handler.exact, definitions[2].handler.exact);
    assert_eq!(
        definitions[1].handler.normalized,
        definitions[2].handler.normalized
    );

    let renamed =
        extract("Given('first') { |z, a| z.goto(a) }\nGiven('second') { |b, y| b.goto(y) }");
    assert_eq!(
        renamed[0].handler.alpha_normalized,
        renamed[1].handler.alpha_normalized
    );
    assert_eq!(
        renamed[0].handler.behavior_signature,
        renamed[1].handler.behavior_signature
    );

    let literals = extract(
        "Given('save') { page.locator('#save') }\nGiven('saves') { page.locator('#saves') }",
    );
    assert_ne!(
        literals[0].handler.alpha_normalized,
        literals[1].handler.alpha_normalized
    );
    assert_eq!(
        literals[0].handler.structural,
        literals[1].handler.structural
    );

    let calls = extract(
        "Given('direct') { page.click('#save') }\nGiven('fluent') { page.locator('#save').click }\nGiven('factory') { browser().click('#save') }\nGiven('callback') { |perform| perform.call }",
    );
    for (definition, expected) in calls.iter().zip([
        "page.#click",
        "page.#click",
        "browser.#click",
        "local:0.#call",
    ]) {
        assert!(
            definition
                .handler
                .behavior_signature
                .iter()
                .any(|event| event.contains(expected)),
            "{expected}: {:?}",
            definition.handler.behavior_signature
        );
    }

    let structural = extract(
        "Given('strip') { 'value'.strip }\nGiven('upcase') { 'value'.upcase }\nGiven('user') { render(\"#{user.name}\") }\nGiven('order') { render(\"#{order.id}\") }",
    );
    assert_ne!(
        structural[0].handler.behavior_signature,
        structural[1].handler.behavior_signature
    );
    assert_ne!(
        structural[2].handler.structural,
        structural[3].handler.structural
    );

    let forged = extract(
        "Given('two') { combine('m', 'n') }\nGiven('one') { combine(\"m') }, (string n\") }",
    );
    assert_ne!(
        forged[0].handler.alpha_normalized,
        forged[1].handler.alpha_normalized
    );

    let long = format!("{{\r\n# {}\r\nwork()\r\n}}", "#\r\n界".repeat(900));
    for handler_source in ["{\r\n# note\r\nwork()\r\n}".to_owned(), long] {
        let source = format!("Given('work') {handler_source}");
        let definition = &extract(&source)[0];
        let legacy_snippet_bytes = handler_source
            .chars()
            .take(2_000)
            .map(char::len_utf8)
            .sum::<usize>();
        assert_eq!(
            definition.handler.exact.len() + definition.handler.source_snippet.len(),
            definition.handler.normalized.len() + legacy_snippet_bytes,
            "{handler_source:?}"
        );
    }
}

/// Verify safe local registration aliases preserve final findings while uncertain or escaping aliases gain no trust.
#[test]
fn local_registration_aliases_preserve_final_findings_without_granting_unknown_trust() {
    use crate::model::Rule;
    use crate::source_adapter::{SourceAdapter, SourceFile};
    let dir = tempfile::tempdir().unwrap();
    let file = SourceFile {
        path: dir.path().join("steps.rb"),
        language: crate::ruby::RUBY,
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

/// Verify unresolved dependencies expose unknown usage through direct adapter extraction without dropping local definitions.
#[test]
fn unresolved_dependency_usage_is_exposed_by_the_direct_adapter_api() {
    use crate::source_adapter::{SourceAdapter, SourceFile};
    let extraction = super::RUBY_ADAPTER
        .extract(
            "require 'external/provider'; Given('local') { work() }",
            &SourceFile {
                path: "steps.rb".into(),
                language: crate::ruby::RUBY,
            },
        )
        .unwrap();
    assert_eq!(extraction.definitions.len(), 1);
    assert!(extraction.indirect_usage.unwrap().unknown);
}

/// Verify Ruby regex translation retains line anchors and ASCII classes while rejecting unsupported flags and constructs.
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
        ("(?<bad!>value)", ""),
        ("(", ""),
        ("[[:alpha:]]", ""),
        (r"(x)\1", ""),
    ] {
        assert!(
            regex_expression(pattern, flags).is_none(),
            "{pattern}/{flags}"
        );
    }
}

/// Verify public analysis rejects mixed Ruby and JavaScript definitions in either order.
#[test]
fn public_analysis_rejects_mixed_languages_in_either_order() {
    use crate::source_adapter::{SourceAdapter, SourceFile};
    let dir = tempfile::tempdir().unwrap();
    let ruby = super::RUBY_ADAPTER
        .extract(
            "Given('same') { work() }",
            &SourceFile {
                path: dir.path().join("steps.rb"),
                language: crate::ruby::RUBY,
            },
        )
        .unwrap()
        .definitions
        .remove(0);
    let js = crate::typescript::extract(
        "Given('same', () => work())",
        &SourceFile {
            path: dir.path().join("steps.js"),
            language: JAVASCRIPT,
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

/// Verify escaped ASCII punctuation remains literal both inside and outside Ruby regex character classes.
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

/// Verify unresolved owner effects report incompleteness even without neighboring registrations.
#[test]
fn unresolved_owner_effects_are_reported_without_adjacent_registrations() {
    use crate::source_adapter::{ExtractionDiagnosticKind, SourceAdapter, SourceFile};
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
                    language: crate::ruby::RUBY,
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

/// Verify static parameter-type metadata is extracted without running transformers and built-in name collisions produce diagnostics.
#[test]
fn parameter_type_metadata_is_available_without_running_a_transformer() {
    use crate::source_adapter::{SourceAdapter, SourceFile};
    let file = SourceFile {
        path: "support.rb".into(),
        language: crate::ruby::RUBY,
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

/// Verify parameter-type regexps accept every `%r` delimiter through the shared literal splitter,
/// including a nested pair and an escaped delimiter, and that an unterminated literal is rejected.
#[test]
fn parameter_type_regexps_accept_percent_r_delimiters() {
    use crate::source_adapter::{SourceAdapter, SourceFile};
    let file = SourceFile {
        path: "support.rb".into(),
        language: crate::ruby::RUBY,
    };
    for (literal, hit, miss) in [
        ("%r!std(out)!", "stdout", "std"),
        ("%r(std(out))", "stdout", "std"),
        ("%r[std[o]ut]", "stdout", "stu"),
        ("%r<std<out>>", "std<out>", "stdout"),
        ("%r{std{1}}", "std", "stdd"),
        ("%r|std\\|out|", "std|out", "stdout"),
        ("%r-std\\-out-", "std-out", "stdout"),
    ] {
        let source = format!(
            "ParameterType(name: 'channel', regexp: {literal}, transformer: ->(value) {{ value }})"
        );
        let extraction = super::RUBY_ADAPTER.extract(&source, &file).unwrap();
        assert!(
            extraction.diagnostics.is_empty(),
            "{literal}: {:?}",
            extraction.diagnostics
        );
        let pattern = extraction.parameter_types[0].expression.as_ref().unwrap();
        let matcher = regex::Regex::new(&format!("^(?:{pattern})$")).unwrap();
        assert!(matcher.is_match(hit), "{literal} hit");
        assert!(!matcher.is_match(miss), "{literal} miss");
    }
    let unterminated =
        "ParameterType(name: 'channel', regexp: %r(std(out), transformer: ->(value) { value })";
    let extraction = super::RUBY_ADAPTER.extract(unterminated, &file).unwrap();
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

/// Verify Ruby definitions and diagnostics each retain their source order.
#[test]
fn ruby_review_extraction_is_in_source_order() {
    use crate::source_adapter::{SourceAdapter, SourceFile};
    let extraction = super::RUBY_ADAPTER.extract(
        "Given('first') { work() }\nGiven(dynamic) { work() }\nThen('last') { finish() }\nThen(other) { finish() }",
        &SourceFile { path: "steps.rb".into(), language: crate::ruby::RUBY },
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

/// Extract one Ruby registration and return its matcher, flags and whether analysis stayed complete.
fn extract_matcher(literal: &str) -> (Vec<(String, String)>, bool) {
    use crate::source_adapter::{SourceAdapter, SourceFile};
    let file = SourceFile {
        path: "steps.rb".into(),
        language: crate::ruby::RUBY,
    };
    let extraction = super::RUBY_ADAPTER
        .extract(&format!("Given({literal}) {{ work() }}"), &file)
        .unwrap();
    (
        extraction
            .definitions
            .into_iter()
            .map(|d| (d.matcher, d.matcher_flags))
            .collect(),
        extraction.diagnostics.is_empty(),
    )
}

/// Verify every `%r` delimiter registers like `/…/`: bracket pairs nest by depth, an escaped
/// delimiter or backslash does not end the literal, and each row's matcher is Ruby 4.0's
/// `source` with the same match results.
#[test]
fn percent_r_delimiters_register_with_nesting_and_escapes() {
    // (literal, matcher, text Ruby matches, text Ruby does not match)
    for (literal, matcher, hit, miss) in [
        ("/a.c/", "a.c", "abc", "ac"),
        ("%r{a.c}", "a.c", "abc", "ac"),
        ("%r!a.c!", "a.c", "abc", "ac"),
        ("%r(a.c)", "a.c", "abc", "ac"),
        ("%r[a.c]", "a.c", "abc", "ac"),
        ("%r<a.c>", "a.c", "abc", "ac"),
        ("%r|a.c|", "a.c", "abc", "ac"),
        // Nested pairs close at depth zero, not at the first closer.
        ("%r(a(b)c)", "a(b)c", "abc", "ac"),
        ("%r[a[b]c]", "a[b]c", "abc", "ac"),
        ("%r<a<b>c>", "a<b>c", "a<b>c", "abc"),
        ("%r{a{2}c}", "a{2}c", "aac", "ac"),
        // An escaped bracket neither opens nor closes a level.
        (r"%r(a\(b)", r"a\(b", "a(b", "ab"),
        (r"%r(a\)b)", r"a\)b", "a)b", "ab"),
        (r"%r[a\]b]", r"a\]b", "a]b", "ab"),
        (r"%r<a\>b>", r"a\>b", "a>b", "ab"),
        (r"%r{a\}b}", r"a\}b", "a}b", "ab"),
        // An escaped non-bracket delimiter stays inside. Ruby drops its backslash unless the
        // character is regex syntax, which changes `[a\-z]` into the range `[a-z]`.
        (r"%r!a\!b!", "a!b", "a!b", "ab"),
        (r"/a\/b/", "a/b", "a/b", "ab"),
        // `/` is not the terminator here, so its backslash stays.
        (r"%r!a\/b!", r"a\/b", "a/b", "ab"),
        (r"%r,a\,b,", "a,b", "a,b", "ab"),
        (r"%r-\A[a\-z]\z-", r"\A[a-z]\z", "b", "-"),
        (r"%r|a\|b|", r"a\|b", "a|b", "a"),
        (r"%r^a\^b^", r"a\^b", "a^b", "ab"),
        (r"%r.a\.b.", r"a\.b", "a.b", "axb"),
        (r"%r?a\?b?", r"a\?b", "a?b", "ab"),
        (r"%r*a\*b*", r"a\*b", "a*b", "ab"),
        (r"%r+a\+b+", r"a\+b", "a+b", "ab"),
        (r"%r$a\$b$", r"a\$b", "a$b", "ab"),
        (r"%r)a\)b)", r"a\)b", "a)b", "ab"),
        (r"%r]a\]b]", r"a\]b", "a]b", "ab"),
        (r"%r}a\}b}", r"a\}b", "a}b", "ab"),
        (r"%r>a\>b>", r"a\>b", "a>b", "ab"),
        // An escaped backslash does not escape the delimiter after it.
        (r"%r(a\\b)", r"a\\b", r"a\b", "ab"),
        (r"%r!a\\!", r"a\\", r"a\", "a"),
        ("%r(a\nb)", "a\nb", "a\nb", "ab"),
    ] {
        let (definitions, complete) = extract_matcher(literal);
        assert!(complete, "{literal}");
        assert_eq!(
            definitions,
            [(matcher.to_owned(), String::new())],
            "{literal}"
        );
        let regex = regex::Regex::new(&regex_expression(matcher, "").unwrap()).unwrap();
        assert!(regex.is_match(hit), "{literal} hit");
        assert!(!regex.is_match(miss), "{literal} miss");
    }
    // Ruby 4.0 accepts every ASCII punctuation character as a symmetric delimiter, including
    // closing brackets, and drops the backslash of an escaped delimiter except for the regex
    // syntax characters (probe: `ruby -e 'p %r<c>a\<c>b<c>.source'`). tree-sitter-ruby 0.23
    // cannot parse `%r=`, and `\` as a delimiter is not modeled; both stay incomplete.
    let dropped = "!\"#%&',-/:;@_`~";
    for delimiter in ('!'..='~').filter(|c| c.is_ascii_punctuation() && !"([{<".contains(*c)) {
        let modeled = !"=\\".contains(delimiter);
        let (plain, escaped) = (
            format!("%r{delimiter}abc{delimiter}i"),
            format!("%r{delimiter}a\\{delimiter}b{delimiter}"),
        );
        let expect = |matcher: String, flags: &str| {
            if modeled {
                (vec![(matcher, flags.to_owned())], true)
            } else {
                (vec![], false)
            }
        };
        assert_eq!(
            extract_matcher(&plain),
            expect("abc".into(), "i"),
            "{plain}"
        );
        let source = if dropped.contains(delimiter) {
            format!("a{delimiter}b")
        } else {
            format!("a\\{delimiter}b")
        };
        assert_eq!(extract_matcher(&escaped), expect(source, ""), "{escaped}");
    }
}

/// Verify flags are collected after the closing delimiter for every form, and that literals
/// Ruby rejects or this adapter does not model stay incomplete instead of registering.
#[test]
fn percent_r_flags_and_unmodeled_literals_fail_closed() {
    for (raw, expected) in [
        ("/a/", Some(("a", ""))),
        ("/a/mi", Some(("a", "im"))),
        ("%r{a}iim", Some(("a", "im"))),
        ("%r!a!xm", Some(("a", "mx"))),
        ("%r(a)n", Some(("a", "n"))),
        ("%r a ", None),
        (r"%r\a\", None),
        // Unterminated, escaped closer, bare prefix, alphanumeric delimiter and other `%` literals.
        ("%r(a", None),
        ("%r(a(b)", None),
        ("%r!a", None),
        (r"%r(a\)", None),
        ("%r", None),
        ("%ra", None),
        ("%R(a)", None),
        ("%w(a)", None),
        ("(a)", None),
    ] {
        assert_eq!(
            super::regex_literal(raw),
            expected.map(|(pattern, flags)| (pattern.to_owned(), flags.to_owned())),
            "{raw}"
        );
    }
    // Control: the same registration is complete with a supported delimiter and incomplete when
    // tree-sitter-ruby reports a syntax error (invalid Ruby, or a valid whitespace or `=`
    // delimiter it cannot parse), the matcher interpolates, or a flag is unsupported.
    assert!(extract_matcher("%r(a)").1);
    for literal in [
        "%r|a|b|",
        "%r a ",
        r"%r\a\",
        "%r(a",
        "%r(a#{x})",
        "%r!a#{x}!",
        "/a#{x}/",
        "%r(a)z",
    ] {
        assert!(!extract_matcher(literal).1, "{literal}");
    }
}

/// Assert each raw Ruby literal decodes to the expected value (None when Ruby rejects it or the decoder leaves it undecoded).
fn assert_literal_rows(rows: &[(&str, Option<&str>)]) {
    for (raw, expected) in rows {
        assert_eq!(super::literal_string(raw).as_deref(), *expected, "{raw:?}");
    }
}

/// Verify literal forms other than plain quotes stay undecoded, even when they hold only plain text.
#[test]
fn ruby_non_quote_string_forms_stay_undecoded() {
    assert_literal_rows(&[
        ("%q(plain)", None),
        ("%Q(plain)", None),
        ("%(plain)", None),
        ("%q(a\\sb)", None),
        ("%Q(a\\sb)", None),
        (":\"plain\"", None),
        ("<<~TEXT", None),
        ("?a", None),
        ("`plain`", None),
        // Control: the same text in plain quotes decodes.
        ("\"plain\"", Some("plain")),
        ("'plain'", Some("plain")),
    ]);
}

/// Parse `source` and return its first string node's text through `read`.
fn first_string<T>(source: &str, read: impl Fn(tree_sitter::Node<'_>, &str) -> T) -> T {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    let node = super::descendants(tree.root_node())
        .into_iter()
        .find(|node| node.kind() == "string")
        .unwrap();
    read(node, source)
}

/// Verify an escaped `#` is not interpolation, so its string decodes, while a real interpolation and an unmodeled escape stay undecoded.
#[test]
fn ruby_escaped_hash_is_text_but_interpolation_is_not() {
    for (source, expected) in [
        ("x = \"\\#{x}\"", Some("#{x}")),
        ("x = \"\\#$x\"", Some("#$x")),
        ("x = \"fo\\x6f\"", Some("foo")),
        // Controls: interpolation and an unmodeled escape have no static value.
        ("x = \"#{x}\"", None),
        ("x = \"a#{x}\\sb\"", None),
        ("x = \"a\\cAb\"", None),
    ] {
        let name = first_string(source, super::static_method_name);
        assert_eq!(name.as_deref(), expected, "{source}");
    }
}

/// Extract the definitions and diagnostics of one Ruby source.
fn extract_ruby(source: &str) -> crate::source_adapter::Extraction {
    use crate::source_adapter::{SourceAdapter, SourceFile};
    super::RUBY_ADAPTER
        .extract(
            source,
            &SourceFile {
                path: "steps.rb".into(),
                language: crate::ruby::RUBY,
            },
        )
        .unwrap()
}

/// Verify matchers with decodable escapes register under every step keyword with Ruby's value.
#[test]
fn ruby_escaped_matchers_register_with_their_ruby_value() {
    for keyword in ["Given", "When", "Then"] {
        for (literal, expected) in [
            (r#""tab\sseparated""#, "tab separated"),
            (r#""caf\xC3\xA9""#, "caf\u{e9}"),
            ("\"joined \\\nline\"", "joined line"),
            // Control: a plain single-quoted matcher registers unchanged.
            (r#"'tab separated'"#, "tab separated"),
        ] {
            let source =
                format!("Given('first') {{ work() }}\n{keyword}({literal}) {{ other() }}\n");
            let extraction = extract_ruby(&source);
            assert_eq!(extraction.definitions.len(), 2, "{source}");
            assert!(extraction.diagnostics.is_empty(), "{source}");
            assert_eq!(extraction.definitions[1].matcher, expected, "{source}");
        }
    }
}

/// Verify matchers with unmodeled escapes or bytes that are not UTF-8 are diagnosed at their own line and column, and register nothing.
#[test]
fn ruby_unmodeled_escape_matchers_are_diagnosed_where_they_start() {
    for (literal, diagnostics) in [
        (r#""a\cAb""#, 1),
        (r#""a\C-ab""#, 1),
        (r#""a\M-ab""#, 1),
        (r#""a\M-\C-ab""#, 1),
        (r#""bad \xFF byte""#, 1),
        (r#""bad \351 byte""#, 1),
        ("%q(a\\sb)", 1),
        // Ruby rejects `\x` without a hex digit, so the file also carries a syntax error.
        (r#""bad \xg""#, 2),
        (r#""bad \x""#, 2),
    ] {
        let source = format!("\nGiven('first') {{ work() }}\n  When({literal}) {{ other() }}\n");
        let extraction = extract_ruby(&source);
        assert_eq!(extraction.definitions.len(), 1, "{source}");
        assert_eq!(extraction.diagnostics.len(), diagnostics, "{source}");
        let location = &extraction.diagnostics[0].location;
        assert_eq!((location.line, location.column), (3, 3), "{source}");
    }
}

/// Verify `step` calls use the decoded text, and that an unmodeled escape leaves usage unknown instead of dropping the call.
#[test]
fn ruby_step_usage_text_decodes_escapes() {
    let usage = |literal: &str| {
        extract_ruby(&format!("Given('outer') {{ step {literal} }}\n"))
            .indirect_usage
            .unwrap()
    };
    let decoded = usage(r#""tab\sseparated""#);
    assert!(!decoded.unknown);
    assert_eq!(
        decoded.texts.iter().map(String::as_str).collect::<Vec<_>>(),
        ["tab separated"]
    );
    // Control: an unmodeled escape yields no text and unknown usage.
    let unmodeled = usage(r#""tab\csseparated""#);
    assert!(unmodeled.unknown);
    assert!(unmodeled.texts.is_empty());
}

/// Verify `\a \b \e \f \s \v` and the existing `\n \r \t` decode at the start, middle, end and next to each other, with values taken from `ruby -e`.
#[test]
fn ruby_single_char_escapes_decode_to_their_ruby_values() {
    assert_literal_rows(&[
        (r##""a\sb""##, Some(r##"a b"##)),
        (r##""\sb""##, Some(r##" b"##)),
        (r##""a\s""##, Some(r##"a "##)),
        (r##""\s\s""##, Some(r##"  "##)),
        (r##""tab\sseparated""##, Some(r##"tab separated"##)),
        (r##""\a""##, Some("\u{7}")),
        (r##""x\ay""##, Some("x\u{7}y")),
        (r##""\b""##, Some("\u{8}")),
        (r##""x\by""##, Some("x\u{8}y")),
        (r##""\e""##, Some("\u{1b}")),
        (
            r##""the \e[1mbold\e[0m step""##,
            Some("the \u{1b}[1mbold\u{1b}[0m step"),
        ),
        (r##""\f""##, Some("\u{c}")),
        (r##""x\fy""##, Some("x\u{c}y")),
        (r##""\v""##, Some("\u{b}")),
        (r##""x\vy""##, Some("x\u{b}y")),
        (r##""\n""##, Some("\n")),
        (r##""\r""##, Some("\r")),
        (r##""\t""##, Some(r##"	"##)),
        (r##""\a\b\e\f\s\v""##, Some("\u{7}\u{8}\u{1b}\u{c} \u{b}")),
    ]);
}

/// Verify `\0`, `\nn` and `\nnn` octal escapes: digit count, trailing digits, the low eight bits of `\400` and `\777`, and `\8`/`\9` as plain characters.
#[test]
fn ruby_octal_escapes_take_up_to_three_digits_and_keep_the_low_byte() {
    assert_literal_rows(&[
        (r##""\0""##, Some("\u{0}")),
        (r##""a\0b""##, Some("a\u{0}b")),
        (r##""\00""##, Some("\u{0}")),
        (r##""\000""##, Some("\u{0}")),
        (r##""\0000""##, Some("\u{0}0")),
        (r##""\01""##, Some("\u{1}")),
        (r##""\0a""##, Some("\u{0}a")),
        (r##""\08""##, Some("\u{0}8")),
        (r##""\1""##, Some("\u{1}")),
        (r##""\7""##, Some("\u{7}")),
        (r##""\12""##, Some("\n")),
        (r##""\101""##, Some(r##"A"##)),
        (r##""\123""##, Some(r##"S"##)),
        (r##""\1234""##, Some(r##"S4"##)),
        (r##""\377""##, None), // invalid UTF-8: ff
        (r##""\400""##, Some("\u{0}")),
        (r##""\777""##, None), // invalid UTF-8: ff
        (r##""\401""##, Some("\u{1}")),
        (r##""\441""##, Some("!")),
        (r##""\378""##, Some("\u{1f}8")),
        (r##""\8""##, Some(r##"8"##)),
        (r##""\9""##, Some(r##"9"##)),
        (r##""a\101b""##, Some(r##"aAb"##)),
        (r##""\101\102""##, Some(r##"AB"##)),
    ]);
}

/// Verify `\xH` and `\xHH` decode, a third digit stays literal, and `\x` without a hex digit or an uppercase `\X` follow Ruby.
#[test]
fn ruby_hex_escapes_take_one_or_two_digits() {
    assert_literal_rows(&[
        (r##""\x41""##, Some(r##"A"##)),
        (r##""\x4""##, Some("\u{4}")),
        (r##""\x411""##, Some(r##"A1"##)),
        (r##""\x4g""##, Some("\u{4}g")),
        (r##""\xaB""##, None), // invalid UTF-8: ab
        (r##""\x7f""##, Some("\u{7f}")),
        (r##""\x00""##, Some("\u{0}")),
        (r##""\x1B[1mbold\x1B[0m""##, Some("\u{1b}[1mbold\u{1b}[0m")),
        (r##""a\x41b""##, Some(r##"aAb"##)),
        (r##""\x41\x42""##, Some(r##"AB"##)),
        (r##""\x""##, None),  // ruby: syntax error
        (r##""\xg""##, None), // ruby: syntax error
        (r##""\X41""##, Some(r##"X41"##)),
    ]);
}

/// Verify escapes decode as bytes: pairs that form UTF-8 decode, and a lone high byte or any other invalid sequence returns no value instead of a lossy conversion.
#[test]
fn ruby_escapes_that_do_not_form_utf8_fail_closed() {
    assert_literal_rows(&[
        (r##""\xC3\xA9""##, Some("\u{e9}")),
        (r##""\303\251""##, Some("\u{e9}")),
        (r##""\xc3\xa9\u00e9""##, Some("\u{e9}\u{e9}")),
        (r##""\u{e9}\xC3\xA9""##, Some("\u{e9}\u{e9}")),
        ("\"\u{e9}\\xC3\\xA9\"", Some("\u{e9}\u{e9}")),
        (r##""\xFF""##, None),       // invalid UTF-8: ff
        (r##""\xff""##, None),       // invalid UTF-8: ff
        (r##""\xE9""##, None),       // invalid UTF-8: e9
        (r##""\xC3""##, None),       // invalid UTF-8: c3
        (r##""\xC3(""##, None),      // invalid UTF-8: c328
        (r##""\351""##, None),       // invalid UTF-8: e9
        (r##""\u00e9\x80""##, None), // invalid UTF-8: c3a980
        (r##""\x80\u0041""##, None), // invalid UTF-8: 8041
        (r##""\u0041\x80""##, None), // invalid UTF-8: 4180
        (r##""\xC3\u00e9""##, None), // invalid UTF-8: c3c3a9
        (r##""\u00e9\351""##, None), // invalid UTF-8: c3a9e9
        (r##""\x80""##, None),       // invalid UTF-8: 80
    ]);
}

/// Verify an escape Ruby does not define yields the escaped character, including `\#`, `\d`, `\.` and a multibyte character.
#[test]
fn ruby_unknown_double_quoted_escapes_yield_the_escaped_character() {
    assert_literal_rows(&[
        (r##""\q""##, Some(r##"q"##)),
        (r##""\z""##, Some(r##"z"##)),
        (r##""\Q""##, Some(r##"Q"##)),
        (r##""\#""##, Some(r##"#"##)),
        (r##""\#{x}""##, Some(r##"#{x}"##)),
        (r##""\#$x""##, Some(r##"#$x"##)),
        (r##""\#@x""##, Some(r##"#@x"##)),
        (r##""\ ""##, Some(r##" "##)),
        (r##""\%""##, Some(r##"%"##)),
        (r##""\/""##, Some(r##"/"##)),
        (r##""\.""##, Some(r##"."##)),
        (r##""\d+""##, Some(r##"d+"##)),
        (r##""\w""##, Some(r##"w"##)),
        (r##""\{""##, Some(r##"{"##)),
        ("\"\\\u{e9}\"", Some("\u{e9}")),
        (r##""\U0041""##, Some(r##"U0041"##)),
        (r##""\'""##, Some(r##"'"##)),
        (r##""\"""##, Some(r##"""##)),
        (r##""\\""##, Some(r##"\"##)),
        (r##""\\n""##, Some(r##"\n"##)),
    ]);
}

/// Verify `\u` escapes keep their Ruby values and the forms Ruby rejects return no value, alone and mixed with byte escapes.
#[test]
fn ruby_unicode_escapes_match_ruby_and_reject_its_syntax_errors() {
    assert_literal_rows(&[
        ("\"\u{e9}\"", Some("\u{e9}")),
        (r##""\u00e9""##, Some("\u{e9}")),
        (r##""\u{e9}""##, Some("\u{e9}")),
        (r##""\u{1F600}""##, Some("\u{1f600}")),
        (r##""\u{1F600 1F601}""##, Some("\u{1f600}\u{1f601}")),
        (r##""\u{ 41 }""##, Some(r##"A"##)),
        (r##""\u{}""##, Some(r##""##)),
        (r##""\u0041\u0042""##, Some(r##"AB"##)),
        (r##""\u{41}\x42""##, Some(r##"AB"##)),
        (r##""\u041""##, None),       // ruby: syntax error
        (r##""\uD800""##, None),      // ruby: syntax error
        (r##""\u{D800}""##, None),    // ruby: syntax error
        (r##""\u{110000}""##, None),  // ruby: syntax error
        (r##""\u{41,42}""##, None),   // ruby: syntax error
        (r##""\u{0000041}""##, None), // ruby: syntax error
    ]);
}

/// Verify backslash-newline joins lines in double quotes, a source CRLF reads as LF in both quote styles, and a lone CR is kept.
#[test]
fn ruby_line_continuation_and_source_crlf_follow_the_ruby_lexer() {
    assert_literal_rows(&[
        ("\"a\\\nb\"", Some(r##"ab"##)),
        ("\"a\\\r\nb\"", Some(r##"ab"##)),
        ("\"a\\\n\"", Some(r##"a"##)),
        ("\"\\\n\"", Some(r##""##)),
        ("\"a\\\n  b\"", Some(r##"a  b"##)),
        ("\"a\\\rb\"", Some("a\rb")),
        ("\"a\nb\"", Some("a\nb")),
        ("\"a\r\nb\"", Some("a\nb")),
        ("'a\\\nb'", Some("a\\\nb")),
        ("'a\r\nb'", Some("a\nb")),
        ("'a\\\r\nb'", Some("a\\\nb")),
        ("\"\\\\\n\"", Some("\\\n")),
    ]);
}

/// Verify single-quoted strings unescape only `\\` and `\'` and keep every other backslash.
#[test]
fn ruby_single_quoted_strings_decode_only_backslash_and_quote_escapes() {
    assert_literal_rows(&[
        (r##"'a\sb'"##, Some(r##"a\sb"##)),
        (r##"'\\'"##, Some(r##"\"##)),
        (r##"'\''"##, Some(r##"'"##)),
        (r##"'\n'"##, Some(r##"\n"##)),
        (r##"'\q'"##, Some(r##"\q"##)),
        (r##"'\"'"##, Some(r##"\""##)),
        (r##"'\e'"##, Some(r##"\e"##)),
        (r##"'a\\\'b'"##, Some(r##"a\'b"##)),
        (r##"'\u0041'"##, Some(r##"\u0041"##)),
        (r##"'\x41'"##, Some(r##"\x41"##)),
        (r##"'\101'"##, Some(r##"\101"##)),
        (r##"'\0'"##, Some(r##"\0"##)),
        (r##"'\c'"##, Some(r##"\c"##)),
        (r##"'\M-a'"##, Some(r##"\M-a"##)),
        (r##"'\\n'"##, Some(r##"\n"##)),
        ("'\u{e9}'", Some("\u{e9}")),
    ]);
}

/// Verify `\cX`, `\C-X` and `\M-X` forms return no value even where Ruby accepts them, so the matcher stays indeterminate.
#[test]
fn ruby_control_and_meta_escapes_stay_undecoded() {
    assert_literal_rows(&[
        (r##""\c?""##, None),      // ruby: 7f
        (r##""\C-?""##, None),     // ruby: 7f
        (r##""\ca""##, None),      // ruby: 01
        (r##""\C-a""##, None),     // ruby: 01
        (r##""\M-a""##, None),     // ruby: e1
        (r##""\M-\C-a""##, None),  // ruby: 81
        (r##""\M-\ca""##, None),   // ruby: 81
        (r##""\c\M-a""##, None),   // ruby: 81
        (r##""\C-\M-a""##, None),  // ruby: 81
        (r##""\c\e""##, None),     // ruby: 1b
        (r##""\M-C\M-)""##, None), // ruby: c3a9
        (r##""x\cAy""##, None),    // ruby: 780179
        (r##""\M-\xC3""##, None),  // ruby: c3
    ]);
}

/// Ruby keeps normalized and alpha text for similarity while the exact identity is a digest
/// repeated to the legacy comparison-work length; an empty handler and non-ASCII text follow the
/// same encoding, and identical spelling keeps equal identities.
#[test]
fn handler_representations_stay_verbatim_with_a_padded_exact_digest() {
    use crate::source_adapter::{SourceAdapter, SourceFile};
    let file = SourceFile {
        path: "steps.rb".into(),
        language: crate::ruby::RUBY,
    };
    let definitions = super::RUBY_ADAPTER
        .extract(
            "Given('a') { }\nGiven('b') { puts 'caf\u{e9}' }\nGiven('c') { puts 'caf\u{e9}' }",
            &file,
        )
        .unwrap()
        .definitions;
    assert_eq!(definitions.len(), 3);
    for definition in &definitions {
        let handler = &definition.handler;
        assert!(handler.normalized.starts_with('['));
        assert!(handler.alpha_normalized.starts_with('['));
        // One-line handlers lose no bytes to snippet normalization, so the charge equals this.
        assert_eq!(handler.exact.len(), handler.normalized.len());
        assert!(handler.exact.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
    assert_eq!(definitions[1].handler.exact, definitions[2].handler.exact);
    assert_ne!(definitions[0].handler.exact, definitions[1].handler.exact);
    assert_ne!(
        definitions[0].handler.normalized,
        definitions[1].handler.normalized
    );
}
