import { reportSafe } from "../../../npm/lib/display-text.mjs";

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
 * Projects a SARIF URI onto the JSON reporter's display path for parity checks.
 * @param {string} uri Root-relative URI from the SARIF reporter.
 * @param {string} [displayPath] Original display projection supplied with a sampled URI.
 * @returns {string} The path as the JSON reporter displays it.
 */
export function sarifDisplayPath(uri, displayPath) {
  if (typeof displayPath === "string") return displayPath;
  return reportSafe(decodeSarifUri(uri).replaceAll("\\", "/"));
}
