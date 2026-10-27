import { test } from "node:test";
import assert from "node:assert/strict";
import { cfgValueWithoutTest, misplacedTestModules } from "./rust-scopes.mjs";

const PRODUCTION = "pub fn add(a: u8, b: u8) -> u8 {\n    a + b\n}\n";

/** Returns the reported lines for `tail` appended to a production prelude of three lines. */
function reported(tail) {
  return misplacedTestModules(PRODUCTION + tail).map(({ line }) => line);
}

// Each row is one placement of a test module in a production file; `expected` lists the lines the
// scan reports. Every reporting row has a control that differs in the one property it pins:
// inline against `mod tests;`, the predicate against one that holds without `test`, a real
// attribute against the same text inside a comment or string.
const LAYOUTS = [
  ["inline module", "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n", [5]],
  ["out-of-line `mod tests;`", "#[cfg(test)]\nmod tests;\n", []],
  ["visibility-qualified inline module", "#[cfg(test)]\npub(super) mod tests {\n}\n", [5]],
  ["visibility-qualified `mod tests;`", "#[cfg(test)]\npub(super) mod tests;\n", []],
  ["inline module under another name", "#[cfg(test)]\nmod wording {\n}\n", [5]],
  ["raw-identifier module name", "#[cfg(test)]\nmod r#async {\n}\n", [5]],
  ["out of line under another name", "#[cfg(test)]\nmod test_support;\n", [5]],
  ["raw-identifier `mod r#tests;` loads tests.rs", "#[cfg(test)]\nmod r#tests;\n", []],
  ["out of line through #[path]", "#[cfg(test)]\n#[path = \"x_tests.rs\"]\nmod tests;\n", [6]],
  ["attributes and docs between", "#[cfg(test)]\n#[allow(dead_code)]\n/// Fixtures.\nmod tests {\n}\n", [7]],
  ["comment line between", "#[cfg(test)]\n// helpers\nmod tests {\n}\n", [6]],
  ["blank line between", "#[cfg(test)]\n\nmod tests {\n}\n", [6]],
  ["multi-line attribute between", "#[cfg(test)]\n#[allow(\n    clippy::a,\n)]\nmod tests {\n}\n", [8]],
  ["multi-line cfg", "#[cfg(all(\n    test,\n    unix,\n))]\nmod tests {\n}\n", [8]],
  ["one line", "#[cfg(test)] mod tests {}\n", [4]],
  ["test-only predicate", "#[cfg(all(test, unix))]\nmod tests {\n}\n", [5]],
  // The test-only attribute comes first, so an evaluation that keeps only the last one misses it.
  ["stacked cfgs, one test-only", "#[cfg(test)]\n#[cfg(unix)]\nmod tests {\n}\n", [6]],
  // Neither attribute is test-only alone; together they hold only under `test`.
  ["stacked cfgs that are test-only together", "#[cfg(any(test, unix))]\n#[cfg(not(unix))]\nmod tests {\n}\n", [6]],
  ["stacked cfgs that hold without test", "#[cfg(any(test, unix))]\n#[cfg(unix)]\nmod platform {\n}\n", []],
  ["nested inside another module", "mod inner {\n    #[cfg(test)]\n    mod tests {\n    }\n}\n", [6]],
  ["inner test-only attribute", "#![cfg(test)]\nfn helper() {}\n", [4]],
  ["predicate that holds without test", "#[cfg(any(windows, test))]\nmod platform {\n}\n", []],
  ["negated test", "#[cfg(not(test))]\nmod real_clock {\n}\n", []],
  ["option value containing test", "#[cfg(feature = \"test-utils\")]\nmod fixtures {\n}\n", []],
  ["inner attribute that is not test-only", "#![cfg(feature = \"x\")]\nfn helper() {}\n", []],
  // `cfg_attr`, or `cfg` under a qualified path, is not evaluated, so a module carrying one is
  // reported; the same attributes on other items are not read at all.
  ["cfg_attr on a module", "#[cfg_attr(test, derive(Debug))]\nmod model {\n}\n", [5]],
  ["cfg_attr adding #[path]", "#[cfg(test)]\n#[cfg_attr(test, path = \"x.rs\")]\nmod tests;\n", [6]],
  ["cfg_attr on another item", "#[cfg_attr(test, derive(Debug))]\nstruct Model;\n", []],
  ["path-qualified cfg", "#[core::prelude::v1::cfg(test)]\nmod tests {\n}\n", [5]],
  ["path-qualified cfg with a leading `::`", "#[::core::prelude::v1::cfg(test)]\nmod tests;\n", [5]],
  ["path-qualified attribute other than cfg", "#[rustfmt::skip]\nmod table {\n}\n", []],
  ["inner cfg_attr", "#![cfg_attr(test, cfg(test))]\nfn helper() {}\n", [4]],
  ["unparsable cfg on another item", "macro_rules! m {\n    ($c:meta) => {\n        #[cfg($c)]\n        fn helper() {}\n    };\n}\n", []],
  ["too many options on another item", `#[cfg(all(test, ${Array.from({ length: 17 }, (_, n) => `o${n}`).join(", ")}))]\nfn helper() {}\n`, []],
  ["test-only module named by a macro", "macro_rules! m {\n    ($n:ident) => {\n        #[cfg(test)]\n        mod $n {\n        }\n    };\n}\n", [7]],
  ["module named by a macro, not test-only", "macro_rules! m {\n    ($n:ident) => {\n        mod $n {\n        }\n    };\n}\n", []],
  ["inner test-only cfg in a nested module", "mod inner {\n    #![cfg(test)]\n}\n", [5]],
  // The nested module is part of the outer one's report.
  ["test module nested in a reported one", "#[cfg(test)]\nmod outer {\n    #[cfg(test)]\n    mod tests {\n    }\n}\n", [5]],
  // A `tests` module's file is measured as test code, so it must be test-only.
  ["`mod tests;` without cfg", "mod tests;\n", [4]],
  ["inline `pub mod tests` without cfg", "pub mod tests {\n}\n", [4]],
  ["`mod tests;` under a cfg that holds without test", "#[cfg(unix)]\nmod tests;\n", [5]],
  ["test-only helper function", "#[cfg(test)]\nfn helper() {}\n", []],
  ["`#` that opens no attribute", "macro_rules! m {\n    (#$x:ident) => {};\n}\n#[cfg(test)]\nmod tests {\n}\n", [8]],
  ["attribute text inside a comment", "/*\n#[cfg(test)]\nmod tests {\n*/\nfn a() {}\n", []],
  ["attribute text inside a raw string", "const S: &str = r#\"\n#[cfg(test)]\nmod tests {\n\"#;\n", []],
];
for (const [name, tail, expected] of LAYOUTS) {
  /** Checks one placement is reported, or not, at the module's `mod` line. */
  test(`layout: ${name}`, () => {
    assert.deepEqual(reported(tail), expected);
  });
}

