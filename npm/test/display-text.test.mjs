import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { diagnosticSafe, REPORT_UNSAFE_RANGES } from "../lib/display-text.mjs";

/** Unsafe code points from Unicode Cc, Zl/Zp, surrogates and the report ranges, as raw intervals. */
function oracleUnsafe(point) {
  return [
    [0x00, 0x1f], [0x7f, 0x9f], [0xad, 0xad], [0x61c, 0x61c], [0x200b, 0x200f],
    [0x2028, 0x202e], [0x2060, 0x2069], [0xd800, 0xdfff], [0xfeff, 0xfeff], [0xe0000, 0xe007f],
  ].some(([start, end]) => point >= start && point <= end);
}

/** Builds the expected escape text by hand, one code point at a time. */
function oracleEscape(value) {
  const named = { "\n": "\\n", "\r": "\\r", "\t": "\\t" };
  return [...value].map((character) => {
    const point = character.codePointAt(0);
    if (named[character]) return named[character];
    return oracleUnsafe(point) ? `\\u{${point.toString(16)}}` : character;
  }).join("");
}

/** Shared cases also checked by the Rust `shared_escape_cases_match` test. */
test("diagnosticSafe matches the shared escape cases", () => {
  const cases = JSON.parse(readFileSync(new URL("../../fixtures/diagnostics/escapes.json", import.meta.url), "utf8"));
  assert.ok(cases.length >= 20);
  for (const { name, input, expected } of cases) assert.equal(diagnosticSafe(input), expected, name);
});

/** JavaScript strings can hold lone surrogates, which Rust text cannot; they are escaped too. */
test("diagnosticSafe escapes lone surrogates and keeps pairs", () => {
  assert.equal(diagnosticSafe("a\ud800b\udfffc"), "a\\u{d800}b\\u{dfff}c");
  assert.equal(diagnosticSafe("\ud83d\ude00"), "\u{1f600}");
  assert.equal(diagnosticSafe(String(42)), "42");
});

/** Seeded strings from boundary code points match the hand-built escape and keep one line. */
test("diagnosticSafe matches the escape oracle for generated text", () => {
  const boundaries = [
    0x0, 0x9, 0xa, 0xd, 0x1b, 0x1f, 0x20, 0x7e, 0x7f, 0x80, 0x85, 0x9f, 0xa0, 0xac, 0xad, 0xae,
    0x61b, 0x61c, 0x61d, 0x200a, 0x200b, 0x200f, 0x2010, 0x2027, 0x2028, 0x2029, 0x202a, 0x202e,
    0x202f, 0x205f, 0x2060, 0x2069, 0x206a, 0xd7ff, 0xd800, 0xdbff, 0xdc00, 0xdfff, 0xe000,
    0xfefe, 0xfeff, 0xff00, 0xdffff, 0xe0000, 0xe007f, 0xe0080, 0x5c, 0x3a, 0x23,
  ];
  let state = 42;
  /** Xorshift32 step: deterministic input without a dependency, with uncorrelated low bits. */
  const next = () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return state >>> 0;
  };
  let changed = 0;
  for (let run = 0; run < 256; run += 1) {
    const units = [];
    for (let index = next() % 24; index > 0; index -= 1) {
      const point = next() % 4 === 0 ? next() % 0x110000 : boundaries[next() % boundaries.length];
      // Code units, not code points: a lone surrogate must stay lone.
      units.push(point >= 0xd800 && point <= 0xdfff ? String.fromCharCode(point) : String.fromCodePoint(point));
    }
    const input = units.join("");
    const actual = diagnosticSafe(input);
    assert.equal(actual, oracleEscape(input), JSON.stringify(input));
    assert.ok(![...actual].some((character) => oracleUnsafe(character.codePointAt(0))));
    if (actual !== input) changed += 1;
  }
  assert.ok(changed >= 64 && changed <= 248, `changed stratum: ${changed}`);
});

/** Pins the range list to the Rust `is_invisible_format` predicate so the two cannot drift apart. */
test("REPORT_UNSAFE_RANGES matches the Rust invisible-format predicate", () => {
  const source = readFileSync(new URL("../../src/display_text.rs", import.meta.url), "utf8");
  const start = source.indexOf("fn is_invisible_format(");
  const body = source.slice(start, source.indexOf("\n}\n", start));
  const ranges = [...body.matchAll(/'\\u\{([0-9a-f]+)\}'(?:\.\.='\\u\{([0-9a-f]+)\}')?/g)]
    .map(([, first, last]) => [Number.parseInt(first, 16), Number.parseInt(last ?? first, 16)]);
  assert.ok(start >= 0 && ranges.length > 0, "no ranges parsed from is_invisible_format");
  assert.deepEqual(REPORT_UNSAFE_RANGES, ranges);
});
