// Compares the previous release with a new binary on the committed corpus and writes the table for
// the release notes. Both binaries are run on the same cases; counts are exact, timing is advisory.
//
//   node scripts/release/release-comparison.mjs --previous BIN --candidate BIN
//     [--previous-label vX] [--candidate-label vY] [--repeats N] [--timeout-ms N] [--markdown FILE] [--json FILE]
//
// Exit 1 when no case could be compared; 2 for a bad invocation.

import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { compare, loadCases, renderTable, runCase, summarizeRuns } from "./lib/release-comparison.mjs";

let values;
try {
  ({ values } = parseArgs({
    options: {
      previous: { type: "string" },
      candidate: { type: "string" },
      "previous-label": { type: "string", default: "previous release" },
      "candidate-label": { type: "string", default: "this release" },
      repeats: { type: "string", default: "3" },
      "timeout-ms": { type: "string", default: "120000" },
      markdown: { type: "string" },
      json: { type: "string" },
    },
  }));
} catch (error) {
  // An unknown option or a missing value is a bad invocation, like the checks below.
  console.error(`release-comparison.mjs: ${error.message}`);
  process.exit(2);
}
const repeats = Number(values.repeats);
const timeoutMs = Number(values["timeout-ms"]);
if (!values.previous || !values.candidate || !Number.isSafeInteger(repeats) || repeats < 1 || !Number.isSafeInteger(timeoutMs) || timeoutMs < 1) {
  console.error("usage: release-comparison.mjs --previous BIN --candidate BIN [--repeats N>=1] [--timeout-ms N>=1] [--markdown FILE] [--json FILE]");
  process.exit(2);
}

const scratch = await mkdtemp(join(tmpdir(), "cuke-dedup-release-comparison-"));
try {
  const cases = await loadCases(resolve("."));
  const summaries = [];
  for (const testCase of cases) {
    const entry = { name: testCase.name };
    for (const side of ["previous", "candidate"]) {
      const runs = [];
      for (let repeat = 0; repeat < repeats; repeat += 1) {
        runs.push(await runCase(resolve(values[side]), testCase, join(scratch, testCase.name, side, String(repeat)), timeoutMs));
      }
      entry[side] = summarizeRuns(runs);
    }
    summaries.push(entry);
  }
  const comparison = compare(summaries);
  // Nothing comparable means an unusable invocation (a binary that will not run), not a release with
  // no findings; the table would read as a clean zero.
  // The JSON is written either way, so the full exclusion reasons survive an unusable invocation.
  if (values.json) await writeFile(values.json, `${JSON.stringify(comparison, null, 2)}\n`);
  if (comparison.compared === 0) {
    console.error(`release comparison: no case could be compared; first exclusions:\n${comparison.excluded.slice(0, 3).map((entry) => `  ${entry.name}: ${entry.reason}`).join("\n")}`);
    // Not `process.exit`: the scratch tree is removed below, then the run ends with this status.
    process.exitCode = 1;
  } else {
    const markdown = renderTable(comparison, { previous: values["previous-label"], candidate: values["candidate-label"] });
    if (values.markdown) await writeFile(values.markdown, markdown);
    if (!values.json && !values.markdown) process.stdout.write(markdown);
  }
} finally {
  await rm(scratch, { recursive: true, force: true });
}
