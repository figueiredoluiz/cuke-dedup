// Distribution and coverage of the recall corpus from its manifest: no binary is run.
//
//   node scripts/evaluate/corpus-distribution.mjs [--floor N] [--json FILE] [--markdown FILE]

import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { distribution, renderDistribution } from "./lib/distribution.mjs";

let values;
try {
  ({ values } = parseArgs({ options: { floor: { type: "string", default: "1" }, json: { type: "string" }, markdown: { type: "string" } } }));
} catch (error) {
  console.error(`corpus-distribution.mjs: ${error.message}`);
  process.exit(2);
}
const floor = Number(values.floor);
if (!Number.isSafeInteger(floor) || floor < 0) {
  console.error("usage: corpus-distribution.mjs [--floor N>=0] [--json FILE] [--markdown FILE]");
  process.exit(2);
}
const manifest = JSON.parse(await readFile(join(resolve("fixtures/recall"), "manifest.json"), "utf8"));
const behavior = JSON.parse(await readFile(join(resolve("fixtures/corpus"), "manifest.json"), "utf8"));
const report = distribution(manifest, { floor, behaviorCases: behavior.cases.length });
if (values.json) await writeFile(values.json, `${JSON.stringify(report, null, 2)}\n`);
const markdown = renderDistribution(report);
if (values.markdown) await writeFile(values.markdown, markdown);
if (!values.json && !values.markdown) process.stdout.write(markdown);
