// Exact observed outcomes ratchet regressions without redefining completion oracles.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";

const object = (value) => value !== null && typeof value === "object"
  && !Array.isArray(value) && Object.getPrototypeOf(value) === Object.prototype;
const statuses = new Set(["pass", "known-gap", "accepted-difference"]);
const nonempty = (value) => typeof value === "string" && value.trim().length > 0;
function canonical(value) {
  if (value === null || typeof value === "string" || typeof value === "boolean") return value;
  if (typeof value === "number") { assert.ok(Number.isFinite(value), "non-finite snapshot number"); return value; }
  if (Array.isArray(value)) {
    assert.deepEqual(Object.keys(value), Array.from({ length: value.length }, (_, index) => String(index)), "sparse or extended snapshot array");
    assert.equal(Object.getOwnPropertySymbols(value).length, 0, "symbol snapshot keys");
    return value.map(canonical);
  }
  assert.ok(object(value), "snapshot must contain JSON values and plain objects");
  assert.equal(Object.getOwnPropertySymbols(value).length, 0, "symbol snapshot keys");
  return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]));
}

export function snapshot(value) {
  return createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex");
}

export function regressionFailures(observations, manifest) {
  assert.ok(object(manifest) && manifest.schemaVersion === 1, "invalid regression manifest schema");
  assert.ok(object(manifest.cases) && Object.keys(manifest.cases).length > 0, "empty or malformed regression cases");
  for (const [id, entry] of Object.entries(manifest.cases)) {
    assert.ok(nonempty(id) && object(entry), `invalid disposition: ${id}`);
    assert.ok(Object.keys(entry).every((key) => ["status", "digest", "reason"].includes(key)), `${id}: unknown disposition field`);
    assert.ok(statuses.has(entry.status), `${id}: invalid disposition status`);
    assert.ok(typeof entry.digest === "string" && /^[a-f0-9]{64}$/.test(entry.digest), `${id}: invalid snapshot digest`);
    if (entry.status !== "pass" || Object.hasOwn(entry, "reason")) assert.ok(nonempty(entry.reason), `${id}: disposition requires a reason`);
  }
  if (!Array.isArray(observations) || observations.length === 0) return ["empty or malformed regression observations"];
  const failures = [], seen = new Set();
  for (const [index, observation] of observations.entries()) {
    if (!object(observation) || !nonempty(observation.id)) {
      failures.push(`observation ${index}: missing or invalid ID`); continue;
    }
    const { id, outcome, deficits } = observation;
    if (seen.has(id)) { failures.push(`${id}: duplicate observation`); continue; }
    seen.add(id);
    if (!Object.hasOwn(manifest.cases, id)) failures.push(`${id}: unclassified case`);
    if (Object.hasOwn(observation, "error")) {
      failures.push(`${id}: operational error: ${String(observation.error)}`); continue;
    }
    if (!object(outcome) || !Array.isArray(deficits) || deficits.some((value) => typeof value !== "string")
      || (Object.hasOwn(observation, "difference") && typeof observation.difference !== "boolean")) {
      failures.push(`${id}: malformed observation outcome, deficits, or difference`); continue;
    }
    let digest;
    try { digest = snapshot({ outcome, deficits }); }
    catch (error) { failures.push(`${id}: malformed observation: ${error.message}`); continue; }
    const expected = manifest.cases[id];
    if (!Object.hasOwn(manifest.cases, id)) continue;
    const status = deficits.length ? "known-gap" : observation.difference === true ? "accepted-difference" : "pass";
    if (status !== expected.status) failures.push(`${id}: disposition changed from ${expected.status} to ${status}; reassess explicitly`);
    if (digest !== expected.digest) failures.push(`${id}: outcome or deficits changed; expected ${expected.digest}, actual ${digest}; reassess explicitly`);
  }
  for (const id of Object.keys(manifest.cases)) if (!seen.has(id)) failures.push(`${id}: missing observation`);
  return failures;
}
