// Decisions for check-ruby-syntax: which sources Ruby must compile or reject, and whether each
// Ruby result meets that expectation. Ruby itself only reports per-file outcomes.
import { createHash } from "node:crypto";

const sha256 = /^[0-9a-f]{64}$/;

/**
 * Validates the expected-rejection list for captured test sources: an object mapping each
 * source's SHA-256 to the parser message its SyntaxError must contain and the test that owns it.
 */
export function parseRejections(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("rejection list must be a JSON object keyed by source SHA-256");
  }
  const rejections = new Map();
  for (const [hash, entry] of Object.entries(value)) {
    if (!sha256.test(hash)) throw new Error(`rejection key ${JSON.stringify(hash)} is not a lowercase SHA-256`);
    if (entry === null || typeof entry !== "object" || Array.isArray(entry)) {
      throw new Error(`rejection ${hash} must be an object with message and test`);
    }
    if (typeof entry.message !== "string" || entry.message === "") {
      throw new Error(`rejection ${hash} needs a non-empty message`);
    }
    if (typeof entry.test !== "string" || entry.test === "") {
      throw new Error(`rejection ${hash} needs the owning test name`);
    }
    rejections.set(hash, entry.message);
  }
  return rejections;
}

/**
 * Deduplicates captured sources by content and attaches each listed rejection. `files` holds
 * `{ path, bytes }`; one path per distinct content is kept. A listed hash no captured source has
 * is stale: the source changed or its test is gone. `fixtures` maps the hashes of tracked
 * rejection fixtures to their messages; a test may read one, but none has to.
 */
export function capturedEntries(files, rejections, fixtures = new Map()) {
  if (files.length === 0) throw new Error("no captured Ruby sources; the capture run recorded nothing");
  const byHash = new Map();
  for (const file of [...files].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0))) {
    const hash = createHash("sha256").update(file.bytes).digest("hex");
    if (!byHash.has(hash)) byHash.set(hash, file.path);
  }
  const entries = [...byHash].map(([hash, path]) => ({
    path,
    hash,
    expected: rejections.get(hash) ?? fixtures.get(hash) ?? "",
  }));
  const stale = [...rejections.keys()].filter((hash) => !byHash.has(hash)).sort();
  return { entries, stale };
}

/**
 * Compares Ruby's per-file results with the expectations. Each result is `{ path, status, error?,
 * message? }` with status `compiled`, `syntax` (SyntaxError) or `error` (any other failure).
 * Every entry needs exactly one result, so a crashed or partial Ruby run cannot pass.
 */
export function judge(entries, results) {
  const failures = [];
  const byPath = new Map();
  for (const result of results) {
    if (byPath.has(result.path)) failures.push(`${result.path}: reported more than once`);
    byPath.set(result.path, result);
  }
  const expectedPaths = new Set(entries.map((entry) => entry.path));
  for (const path of byPath.keys()) {
    if (!expectedPaths.has(path)) failures.push(`${path}: reported, but was not checked`);
  }
  for (const { path, expected } of entries) {
    const result = byPath.get(path);
    if (result === undefined) {
      failures.push(`${path}: no result from Ruby`);
    } else if (result.status === "compiled") {
      if (expected !== "") failures.push(`${path}: compiles, but is listed as an expected rejection`);
    } else if (result.status === "syntax") {
      if (expected === "") failures.push(`${path}: SyntaxError: ${result.message}`);
      else if (!result.message.includes(expected)) {
        failures.push(`${path}: rejected, but without ${JSON.stringify(expected)}: ${result.message}`);
      }
    } else {
      failures.push(`${path}: ${result.error ?? "error"}: ${result.message ?? "no message"}`);
    }
  }
  const rejected = entries.filter((entry) => entry.expected !== "").length;
  return { failures, rejected };
}
