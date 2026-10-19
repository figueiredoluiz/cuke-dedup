// Candidate recall of the recall corpus: for every pair finding a case expects, which candidate
// sources proposed the pair, and where each declared-but-unreported pair stopped. Report-only; it
// exits 1 only when a case cannot be observed.
//
//   node scripts/evaluate/candidate-recall.mjs [--binary PATH] [--case NAME] [--json FILE] [--markdown FILE]

import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { observeRecallCase } from "../check/lib/recall-runner.mjs";
import { aggregateCandidateRecall, recallForCase, renderCandidateMarkdown } from "./lib/candidate-recall.mjs";

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

const scratch = await mkdtemp(join(tmpdir(), "cuke-dedup-candidate-recall-"));
try {
  const results = [];
  for (const testCase of cases) {
    results.push(await recallForCase(testCase, () => observeRecallCase({
      binary: resolve(values.binary),
      recallRoot,
      scratch: join(scratch, testCase.name),
      testCase,
      emitPairs: true,
    })));
  }
  const report = aggregateCandidateRecall(results);
  if (values.json) await writeFile(values.json, `${JSON.stringify(report, null, 2)}\n`);
  const markdown = renderCandidateMarkdown(report);
  if (values.markdown) await writeFile(values.markdown, markdown);
  if (!values.json && !values.markdown) process.stdout.write(markdown);
  if (report.unobservable.length > 0) process.exitCode = 1;
} finally {
  await rm(scratch, { recursive: true, force: true });
}
