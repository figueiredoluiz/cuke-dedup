import assert from "node:assert/strict";
import { test } from "node:test";
import { coverage, touchedLines } from "./docstrings.mjs";

/** Build a diff hunk with an explicit destination range. */
function patch(path, start = 1, count = 100) {
  return `diff --git a/${path} b/${path}\n--- a/${path}\n+++ b/${path}\n@@ -1,1 +${start},${count} @@\n`;
}

/** Require documentation for each supported declaration and callback form. */
test("documented and undocumented function matrix", () => {
  const cases = [
    ["rust function", "a.rs", "fn run() {}", "/// Run the operation.\n"],
    ["rust test", "a.rs", "#[test]\nfn run() {}", "/// Pin the operation.\n"],
    ["JS function", "a.mjs", "export async function run() {}", "/** Run the operation. */\n"],
    ["JS arrow", "a.cjs", "const run = () => {};", "/** Run the operation. */\n"],
    ["TS arrow", "a.ts", "const run = async () => {};", "/** Run the operation. */\n"],
    ["TS variable annotation", "a.ts", "const run: Handler = () => {};", "/** Run the operation. */\n"],
    ["TS return annotation", "a.ts", "const run = (): Result => {};", "/** Run the operation. */\n"],
    ["TS function annotation", "a.ts", "const run: (value: string) => Result = (value) => value;", "/** Run the operation. */\n"],
    ["TS generic async arrow", "a.ts", "export const run = async <T>(value: T): Promise<T> => value;", "/** Run the operation. */\n"],
    ["parenthesized arrow", "a.mjs", "const run = (() => {});", "/** Run the operation. */\n"],
    ["TS satisfies arrow", "a.ts", "const run = (() => {}) satisfies Handler;", "/** Run the operation. */\n"],
    ["TS asserted arrow", "a.ts", "const run = (() => {}) as Handler;", "/** Run the operation. */\n"],
    ["generator value", "a.mjs", "const run = function* () {};", "/** Run the operation. */\n"],
    ["test callback", "a.mjs", 'test("operation", () => {});', "/** Pin the operation. */\n"],
    ["suite callback", "a.mjs", 'describe("operation", () => {});', "/** Group operation controls. */\n"],
  ];
  for (const [name, path, source, doc] of cases) {
    for (const documented of [false, true]) {
      const result = coverage(patch(path), () => (documented ? doc : "") + source);
      assert.equal(result.functions.length, 1, name);
      assert.equal(result.passed, documented, name);
    }
  }
});

/** Use Python's AST to distinguish docstrings from ordinary comments and include async methods. */
test("Python literal documentation", () => {
  for (const declaration of ["def run():", "async def run():"]) {
    for (const documented of [false, true]) {
      const source = declaration + (documented ? '\n    """Run the operation."""' : "\n    # Run the operation.") + "\n    pass\n";
      const result = coverage(patch("a.py"), () => source);
      assert.equal(result.functions.length, 1);
      assert.equal(result.passed, documented);
    }
  }
});

/** Keep unrelated functions outside the supplied context range and include deletion boundaries. */
test("diff scope and deletion-only changes", () => {
  const source = "/// Documented.\nfn first() {}\n\n\n\n\nfn second() {}\n";
  assert.equal(coverage(patch("a.rs", 1, 3), () => source).passed, true);
  assert.equal(coverage(patch("a.rs", 7, 0), () => source).passed, false);
  assert.equal(coverage(patch("a.rs", 1, 100), () => source).passed, false);
  assert.equal(coverage("", () => assert.fail("must not read")).passed, true);
  const deleted = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n";
  assert.equal(coverage(deleted, () => assert.fail("must not read deleted file")).passed, true);
});

/** Removed documentation must fail on the surviving function, and plain comments do not qualify. */
test("documentation removal and non-doc comments", () => {
  for (const source of ["fn run() {}", "// Run.\nfn run() {}", "/* Run. */\nfn run() {}", "/** */\nfn run() {}", "/** Earlier. */\nfn first() {}\n/* Plain. */\nfn run() {}"]) {
    assert.equal(coverage(patch("a.rs", 1, 4), () => source).passed, false);
  }
});

/** Reject ambiguous paths and malformed hunks instead of silently validating a partial patch. */
test("invalid diff input", () => {
  assert.throws(() => touchedLines(patch('"quoted.rs"')), /Unsupported/);
  assert.throws(() => touchedLines(patch("../a.rs")), /Unsupported/);
  assert.throws(() => touchedLines(patch("a.rs").replace("@@ -1,1 +1,100 @@", "@@ broken @@")), /Malformed/);
  assert.throws(() => coverage(patch("a.py"), () => "def broken(:"));
  assert.throws(() => coverage(patch("a.mjs"), () => "function broken( {"), /Cannot parse/);
  for (const source of ["fn broken() {", "fn broken() { /*", 'fn broken() { let value = r#"oops']) {
    assert.throws(() => coverage(patch("a.rs"), () => source));
  }
});

