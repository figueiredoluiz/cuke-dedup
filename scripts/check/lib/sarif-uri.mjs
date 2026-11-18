/**
 * Decodes a percent-encoded SARIF `artifactLocation.uri` to the analyzer's display path.
 * Decodes bytes, then UTF-8 with U+FFFD substitution, so non-UTF-8 names read like the JSON
 * reporter's lossy path instead of throwing as `decodeURIComponent` does.
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
  return new TextDecoder().decode(Uint8Array.from(bytes));
}
