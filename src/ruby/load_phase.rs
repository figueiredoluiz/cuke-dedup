//! Closed load phase: an allowlist of load-time statement shapes under which instance-method bodies
//! of suite classes cannot run while support and step files load.
//!
//! The load phase of a file is its top-level statements and, recursively, the bodies of the class
//! and module declarations among them. Handler blocks and method bodies are not part of it. When
//! every load-phase statement of every analyzed file is allowed and every load resolves, the only
//! load-time execution is the runner's DSL entry points called with literal arguments (which store
//! blocks without calling them), loading other closed files, and creating new classes and modules
//! with instance methods. No load-time code then calls a user method, creates an instance, or hands
//! a user class to other code, so dynamic dispatch inside those methods cannot change registration.
//! Assumption: a suite class whose name is not a Ruby core constant is not also defined and used
//! during load by the runner or its dependencies.

use super::{registration, text};
use tree_sitter::Node;

/// Constants that exist before suite files load, so a suite declaration of one reopens it: the
/// top-level constants of Ruby 4.0.5 with RubyGems (`Object.constants`), plus the standard
/// libraries, Bundler and Cucumber dependency namespaces the runner loads first. Sorted.
const CORE_CONSTANTS: [&str; 155] = [
    "ARGF",
    "ARGV",
    "ArgumentError",
    "Array",
    "BasicObject",
    "BigDecimal",
    "Binding",
    "Builder",
    "Bundler",
    "CGI",
    "CROSS_COMPILING",
    "Class",
    "ClosedQueueError",
    "Comparable",
    "Complex",
    "ConditionVariable",
    "Cucumber",
    "Data",
    "Date",
    "DateTime",
    "DidYouMean",
    "Diff",
    "Digest",
    "Dir",
    "ENV",
    "EOFError",
    "ERB",
    "Encoding",
    "EncodingError",
    "Enumerable",
    "Enumerator",
    "Errno",
    "ErrorHighlight",
    "Etc",
    "Exception",
    "FalseClass",
    "Fiber",
    "FiberError",
    "File",
    "FileTest",
    "FileUtils",
    "Float",
    "FloatDomainError",
    "Forwardable",
    "FrozenError",
    "GC",
    "Gem",
    "Gherkin",
    "Hash",
    "IO",
    "IOError",
    "IndexError",
    "Integer",
    "Interrupt",
    "JSON",
    "Kernel",
    "KeyError",
    "LoadError",
    "LocalJumpError",
    "Logger",
    "Marshal",
    "MatchData",
    "Math",
    "Method",
    "MiniMime",
    "Module",
    "Monitor",
    "MonitorMixin",
    "MultiTest",
    "Mutex",
    "NameError",
    "NilClass",
    "NoMatchingPatternError",
    "NoMatchingPatternKeyError",
    "NoMemoryError",
    "NoMethodError",
    "NotImplementedError",
    "Numeric",
    "Object",
    "ObjectSpace",
    "Open3",
    "OpenStruct",
    "OptionParser",
    "PP",
    "Pathname",
    "PrettyPrint",
    "Proc",
    "Process",
    "Psych",
    "Queue",
    "RUBYGEMS_ACTIVATION_MONITOR",
    "RUBY_COPYRIGHT",
    "RUBY_DESCRIPTION",
    "RUBY_ENGINE",
    "RUBY_ENGINE_VERSION",
    "RUBY_PATCHLEVEL",
    "RUBY_PLATFORM",
    "RUBY_RELEASE_DATE",
    "RUBY_REVISION",
    "RUBY_VERSION",
    "Ractor",
    "Random",
    "Range",
    "RangeError",
    "Rational",
    "RbConfig",
    "Refinement",
    "Regexp",
    "RegexpError",
    "Ruby",
    "RubyVM",
    "RuntimeError",
    "STDERR",
    "STDIN",
    "STDOUT",
    "ScriptError",
    "SecureRandom",
    "SecurityError",
    "Set",
    "Shellwords",
    "Signal",
    "SignalException",
    "SingleForwardable",
    "Singleton",
    "SizedQueue",
    "StandardError",
    "StopIteration",
    "String",
    "StringIO",
    "Struct",
    "Symbol",
    "SyntaxError",
    "SyntaxSuggest",
    "Sys",
    "SystemCallError",
    "SystemExit",
    "SystemStackError",
    "TOPLEVEL_BINDING",
    "Tempfile",
    "Thread",
    "ThreadError",
    "ThreadGroup",
    "Time",
    "Timeout",
    "TracePoint",
    "TrueClass",
    "TypeError",
    "URI",
    "UnboundMethod",
    "UncaughtThrowError",
    "UnicodeNormalize",
    "Warning",
    "YAML",
    "ZeroDivisionError",
    "Zlib",
];

