import { Buffer } from "node:buffer";

/** Unicode 17 default-ignorable ranges, pinned to `is_invisible_format` in Rust. */
export const REPORT_UNSAFE_RANGES = [
  [0xad, 0xad],
  [0x034f, 0x034f],
  [0x61c, 0x61c],
  [0x115f, 0x1160],
  [0x17b4, 0x17b5],
  [0x180b, 0x180f],
  [0x200b, 0x200f],
  [0x202a, 0x202e],
  [0x2060, 0x206f],
  [0x3164, 0x3164],
  [0xfe00, 0xfe0f],
  [0xfeff, 0xfeff],
  [0xffa0, 0xffa0],
  [0xfff0, 0xfff8],
  [0x1bca0, 0x1bca3],
  [0x1d173, 0x1d17a],
  [0xe0000, 0xe0fff],
];

const NAMED_ESCAPES = { "\n": "\\n", "\r": "\\r", "\t": "\\t" };

/** Returns whether the pinned Unicode range list includes `point`. */
function isInvisibleFormat(point) {
  return REPORT_UNSAFE_RANGES.some(([start, end]) => point >= start && point <= end);
}

/**
 * Whether a code point must not appear raw on a diagnostic line: controls, lone surrogates,
 * line and paragraph separators, and invisible-format characters.
 * @param {number} point Code point, or a lone surrogate's code unit.
 * @returns {boolean} True when the character is escaped.
 */
function isDiagnosticUnsafe(point) {
  return point <= 0x1f || (point >= 0x7f && point <= 0x9f) || (point >= 0xd800 && point <= 0xdfff) ||
    point === 0x2028 || point === 0x2029 ||
    isInvisibleFormat(point);
}

/**
 * Escapes unsafe characters like the native CLI so repository-derived text stays visible on one
 * line: `\n`, `\r` and `\t` by name, others as `\u{hex}`. Backslashes are not escaped.
 * @param {unknown} value Text to show; non-strings are converted with `String`.
 * @returns {string} The escaped text.
 */
export function diagnosticSafe(value) {
  let escaped = "";
  for (const character of String(value)) {
    const point = character.codePointAt(0);
    escaped += NAMED_ESCAPES[character] ??
      (isDiagnosticUnsafe(point) ? `\\u{${point.toString(16)}}` : character);
  }
  return escaped;
}

/**
 * Projects structured report text using the Rust renderer's bound and escape spelling.
 * Complete lowercase Unicode escape spellings stay together when shortening the display text.
 * @param {string} value Original text or already escaped display text.
 * @returns {string} The bounded display text.
 */
export function reportSafe(value) {
  const limit = 8 * 1024;
  const markerBoundary = limit - 64;
  let rendered = "";
  let sourceBytes = 0;
  let outputBytes = 0;
  let retainedSource = 0;
  let retainedOutput = 0;
  for (const [unit] of value.matchAll(/\\u\{[0-9a-f]{1,6}\}|./gsu)) {
    const point = unit.codePointAt(0);
    let next = unit;
    if (isInvisibleFormat(point)) {
      next = `\\u{${point.toString(16)}}`;
    } else if (/\p{Cc}/u.test(unit) && !"\t\n\r".includes(unit)) {
      next = "";
    }
    sourceBytes += Buffer.byteLength(unit);
    outputBytes += Buffer.byteLength(next);
    if (sourceBytes > limit || outputBytes > limit) {
      return `${rendered.slice(0, retainedOutput)}… [${Buffer.byteLength(value) - retainedSource} bytes omitted]`;
    }
    rendered += next;
    if (sourceBytes <= markerBoundary && outputBytes <= markerBoundary) {
      retainedSource = sourceBytes;
      retainedOutput = rendered.length;
    }
  }
  return rendered;
}
