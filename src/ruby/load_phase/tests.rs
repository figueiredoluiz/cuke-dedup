use super::{closed, contained_scope};

/// Parses `source` as Ruby.
pub(in crate::ruby) fn parse(source: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .unwrap();
    parser.parse(source, None).unwrap()
}

/// Each load-phase shape is closed or open as the allowlist states.
#[test]
fn load_phase_statements_follow_the_allowlist() {
    let cases = [
        ("# comment\n", true),
        ("# frozen_string_literal: true\nGiven('x') { work }\n", true),
        ("Given(/x (\\d+)/) do |n|\n  work(n)\nend\n", true),
        ("When(:symbol_step, 1, 2.5, true, false, nil) { work }\n", true),
        ("Then(%s{sym}) { work }\n", true),
        ("ParameterType(name: 'c', regexp: /r/, transformer: ->(c) { c })\n", true),
        ("ParameterType({ name: 'c', regexp: /r/, # note\n  transformer: proc { |c| c } })\n", true),
        ("ParameterType(name: 'c', regexp: /r/, transformer: lambda { |c| c })\n", true),
        ("Before('@a') do\n  setup\nend\nAfter { teardown }\nAround { |s, b| b.call }\n", true),
        ("require 'helper'\nrequire_relative '../support/helper'\n", true),
        ("module Support\n  module Inner\n    def a; end\n  end\n  class String\n    def b; end\n  end\nend\n", true),
        ("class ::Fresh\n  # note\n  def a; end\nend\nmodule Fresh::Nested\nend\n", true),
        ("class Empty; end\n", true),
        ("x = 1\n", false),
        ("puts 'x'\n", false),
        ("def top; end\n", false),
        ("World(Support)\n", false),
        ("BEGIN { work }\n", false),
        ("Given('x') { work }\n__END__\ndata\n", false),
        ("Given(\"#{x}\") { work }\n", false),
        ("Given(TEXT) { work }\n", false),
        ("Given(*steps) { work }\n", false),
        ("Given(text: 'x', other: TEXT) { work }\n", false),
        ("Given('x', &handler)\n", false),
        ("ParameterType(name: 'c', transformer: lambda(:x) { |c| c })\n", false),
        ("ParameterType(name: 'c', transformer: helper.lambda { |c| c })\n", false),
        ("ParameterType('name' => 'c')\n", false),
        ("helper.Given('x') { work }\n", false),
        ("require 'a', 'b'\n", false),
        ("require \"#{dir}/x\"\n", false),
        ("require('a') { work }\n", false),
        ("require_relative helper_path\n", false),
        ("class C < Base\nend\n", false),
        ("class String\n  def a; end\nend\n", false),
        ("module Kernel\nend\n", false),
        ("module Cucumber::Glue\nend\n", false),
        ("class Kernel::Helper\nend\n", false),
        ("class lower_case::Thing\nend\n", false),
        ("class C\n  include Comparable\nend\n", false),
        ("class C\n  def self.a; end\nend\n", false),
        ("class C\n  X = 1\nend\n", false),
        ("Given('x' { work }\n", false),
    ];
    for (source, expected) in cases {
        let tree = parse(source);
        assert_eq!(closed(tree.root_node(), source), expected, "{source}");
    }
}

/// The session-less path never contains a call: without finalization it cannot see the suite.
#[test]
fn session_less_extraction_keeps_the_conservative_rule() {
    let source =
        "class Helper\n  def log(level)\n    logger.send(level)\n  end\nend\nGiven('x') { work }\n";
    let file = crate::source_adapter::SourceFile {
        path: std::path::PathBuf::from("helper.rb"),
        language: crate::ruby::RUBY,
    };
    let extraction = crate::source_adapter::adapter_for_language(file.language)
        .unwrap()
        .extract(source, &file)
        .unwrap();
    assert!(
        extraction
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.location.line == 3
                && diagnostic
                    .message
                    .contains("ownership or executable source effects are unresolved")),
        "{:?}",
        extraction.diagnostics
    );
}

/// A call has a containing declaration only directly in an instance method of a class or
/// module body; singleton methods, top-level defs, nested defs and blocks have none.
#[test]
fn contained_calls_sit_in_instance_methods_of_load_phase_declarations() {
    let cases = [
        ("class C\n  def a\n    x.send(n)\n  end\nend\n", true),
        (
            "module M\n  class C\n    def a\n      [1].each { x.send(n) }\n    end\n  end\nend\n",
            true,
        ),
        ("class C\n  def self.a\n    x.send(n)\n  end\nend\n", false),
        ("def a\n  x.send(n)\nend\n", false),
        (
            "class C\n  def a\n    def b\n      x.send(n)\n    end\n  end\nend\n",
            false,
        ),
        (
            "class C\n  Object.class_exec do\n    def a\n      x.send(n)\n    end\n  end\nend\n",
            false,
        ),
        ("class C\n  x.send(n)\nend\n", false),
        ("x.send(n)\n", false),
    ];
    for (source, expected) in cases {
        let tree = parse(source);
        let call = super::super::descendants(tree.root_node())
            .into_iter()
            .find(|node| {
                node.kind() == "call"
                    && node
                        .child_by_field_name("method")
                        .is_some_and(|method| &source[method.byte_range()] == "send")
            })
            .unwrap();
        assert_eq!(contained_scope(call).is_some(), expected, "{source}");
    }
}