/// Scenario hooks: their blocks run only during scenarios, like step handlers.
const SCENARIO_HOOKS: [&str; 5] = ["After", "AfterStep", "Around", "Before", "BeforeStep"];

/// Whether `name` is a scenario hook, whose block runs only during scenarios. Hooks that can run
/// around loading or configuration (`BeforeAll`, `AfterAll`, `AfterConfiguration`,
/// `InstallPlugin`) are not.
pub(super) fn scenario_hook(name: &str) -> bool {
    SCENARIO_HOOKS.contains(&name)
}

/// Whether every load-phase statement of `program` is an allowed shape. A file with syntax errors
/// is never closed.
pub(super) fn closed(program: Node<'_>, source: &str) -> bool {
    !program.has_error() && statements_allowed(program, source, false)
}

/// The class or module declaration whose body directly holds the instance-method definition
/// that `call` sits in, if any. The caller decides whether that declaration is in the load phase.
pub(super) fn contained_scope(call: Node<'_>) -> Option<Node<'_>> {
    ancestors(call)
        .find(|node| {
            matches!(
                node.kind(),
                "method" | "singleton_method" | "class" | "module" | "program"
            )
        })
        .filter(|method| method.kind() == "method")
        .and_then(|method| method.parent())
        .filter(|body| body.kind() == "body_statement")
        .and_then(|body| body.parent())
        .filter(|scope| matches!(scope.kind(), "class" | "module"))
}

/// Top-level `require`/`require_relative` calls of `program`.
pub(super) fn load_requires<'a>(
    program: Node<'a>,
    source: &'a str,
) -> impl Iterator<Item = Node<'a>> + 'a {
    let mut cursor = program.walk();
    let statements: Vec<_> = program.named_children(&mut cursor).collect();
    statements.into_iter().filter(move |statement| {
        statement.kind() == "call"
            && statement
                .child_by_field_name("method")
                .is_some_and(|method| {
                    matches!(text(method, source), "require" | "require_relative")
                })
    })
}

/// The strict ancestors of `node`, nearest first.
fn ancestors(node: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    std::iter::successors(node.parent(), Node::parent)
}

/// Whether every named statement under `scope` is allowed; `in_declaration` admits instance
/// methods (declaration bodies) and rejects calls (top level only).
fn statements_allowed(scope: Node<'_>, source: &str, in_declaration: bool) -> bool {
    let mut cursor = scope.walk();
    let statements: Vec<_> = scope.named_children(&mut cursor).collect();
    statements
        .into_iter()
        .all(|statement| match statement.kind() {
            "comment" => true,
            "class" | "module" => declaration_allowed(statement, source, in_declaration),
            "method" => in_declaration,
            "call" => !in_declaration && call_allowed(statement, source),
            _ => false, // fail-closed: any other load-phase statement may run arbitrary code.
        })
}

/// `class C` / `module C` without a superclass, not reopening a core constant or a protected
/// namespace, whose body holds only comments, instance methods and allowed declarations.
fn declaration_allowed(declaration: Node<'_>, source: &str, nested: bool) -> bool {
    let fresh = declaration.child_by_field_name("superclass").is_none()
        && declaration
            .child_by_field_name("name")
            .is_some_and(|name| fresh_path(text(name, source), nested));
    fresh
        && declaration
            .child_by_field_name("body")
            .is_none_or(|body| statements_allowed(body, source, true))
}

