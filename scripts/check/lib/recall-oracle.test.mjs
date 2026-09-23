import { test } from "node:test";
import assert from "node:assert/strict";
import {
  countMatches,
  expectedTotal,
  findingMatches,
  findingOwners,
  locationLabel,
} from "./recall-oracle.mjs";

const finding = (over = {}) => ({
  rule: "duplicate-matcher",
  primary: { path: "steps.ts", line: 1 },
  related: [],
  message: "duplicate matcher",
  evidence: {},
  ...over,
});

test("a bare rule expectation matches; each named field narrows it", () => {
  const f = finding({ primary: { path: "steps.ts", line: 7 } });
  assert.equal(findingMatches(f, { rule: "duplicate-matcher" }), true);
  assert.equal(findingMatches(f, { rule: "unused-definition" }), false);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", primaryLine: 7 }), true);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", primaryLine: 8 }), false);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", primaryPath: "other.ts" }), false);
});

test("related locations compare unordered and exactly, so a subset does not satisfy", () => {
  const f = finding({ related: [{ path: "a.ts", line: 2 }, { path: "b.ts", line: 3 }] });
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", relatedLocations: ["b.ts:3", "a.ts:2"] }), true);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", relatedLocations: ["a.ts:2"] }), false);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", relatedCount: 2 }), true);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", relatedCount: 1 }), false);
});

test("PR #39 oracle: per-expectation counts balance yet a finding is double-owned and one unclassified", () => {
  // X satisfies a broad and a narrow expectation at once; Y satisfies neither. Counting each
  // expectation independently passes (E1→{X}=1, E2→{X}=1, total findings 2 = expectedTotal 2), which
  // is exactly the shipped bug. Requiring exactly one owner per finding is what catches it.
  const x = finding({ primary: { path: "a.ts", line: 1 } });
  const y = finding({ primary: { path: "b.ts", line: 9 } });
  const broad = { rule: "duplicate-matcher", primaryPath: "a.ts" };
  const narrow = { rule: "duplicate-matcher", primaryPath: "a.ts", primaryLine: 1 };
  const expected = [broad, narrow];

  // Independent per-expectation counts each look correct.
  assert.equal(countMatches([x, y], broad), 1);
  assert.equal(countMatches([x, y], narrow), 1);
  assert.equal(expectedTotal(expected), 2);

  // The one-to-one oracle exposes the truth: X is owned twice, Y not at all.
  assert.equal(findingOwners(x, expected).length, 2);
  assert.equal(findingOwners(y, expected).length, 0);
});

test("a well-formed case gives every finding exactly one owner", () => {
  const x = finding({ primary: { path: "a.ts", line: 1 } });
  const y = finding({ primary: { path: "b.ts", line: 2 } });
  const expected = [
    { rule: "duplicate-matcher", primaryPath: "a.ts" },
    { rule: "duplicate-matcher", primaryPath: "b.ts" },
  ];
  assert.equal(findingOwners(x, expected).length, 1);
  assert.equal(findingOwners(y, expected).length, 1);
});

test("count defaults to 1 and sums; locationLabel renders path:line", () => {
  assert.equal(expectedTotal([{ rule: "r" }, { rule: "r", count: 3 }]), 4);
  assert.equal(locationLabel({ path: "steps.ts", line: 12 }), "steps.ts:12");
});
