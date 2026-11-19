/**
 * Inclusive code-point ranges the JSON reporter drops from report text (`sanitize` in
 * src/reporters/shared.rs); a test pins this list to that source.
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

/**
 * Decodes a percent-encoded SARIF `artifactLocation.uri` to the exact file-name text.
 * Decodes bytes, then UTF-8 with U+FFFD substitution, so non-UTF-8 names read like the JSON
 * reporter's lossy path instead of throwing as `decodeURIComponent` does. A leading U+FEFF is
 * kept: it belongs to the file name.
 * @param {string} uri Root-relative URI from the SARIF reporter.
 * @returns {string} The decoded path.
 */
export function decodeSarifUri(uri) {
  const bytes = [];
  for (let index = 0; index < uri.length; index += 1) {
    if (uri[index] === "%" && /^[0-9A-Fa-f]{2}$/.test(uri.slice(index + 1, index + 3))) {
      bytes.push(Number.parseInt(uri.slice(index + 1, index + 3), 16));
      index += 2;
    } else {
      bytes.push(...new TextEncoder().encode(uri[index]));
    }
  }
  return new TextDecoder("utf-8", { ignoreBOM: true }).decode(Uint8Array.from(bytes));
}

/**
 * Projects a SARIF URI onto the JSON reporter's display path for parity checks: `\` reads as
 * `/`, controls other than tab and line breaks and `REPORT_UNSAFE_RANGES` are dropped. The URI
 * itself stays exact; only the comparison is normalized.
 * @param {string} uri Root-relative URI from the SARIF reporter.
 * @returns {string} The path as the JSON reporter displays it.
 */
export function sarifDisplayPath(uri) {
  return [...decodeSarifUri(uri).replaceAll("\\", "/")]
    .filter((character) => {
      const point = character.codePointAt(0);
      const control = /\p{Cc}/u.test(character) && !"\t\n\r".includes(character);
      return !control && !REPORT_UNSAFE_RANGES.some(([start, end]) => point >= start && point <= end);
    })
    .join("");
}