/// A constant path whose segments are plain constants and that cannot name a Ruby core constant or
/// a protected namespace. A single-segment name declared inside another declaration is always a
/// new constant of that namespace (`module Support; class String` creates `Support::String`);
/// a top-level, absolute (`::X`) or qualified name is looked up, so its full spelling and first
/// segment must not be core or protected.
fn fresh_path(path: &str, nested: bool) -> bool {
    let absolute = path.starts_with("::");
    let path = path.strip_prefix("::").unwrap_or(path);
    let first = path.split("::").next().unwrap_or_default();
    let looked_up = !nested || absolute || path.contains("::");
    !path.is_empty()
        && path.split("::").all(|segment| {
            segment
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
        && (!looked_up
            || [path, first].iter().all(|name| {
                CORE_CONSTANTS.binary_search(name).is_err()
                    && !super::ownership::protected_namespace(name)
            }))
}

/// A receiverless registration, hook or literal load whose arguments execute no user code.
fn call_allowed(call: Node<'_>, source: &str) -> bool {
    let Some(name) = call
        .child_by_field_name("receiver")
        .is_none()
        .then(|| call.child_by_field_name("method"))
        .flatten()
        .map(|method| text(method, source))
    else {
        return false;
    };
    let arguments: Vec<_> = call
        .child_by_field_name("arguments")
        .map(|arguments| {
            let mut cursor = arguments.walk();
            arguments
                .named_children(&mut cursor)
                .filter(|argument| argument.kind() != "comment")
                .collect()
        })
        .unwrap_or_default();
    if matches!(name, "require" | "require_relative") {
        return call.child_by_field_name("block").is_none()
            && arguments.len() == 1
            && literal(arguments[0]);
    }
    (registration(name) || name == "ParameterType" || scenario_hook(name))
        && arguments.iter().all(|argument| match argument.kind() {
            "pair" => pair_allowed(*argument, source),
            "hash" => {
                let mut cursor = argument.walk();
                let pairs: Vec<_> = argument.named_children(&mut cursor).collect();
                pairs
                    .into_iter()
                    .all(|pair| pair.kind() == "comment" || pair_allowed(pair, source))
            }
            _ => literal(*argument),
        })
}

/// A hash pair with a symbol key and a literal or transformer value.
fn pair_allowed(pair: Node<'_>, source: &str) -> bool {
    pair.kind() == "pair"
        && pair
            .child_by_field_name("key")
            .is_some_and(|key| matches!(key.kind(), "hash_key_symbol" | "simple_symbol"))
        && pair
            .child_by_field_name("value")
            .is_some_and(|value| literal(value) || transformer(value, source))
}

/// A lambda literal, or a receiverless `lambda`/`proc` call with a block and no arguments.
fn transformer(value: Node<'_>, source: &str) -> bool {
    super::bindings::callable_literal(value, source).is_some()
}

/// A literal that evaluates without calling user code: an uninterpolated string, regex or symbol,
/// a number, `true`, `false` or `nil`.
fn literal(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "string"
            | "regex"
            | "simple_symbol"
            | "delimited_symbol"
            | "integer"
            | "float"
            | "true"
            | "false"
            | "nil"
    ) && !super::descendants(node)
        .iter()
        .any(|inner| inner.kind() == "interpolation")
}

#[cfg(test)]
mod tests {
    use super::{closed, contained_scope};

    /// Parses `source` as Ruby.
    fn parse(source: &str) -> tree_sitter::Tree {
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
        let source = "class Helper\n  def log(level)\n    logger.send(level)\n  end\nend\nGiven('x') { work }\n";
        let file = crate::source_adapter::SourceFile {
            path: std::path::PathBuf::from("helper.rb"),
            language: crate::source_adapter::SourceLanguage::Ruby,
        };
        let extraction = crate::source_adapter::adapter_for_language(file.language)
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
            ("module M\n  class C\n    def a\n      [1].each { x.send(n) }\n    end\n  end\nend\n", true),
            ("class C\n  def self.a\n    x.send(n)\n  end\nend\n", false),
            ("def a\n  x.send(n)\nend\n", false),
            ("class C\n  def a\n    def b\n      x.send(n)\n    end\n  end\nend\n", false),
            ("class C\n  Object.class_exec do\n    def a\n      x.send(n)\n    end\n  end\nend\n", false),
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
}
