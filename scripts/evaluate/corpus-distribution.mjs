// Distribution and coverage of the labelled corpora from their manifests: no binary is run. The first
// part is the recall corpus in detail; the second compares every corpus side by side.
//
//   node scripts/evaluate/corpus-distribution.mjs [--floor N] [--json FILE] [--markdown FILE]

import { writeFile } from "node:fs/promises";
import { parseArgs } from "node:util";
import { loadCorpora } from "./lib/corpora.mjs";
import { distribution, overview, renderDistribution, renderOverview } from "./lib/distribution.mjs";

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
const corpora = await loadCorpora();
const all = overview(corpora, { floor });
const report = { ...distribution({ cases: corpora[0].items }, { floor }), allCorpora: all };
if (values.json) await writeFile(values.json, `${JSON.stringify(report, null, 2)}\n`);
const markdown = `${renderDistribution(report)}\n${renderOverview(all)}`;
if (values.markdown) await writeFile(values.markdown, markdown);
if (!values.json && !values.markdown) process.stdout.write(markdown);
