// Report-only evaluation of the recall corpus: runs every case through the binary, classifies the
// findings against the manifest without asserting, and writes per-rule metrics. It is not a gate;
// it exits 1 only when a case cannot be observed (the binary will not start or wrote no report).
//
//   node scripts/evaluate/corpus-evaluate.mjs [--binary PATH] [--case NAME] [--json FILE] [--markdown FILE]

import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { observeRecallCase } from "../check/lib/recall-runner.mjs";
import { evaluateCase } from "./lib/classify.mjs";
import { aggregate, renderMarkdown } from "./lib/report.mjs";

const { values } = parseArgs({
  options: {
    binary: { type: "string", default: "target/release/cuke-dedup" },
    case: { type: "string" },
    json: { type: "string" },
    markdown: { type: "string" },
  },
});

const recallRoot = resolve("fixtures/recall");
const manifest = JSON.parse(await readFile(join(recallRoot, "manifest.json"), "utf8"));
const cases = values.case ? manifest.cases.filter((testCase) => testCase.name === values.case) : manifest.cases;
if (cases.length === 0) {
  console.error(`no recall case named ${values.case}`);
  process.exit(2);
}

const scratch = await mkdtemp(join(tmpdir(), "cuke-dedup-evaluate-"));
try {
  const classified = [];
  for (const testCase of cases) {
    classified.push(await evaluateCase(testCase, () => observeRecallCase({
      binary: resolve(values.binary),
      recallRoot,
      scratch: join(scratch, testCase.name),
      testCase,
    })));
  }
  const report = aggregate(classified);
  if (values.json) await writeFile(values.json, `${JSON.stringify(report, null, 2)}\n`);
  const markdown = renderMarkdown(report);
  if (values.markdown) await writeFile(values.markdown, markdown);
  if (!values.json && !values.markdown) process.stdout.write(markdown);
  if (report.unobservable.length > 0) process.exitCode = 1;
} finally {
  await rm(scratch, { recursive: true, force: true });
}
