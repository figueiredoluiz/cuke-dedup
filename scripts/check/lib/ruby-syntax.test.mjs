import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import test from "node:test";
import { capturedEntries, judge, parseRejections } from "./ruby-syntax.mjs";

/** SHA-256 of `text`, as the rejection list keys it. */
const hash = (text) => createHash("sha256").update(Buffer.from(text)).digest("hex");
/** A captured file entry with `text` as its bytes. */
const file = (path, text) => ({ path, bytes: Buffer.from(text) });

/** Each row pairs an expectation with one Ruby outcome; the last column names the failure, or null for a pass. */
test("judge applies each expectation to each Ruby outcome", () => {
  const rows = [
    // [expected, result, failure substring or null for a pass]
    ["", { status: "compiled" }, null],
    ["", { status: "syntax", message: "a.rb:3: unexpected end" }, "SyntaxError: a.rb:3: unexpected end"],
    ["empty range", { status: "syntax", message: "a.rb:3: empty range in char class" }, null],
    ["empty range", { status: "syntax", message: "a.rb:3: unexpected end" }, 'without "empty range"'],
    ["empty range", { status: "compiled" }, "compiles, but is listed"],
    ["", { status: "error", error: "Errno::EACCES", message: "denied" }, "Errno::EACCES: denied"],
    ["empty range", { status: "error", error: "EncodingError", message: "bad" }, "EncodingError: bad"],
  ];
  for (const [expected, result, failure] of rows) {
    const verdict = judge([{ path: "a.rb", expected }], [{ path: "a.rb", ...result }]);
    if (failure === null) assert.deepEqual(verdict.failures, [], JSON.stringify(result));
    else {
      assert.equal(verdict.failures.length, 1, JSON.stringify(result));
      assert.match(verdict.failures[0], new RegExp(`^a\\.rb: .*${failure.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
    }
    assert.equal(verdict.rejected, expected === "" ? 0 : 1);
  }
});

/** A partial or inconsistent Ruby report must fail, so a crashed run cannot pass. */
test("judge fails a missing, duplicated or unexpected result", () => {
  const entries = [{ path: "a.rb", expected: "" }, { path: "b.rb", expected: "" }];
  assert.deepEqual(judge(entries, [{ path: "a.rb", status: "compiled" }]).failures, ["b.rb: no result from Ruby"]);
  assert.deepEqual(
    judge(entries, [
      { path: "a.rb", status: "compiled" },
      { path: "a.rb", status: "compiled" },
      { path: "b.rb", status: "compiled" },
      { path: "c.rb", status: "compiled" },
    ]).failures,
    ["a.rb: reported more than once", "c.rb: reported, but was not checked"],
  );
  // Control: complete results pass.
  assert.deepEqual(
    judge(entries, [{ path: "a.rb", status: "compiled" }, { path: "b.rb", status: "compiled" }]).failures,
    [],
  );
});

/** Identical captured contents collapse to one entry; a listed hash carries its message. */
test("capturedEntries keeps one path per content and attaches listed rejections", () => {
  const bad = "x = 1\n\n  y = /[b-a]/\n";
  const rejections = new Map([[hash(bad), "empty range in char class"]]);
  const { entries, stale } = capturedEntries(
    [file("c.rb", "ok = 1\n"), file("a.rb", bad), file("b.rb", "ok = 1\n")],
    rejections,
  );
  assert.deepEqual(entries, [
    { path: "a.rb", hash: hash(bad), expected: "empty range in char class" },
    { path: "b.rb", hash: hash("ok = 1\n"), expected: "" },
  ]);
  assert.deepEqual(stale, []);
});

/** A captured tracked fixture takes the fixture's message; an uncaptured fixture is not stale. */
test("capturedEntries applies tracked fixture rejections without requiring them", () => {
  const fixture = "Given(/[b-a]/) { }\n";
  const fixtures = new Map([[hash(fixture), "empty range"], [hash("absent = 1\n"), "unused"]]);
  const { entries, stale } = capturedEntries([file("a.rb", fixture), file("b.rb", "ok = 1\n")], new Map(), fixtures);
  assert.deepEqual(
    entries.map((entry) => entry.expected),
    ["empty range", ""],
  );
  assert.deepEqual(stale, []);
  // Control: without the fixture map the same capture expects the source to compile.
  assert.equal(capturedEntries([file("a.rb", fixture)], new Map()).entries[0].expected, "");
});

/** A listed source no run captured is stale; an empty capture is an error. */
test("capturedEntries reports listed sources no capture has, and an empty capture", () => {
  const gone = hash("removed = 1\n");
  const { entries, stale } = capturedEntries([file("a.rb", "ok = 1\n")], new Map([[gone, "x"]]));
  assert.deepEqual(stale, [gone]);
  assert.equal(entries[0].expected, "");
  assert.throws(() => capturedEntries([], new Map()), { message: /no captured Ruby sources/ });
});

/** The list must name a lowercase SHA-256, a message and the owning test for every entry. */
test("parseRejections accepts a complete list and names each malformed entry", () => {
  const key = hash("x");
  assert.deepEqual(
    parseRejections({ [key]: { message: "empty char-class", test: "ruby::empty_class" } }),
    new Map([[key, "empty char-class"]]),
  );
  const rows = [
    [[], /JSON object/],
    [null, /JSON object/],
    [{ ABC: { message: "m", test: "t" } }, /"ABC" is not a lowercase SHA-256/],
    [{ [key.toUpperCase()]: { message: "m", test: "t" } }, /is not a lowercase SHA-256/],
    [{ [key]: "m" }, new RegExp(`${key} must be an object`)],
    [{ [key]: { message: "", test: "t" } }, new RegExp(`${key} needs a non-empty message`)],
    [{ [key]: { message: "m" } }, new RegExp(`${key} needs the owning test name`)],
  ];
  for (const [value, message] of rows) assert.throws(() => parseRejections(value), { message });
});
