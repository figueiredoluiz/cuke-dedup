import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { PAIR_SOURCES, assertPairFile, readCensus, reportPath } from "./pair-join.mjs";
import { census } from "./pair-census-fixture.mjs";

/** Checks the pair census sums the pair-proposing sources only and refuses a census it cannot read. */
test("the pair census counts the sources that write pair records and not matcher overlap", () => {
  const sources = census({ evaluated: 2, skipped: 1 }).candidateSources;
  const classes = { truncatedStructuralClasses: 1 };
  assert.deepEqual(readCensus({ ...classes, candidateSources: { ...sources, structuralHandler: { evaluated: 3, skipped: 2 } } }), { census: { evaluated: 5, skipped: 3, limited: true } });
  // Control: matcher overlap alone changes nothing.
  assert.deepEqual(readCensus({ ...classes, candidateSources: { ...sources, matcherOverlap: { evaluated: 99, skipped: 99 } } }), { census: { evaluated: 2, skipped: 1, limited: true } });
  const { matcherBlocking, ...withoutBlocking } = sources;
  for (const unreadable of [
    undefined,
    {},
    { ...classes, candidateSources: withoutBlocking },
    { ...classes, candidateSources: { ...sources, matcherBlocking: { evaluated: 0 } } },
    { ...classes, candidateSources: { ...sources, identicalHandler: { evaluated: "2", skipped: 0 } } },
    // A source the join does not model could hold pairs or skips it would miss.
    { ...classes, candidateSources: { ...sources, futureSource: { evaluated: 0, skipped: 0 } } },
    { candidateSources: sources },
    { truncatedStructuralClasses: "1", candidateSources: sources },
  ]) {
    assert.equal(readCensus(unreadable).census, undefined);
    assert.equal(typeof readCensus(unreadable).problem, "string");
  }
});

/** Checks the pair file must match the census count exactly and exist. */
test("a pair file must exist and match the pair-source census", () => {
  const record = { schemaVersion: "1" };
  assert.deepEqual(assertPairFile([record], census({ evaluated: 1 })), { evaluated: 1, skipped: 0, limited: false });
  assert.equal(assertPairFile([], census({ skipped: 1 })).limited, true);
  assert.equal(assertPairFile([], census({ structural: 1 })).limited, true);
  assertPairFile([], census());
  assert.throws(() => assertPairFile(null, census()), /wrote no --emit-pairs file/);
  assert.throws(() => assertPairFile([], census({ evaluated: 1 })), /holds 0 records, the report's pair-source census counts 1/);
  assert.throws(() => assertPairFile([record], census()), /holds 1 records, the report's pair-source census counts 0/);
  assert.throws(() => assertPairFile([], undefined), /candidate census is missing/);
  const future = census();
  future.candidateSources.futureSource = { evaluated: 0, skipped: 0 };
  assert.throws(() => assertPairFile([], future), /names sources this check does not model: futureSource/);
});

/** Checks the source list names exactly the sources the analyzer writes pair records for. */
test("the pair sources are the analyzer's candidate sources", async () => {
  const rust = await readFile(new URL("../../../src/analysis/pairs.rs", import.meta.url), "utf8");
  const start = rust.indexOf("fn as_str(self)");
  const arms = [...rust.slice(start, rust.indexOf("\n    }\n", start)).matchAll(/Self::\w+ => "(\w+)"/g)].map((match) => match[1]);
  assert.ok(arms.length > 0, "no candidate source names found in src/analysis/pairs.rs");
  assert.deepEqual([...PAIR_SOURCES].sort(), arms.sort());
});

/** Checks `reportPath` strips exactly the characters the report sanitizer strips, read from the Rust source. */
test("report paths drop the characters the report sanitizer drops", async () => {
  const rust = await readFile(new URL("../../../src/reporters/shared.rs", import.meta.url), "utf8");
  const start = rust.indexOf("fn sanitize(");
  const body = rust.slice(start, rust.indexOf("\n}\n", start));
  const ranges = [...body.matchAll(/'\\u\{([0-9a-f]+)\}'(?:\.\.='\\u\{([0-9a-f]+)\}')?/g)]
    .map((match) => [Number.parseInt(match[1], 16), Number.parseInt(match[2] ?? match[1], 16)]);
  assert.ok(ranges.length >= 7, "no stripped ranges found in src/reporters/shared.rs");
  /** The path `a<character>b` in the report's form. */
  const around = (codePoint) => reportPath(`a${String.fromCodePoint(codePoint)}b`);
  for (const [low, high] of ranges) {
    for (const inside of [low, high, Math.floor((low + high) / 2)]) assert.equal(around(inside), "ab", `U+${inside.toString(16)}`);
    // The code points next to a range stay, unless another range covers them.
    for (const outside of [low - 1, high + 1]) {
      if (!ranges.some(([from, to]) => outside >= from && outside <= to)) assert.equal([...around(outside)].length, 3, `U+${outside.toString(16)}`);
    }
  }
  assert.equal(reportPath("a\u0001b\u0007c"), "abc");
  // Line breaks and tabs are kept, as the report keeps them.
  assert.equal(reportPath("a\nb\tc\rd"), "a\nb\tc\rd");
});
