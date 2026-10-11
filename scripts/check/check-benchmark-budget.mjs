import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { validateBenchmark } from "./lib/benchmark-budget.mjs";

const path = process.argv[2];
assert.ok(path, "usage: node scripts/check/check-benchmark-budget.mjs <benchmark.json>");
const report = JSON.parse(await readFile(path, "utf8"));
const budgets = JSON.parse(
  await readFile(new URL("./benchmark-budgets.json", import.meta.url), "utf8"),
);
const { advisories, failures } = validateBenchmark(report, budgets);
console.log(`Performance gate: ${report.results.length} profiles; ${advisories.join("; ")}.`);
if (failures.length > 0) {
  for (const failure of failures) console.error(failure);
  process.exitCode = 1;
}
