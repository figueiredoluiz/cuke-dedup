import { test } from "node:test";
import assert from "node:assert/strict";
import {
  countMatches,
  expectedTotal,
  findingMatches,
  findingOwners,
  locationLabel,
} from "./recall-oracle.mjs";

/** Builds an overridable duplicate-matcher finding for ownership and recall tests. */
const finding = (over = {}) => ({
  rule: "duplicate-matcher",
  primary: { path: "steps.ts", line: 1 },
  related: [],
  message: "duplicate matcher",
  evidence: {},
  ...over,
});

/** Checks each explicit expectation field narrows a bare rule match. */
test("a bare rule expectation matches; each named field narrows it", () => {
  const f = finding({ primary: { path: "steps.ts", line: 7 } });
  assert.equal(findingMatches(f, { rule: "duplicate-matcher" }), true);
  assert.equal(findingMatches(f, { rule: "unused-definition" }), false);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", primaryLine: 7 }), true);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", primaryLine: 8 }), false);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", primaryPath: "other.ts" }), false);
});

/** Checks related locations match as an exact unordered set with the required count. */
test("related locations compare unordered and exactly, so a subset does not satisfy", () => {
  const f = finding({ related: [{ path: "a.ts", line: 2 }, { path: "b.ts", line: 3 }] });
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", relatedLocations: ["b.ts:3", "a.ts:2"] }), true);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", relatedLocations: ["a.ts:2"] }), false);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", relatedCount: 2 }), true);
  assert.equal(findingMatches(f, { rule: "duplicate-matcher", relatedCount: 1 }), false);
});

/** Checks per-expectation counts cannot hide a doubly owned finding and an unclassified finding. */
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

/** Checks disjoint expectations assign exactly one owner to each finding. */
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

/** Checks default and explicit expected counts sum correctly and locations format as path:line. */
test("count defaults to 1 and sums; locationLabel renders path:line", () => {
  assert.equal(expectedTotal([{ rule: "r" }, { rule: "r", count: 3 }]), 4);
  assert.equal(locationLabel({ path: "steps.ts", line: 12 }), "steps.ts:12");
});
