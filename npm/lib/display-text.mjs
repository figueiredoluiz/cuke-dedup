/**
 * Inclusive invisible-format code-point ranges (`is_invisible_format` in src/display_text.rs):
 * reports drop them, diagnostics escape them. A test pins this list to that source.
 */
export const REPORT_UNSAFE_RANGES = [
  [0xad, 0xad],
  [0x61c, 0x61c],
  [0x200b, 0x200f],
  [0x202a, 0x202e],
  [0x2060, 0x2069],
  [0xfeff, 0xfeff],
  [0xe0000, 0xe007f],
];

const NAMED_ESCAPES = { "\n": "\\n", "\r": "\\r", "\t": "\\t" };

/**
 * Whether a code point must not appear raw on a diagnostic line: controls, lone surrogates,
 * line and paragraph separators, and invisible-format characters.
 * @param {number} point Code point, or a lone surrogate's code unit.
 * @returns {boolean} True when the character is escaped.
 */
function isDiagnosticUnsafe(point) {
  return point <= 0x1f || (point >= 0x7f && point <= 0x9f) || (point >= 0xd800 && point <= 0xdfff) ||
    point === 0x2028 || point === 0x2029 ||
    REPORT_UNSAFE_RANGES.some(([start, end]) => point >= start && point <= end);
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
