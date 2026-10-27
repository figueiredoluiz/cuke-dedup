use super::super::load_phase::tests::parse;
use super::*;

/// Asserts the per-file index answers every handler-shaped node exactly as the root walk
/// does: the locals before it, by set and by name, and the stability of every name the file
/// mentions for each block a registration call defers. Returns the handler and registration
/// block counts.
fn assert_index_matches_walk(source: &str, label: &str) -> (usize, usize) {
    let tree = parse(source);
    let root = tree.root_node();
    let scope = RootScope::new(root, source);
    let mut names: BTreeSet<String> = descendants(root)
        .into_iter()
        .filter(|node| node.kind() == "identifier")
        .map(|node| text(node, source).to_owned())
        .collect();
    names.insert("<source-file>".to_owned());
    let (mut checked, mut deferred) = (0, 0);
    for node in descendants(root) {
        if !matches!(node.kind(), "block" | "do_block" | "lambda" | "method") {
            continue;
        }
        let (mut indexed, mut looked_up, mut walked) = (false, false, false);
        let before = locals_before(root, node, source, &mut walked);
        assert_eq!(
            scope.locals_before(node, &mut indexed),
            before,
            "{label}: names"
        );
        assert_eq!(indexed, walked, "{label}: uncertain");
        let outer = scope.outer_locals(Some(node.start_byte()), &mut looked_up);
        for name in &names {
            assert_eq!(
                outer.contains(name),
                before.contains(name),
                "{label}: outer {name}"
            );
        }
        assert_eq!(looked_up, walked, "{label}: outer uncertain");
        checked += 1;
        if !registration_block(node, source) {
            continue;
        }
        let none = BTreeSet::new();
        for name in &names {
            let single = BTreeSet::from([name.clone()]);
            assert_eq!(
                scope.stable_capture(name),
                stable_captures(node, root, source, &single, &none),
                "{label}: stable {name}"
            );
        }
        assert_eq!(
            names.iter().all(|name| scope.stable_capture(name)),
            stable_captures(node, root, source, &names, &none),
            "{label}: stable set"
        );
        deferred += 1;
    }
    assert_eq!(
        scope.constants(),
        &file_constants(root, source),
        "{label}: constants"
    );
    (checked, deferred)
}

/// The index equals the walk for every assignment form, nested scope, enclosing block and
/// local-binding syntax, before and after each handler.
#[test]
fn root_index_matches_the_walk_on_the_matrix() {
    let source = r#"
LIMIT = 5
first, *rest = [1, 2, 3]
total = 0
total += 1
Given('first') { work(total) }
if ready
  inside_if = 1
end
begin
  inside_begin = 2
rescue => error
  handled = error
end
def helper
  hidden_in_def = 3
end
class Holder
  hidden_in_class = 4
end
module Space
  hidden_in_module = 5
end
class << self
  hidden_in_singleton = 6
end
[1].each { |item| hidden_in_block = item }
3.times do |index|
  inside_loop = index
  Given("loop #{index}") { work(inside_loop) }
end
callable = -> { hidden_in_lambda = 7 }
When('middle') do
  work(callable, inside_if)
end
for element in [1]
  looped = element
end
case value
in { name: }
  matched = name
end
/(?<group>x)/ =~ text
after = 8
Then('last') { work(after) }
def step_method
  Given('in method') { work }
end
"#;
    assert!(assert_index_matches_walk(source, "matrix").0 >= 10);
}

/// The cached stability verdict equals the walk for every way a file-level local can be
/// written, read, shadowed or captured, and the handler shapes that keep their own walk
/// are compared through the fallback.
#[test]
fn root_stability_matches_the_walk_on_the_matrix() {
    let source = r#"
literal = 1
Given('literal') { work(literal) }
nonliteral = make
Given('nonliteral') { work(nonliteral) }
twice = 1
twice = 2
Given('twice') { work(twice) }
later = 1
Given('later') { work(later) }
later = 3
plus = 1
plus += 1
Given('plus') { work(plus) }
inner = 1
Given('inner write') { inner = 2 }
Given('inner read') { work(inner) }
text = "mutable"
Given('text') { work(text) }
[1].each { work(text) }
frozen = :symbol
[1].each { work(frozen) }
Given('frozen') { work(frozen) }
shadowed = 1
[1].each { |shadowed| shadowed = 2 }
Given('shadowed') { work(shadowed) }
local = 1
Given('block local') { |;local| local = 5 }
Given('local') { work(local) }
Given('file') { work(__FILE__) }
CONST = 1
Given('const') { work(CONST) }
first, second = 1, 2
Given('destructured') { work(first, second) }
Given('before') { work(after_handler) }
after_handler = 1
step = lambda { work(text) }
Given('lambda', &step)
callable = -> { work(frozen) }
Before { setup = 1 }
Given('setup') { work(setup) }
def helper_step
  Given('in def') { work(literal) }
end
3.times do
  nested = 4
  Given('nested') { work(nested) }
end
"#;
    let (checked, deferred) = assert_index_matches_walk(source, "stability");
    assert!(
        checked >= 24 && deferred >= 18,
        "{checked} handlers, {deferred} deferred"
    );
}

/// A handler that no registration call defers keeps its own walk: the reads in its own body
/// do not count for it, while they do count for every registration block, whose cached
/// verdict therefore differs from the lambda's.
#[test]
fn handlers_outside_registration_calls_keep_their_own_walk() {
    let source = "text = \"mutable\"\nstep = lambda { work(text) }\nGiven('lambda', &step)\nGiven('block') { work(text) }\n";
    let tree = parse(source);
    let root = tree.root_node();
    let scope = RootScope::new(root, source);
    let mut handlers: Vec<_> = descendants(root)
        .into_iter()
        .filter(|node| matches!(node.kind(), "lambda" | "block"))
        .collect();
    handlers.sort_by_key(Node::start_byte);
    let [lambda, block]: [Node<'_>; 2] = handlers.try_into().unwrap();
    assert!(!Bindings::collect(lambda, &scope, source).uncertain);
    assert!(Bindings::collect(block, &scope, source).uncertain);
    assert!(!scope.stable_capture("text"));
}

/// The index equals the walk for every handler-shaped node in every Ruby fixture.
#[test]
fn root_index_matches_the_walk_on_every_ruby_fixture() {
    let mut files = Vec::new();
    let mut pending = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures")];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rb") {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(files.len() > 100, "Ruby fixtures not found");
    let (mut checked, mut deferred) = (0, 0);
    for path in &files {
        let source = std::fs::read_to_string(path).unwrap();
        let counts = assert_index_matches_walk(&source, &path.display().to_string());
        checked += counts.0;
        deferred += counts.1;
    }
    assert!(
        checked > 100 && deferred > 100,
        "{checked} handlers, {deferred} deferred"
    );
}