/** Checks every offending module in a file is reported, each with its own problem. */
test("several offending modules are all reported with their problems", () => {
  const found = misplacedTestModules(`${PRODUCTION}#[cfg(test)]\nmod tests {\n}\n\n#[cfg(test)]\nmod support;\n`);
  assert.deepEqual(found.map(({ line }) => line), [5, 9]);
  assert.match(found[0].problem, /inline test module `tests`/u);
  assert.match(found[1].problem, /out-of-line test module `support`/u);
});

/** Checks each unmodelled or misplaced shape names its own problem. */
test("problems name the shape", () => {
  /** Returns the problems reported for `tail` after the production prelude. */
  const problem = (tail) => misplacedTestModules(PRODUCTION + tail).map(({ problem }) => problem);
  assert.match(problem("#![cfg(test)]\n")[0], /whole production file/u);
  assert.match(problem("mod inner {\n    #![cfg(test)]\n}\n")[0], /enclosing item/u);
  assert.match(problem("#![cfg_attr(test, cfg(test))]\n")[0], /inner `cfg_attr` attribute is not modelled/u);
  assert.match(problem("#[cfg_attr(test, path = \"x.rs\")]\nmod tests;\n")[0], /`cfg_attr` on module `tests` is not modelled/u);
  assert.match(problem("#[core::cfg(test)]\nmod tests;\n")[0], /`cfg` on module `tests` is not modelled/u);
  assert.match(problem("macro_rules! m {\n    ($n:ident) => { #[cfg(test)] mod $n {} };\n}\n")[0], /name comes from a macro/u);
  assert.match(problem("mod tests;\n")[0], /module `tests` is not test-only/u);
});