/** Added lines resembling headers must retain file identity across subsequent hunks and files. */
test("hunk content cannot replace a file header", () => {
  const diff = patch("a.mjs", 1, 1) + "+++ count;\n@@ -20,1 +20,1 @@\n-function missing() {}\n+function missing() { return 1; }\n" + patch("b.rs", 1, 2);
  const result = coverage(diff, path => path === "a.mjs" ? "\n".repeat(19) + "function missing() {}" : "/// Existing.\nfn existing() {}");
  assert.deepEqual([...touchedLines(diff).keys()], ["a.mjs", "b.rs"]);
  assert.equal(result.passed, false);
  assert.deepEqual(result.functions.map(fn => fn.name), ["missing", "existing"]);
});

/** URL strings must not pull an unrelated function into a later change's documentation scope. */
test("quoted comment markers preserve body boundaries", () => {
  for (const quote of ['"', "'", "`"] ) {
    const source = `function unrelated() { return ${quote}https://example.com${quote}; }\n` + "\n".repeat(10) + "/** Target. */\nfunction target() {}";
    const result = coverage(patch("a.mjs", 12, 2), () => source);
    assert.equal(result.passed, true, quote);
    assert.deepEqual(result.functions.map(fn => fn.name), ["target"]);
  }
  const rust = 'fn unrelated() { let url = "https://example.com"; }\n' + "\n".repeat(10) + "/// Target.\nfn target() {}";
  assert.equal(coverage(patch("a.rs", 12, 2), () => rust).passed, true);
});

/** A callback passed into a call does not make its result variable a named function. */
test("arrow call arguments are not arrow declarations", () => {
  for (const source of ["const value = items.map(item => item);", "const value: Result = call(() => {});"]) {
    assert.equal(coverage(patch("a.ts"), () => source).functions.length, 0);
  }
});

/** Lexical braces must not hide later edits inside an undocumented function. */
test("literal and comment braces cannot truncate a function", () => {
  for (const text of ["const pattern = /}/;", "/* } */", "/* first\n } \n*/", 'const value = `}`;']) {
    const source = "function missing() {\n" + text + "\n\n\n\n\nreturn 1;\n}";
    const result = coverage(patch("a.mjs", source.split("\n").length - 1, 1), () => source);
    assert.equal(result.passed, false, text);
    assert.equal(result.functions.length, 1, text);
  }
});

/** Expression-bodied arrows end at their expression rather than a later declaration. */
test("semicolon-free arrows preserve later declaration boundaries", () => {
  for (const expression of ["1", "(\n  1\n)", "({ value: 1 })"]) {
    const source = "const unrelated = () => " + expression + "\n" + "\n".repeat(10) + "/** Target. */\nfunction target() {}";
    const result = coverage(patch("a.mjs", source.split("\n").length - 1, 2), () => source);
    assert.equal(result.passed, true, expression);
    assert.deepEqual(result.functions.map(fn => fn.name), ["target"]);
  }
});

/** Rust raw strings and nested comments must preserve function ranges and hide fake declarations. */
test("Rust lexical constructs cannot truncate or invent functions", () => {
  for (const text of ['let value = r#"}\nfn fake() {}\n"#;', "/* outer /* inner */ }\nfn fake() {}\n*/", "let value = '}';"]) {
    const source = "fn missing() {\n" + text + "\n\n\n\nreturn;\n}";
    const result = coverage(patch("a.rs", source.split("\n").length - 1, 1), () => source);
    assert.equal(result.passed, false, text);
    assert.deepEqual(result.functions.map(fn => fn.name), ["missing"]);
  }
});

/** Rust signature groups cannot hide later body edits or consume unrelated declarations. */
test("Rust signature delimiters preserve complete body ranges", () => {
  const signatures = [
    "fn run(buf: [u8; 4])",
    "fn run<const N: usize = {4}>(buf: [u8; N])",
    "fn run<T: Fn() -> [u8; 4]>(call: T) -> [u8; 4]",
    "fn run() -> value_type!{u8}",
    'pub unsafe extern "C" fn run()',
  ];
  for (const signature of signatures) for (const documented of [false, true]) {
    const source = (documented ? "/// Run the operation.\n" : "") + signature + "\n{\n\n\n\nreturn;\n}";
    const result = coverage(patch("a.rs", source.split("\n").length - 1, 1), () => source);
    assert.equal(result.functions.length, 1, signature);
    assert.equal(result.passed, documented, signature);
  }
  const source = "fn unrelated(buf: [u8; 4]);\n" + "\n".repeat(10) + "/// Target.\nfn target() {}";
  assert.deepEqual(coverage(patch("a.rs", 12, 2), () => source).functions.map(fn => fn.name), ["target"]);
});

/** A for-loop initializer's documentation belongs to the loop rather than the source-file root. */
test("loop callable ownership stays local", () => {
  const source = "function unrelated() {}\n" + "\n".repeat(10) + "/** Local callable. */\nfor (let run = () => {}; false;) {}";
  const result = coverage(patch("a.mjs", 12, 2), () => source);
  assert.equal(result.passed, true);
  assert.deepEqual(result.functions.map(fn => fn.name), ["run"]);
});
