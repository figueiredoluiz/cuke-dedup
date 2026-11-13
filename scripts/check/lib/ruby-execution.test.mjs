import assert from "node:assert/strict";
import test from "node:test";
import { problems, surplus } from "./ruby-execution.mjs";

/** A `steps.rb` Cucumber-expression registration, with optional overrides. */
const registration = (matcher, line = 1, extra = {}) =>
  ({ path: "steps.rb", line, matcherKind: "cucumber-expression", matcher, matcherFlags: "", ...extra });

/** Identity ignores locations but counts duplicates and keeps kind and flags apart. */
test("surplus compares matcher multisets", () => {
  assert.deepEqual(surplus([registration("a", 1)], [registration("a", 9)]), []);
  assert.deepEqual(surplus([registration("a"), registration("a", 2)], [registration("a")]), ['cucumber-expression "a" at steps.rb:2']);
  assert.deepEqual(surplus([registration("a", 1, { matcherKind: "regular-expression" })], [registration("a")]),
    ['regular-expression "a" at steps.rb:1']);
  assert.deepEqual(surplus([registration("a", 1, { matcherKind: "regular-expression", matcherFlags: "i" })],
    [registration("a", 1, { matcherKind: "regular-expression" })]), ['regular-expression "a"/i at steps.rb:1']);
});

/** Each kind of disagreement is reported; agreement reports nothing. */
test("problems name every disagreement", () => {
  const ruby = { registrations: [registration("a")], loadErrors: [], replaced: [] };
  assert.deepEqual(problems({ registrations: [registration("a")] }, ruby), []);
  assert.deepEqual(problems({ registrations: [registration("b")] }, {
    registrations: [registration("a")],
    loadErrors: [{ path: "steps.rb", line: 2, error: "RuntimeError", message: "stop" }],
    replaced: ["Then"],
  }), [
    'extracted but not registered by Ruby: cucumber-expression "b" at steps.rb:1',
    'registered by Ruby but not extracted: cucumber-expression "a" at steps.rb:1',
    "steps.rb:2 fails to load: RuntimeError: stop",
    "step keyword replaced at load time: Then",
  ]);
});