/** Checks a predicate at the option limit is still decided rather than rejected. */
test("sixteen options are still decided", () => {
  const options = Array.from({ length: 16 }, (_, n) => `o${n}`).join(", ");
  assert.deepEqual(reported(`#[cfg(all(test, ${options}))]\nmod tests {\n}\n`), [5]);
});

/** Checks CRLF checkouts report the same lines as LF. */
test("CRLF line endings report the same lines", () => {
  const source = `${PRODUCTION}#[cfg(test)]\n// helpers\nmod tests {\n}\n`;
  assert.deepEqual(misplacedTestModules(source.replaceAll("\n", "\r\n")), misplacedTestModules(source));
});

// Each row is a predicate inside `cfg(...)` as tokens, with string values already blanked.
const PREDICATES = [
  ["test", "F"],
  ["all ( test , unix )", "F"],
  ["all ( feature = , test , )", "F"],
  ["any ( test , all ( test , unix ) )", "F"],
  ["not ( not ( test ) )", "F"],
  ["any ( )", "F"],
  ["test ,", "F"],
  ["all ( )", "T"],
  ["not ( test )", "T"],
  ["any ( windows , test )", "U"],
  ["unix", "U"],
  ["feature =", "U"],
  ["all ( not ( test ) , unix )", "U"],
  ["all ( any ( test , unix ) , not ( unix ) )", "F"],
  ["any ( unix , not ( unix ) )", "T"],
  // Blanked values cannot be compared, so two `key =` options are independent.
  ["all ( feature = , not ( feature = ) )", "U"],
];
for (const [predicate, value] of PREDICATES) {
  /** Checks one predicate's value with `test` off and every other option unknown. */
  test(`cfg predicate: ${predicate}`, () => {
    assert.equal(cfgValueWithoutTest(predicate.split(" ")), value);
  });
}

// Shapes outside the cfg grammar. Guessing would either hide a test module or reject production
// code, so each throws with the attribute's line.
const REJECTED = [
  ["unknown operator", "#[cfg(either(test))]\nmod tests {\n}\n", /line 4: unparsable cfg predicate/u],
  ["text after the predicate", "#[cfg(test unix)]\nmod tests {\n}\n", /line 4: unparsable cfg predicate/u],
  ["not with two operands", "#[cfg(not(test, unix))]\nmod tests {\n}\n", /line 4: unparsable cfg predicate/u],
  ["number as an option", "#[cfg(1)]\nmod tests {\n}\n", /line 4: unparsable cfg predicate/u],
  ["cfg without a predicate", "#[cfg]\nmod tests {\n}\n", /line 4: `cfg` without a predicate/u],
  ["unclosed attribute", "#[cfg(test)\nmod tests {\n}\n", /line 4: unclosed `\[`/u],
  // One option past the limit; the 16-option control below must still be decided.
  ["too many options", `#[cfg(all(test, ${Array.from({ length: 17 }, (_, n) => `o${n}`).join(", ")}))]\nmod tests {\n}\n`, /line 4: cfg attributes name more than 16 options/u],
];
for (const [name, tail, message] of REJECTED) {
  /** Checks an unparsable cfg throws instead of being classified. */
  test(`fails closed: ${name}`, () => {
    assert.throws(() => misplacedTestModules(PRODUCTION + tail), message);
  });
}
