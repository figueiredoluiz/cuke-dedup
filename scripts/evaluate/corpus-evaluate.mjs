// Report-only evaluation of the labelled corpora: runs every case of the recall, behavior,
// language-parity and Ruby-parity corpora through the binary, classifies the outcome against the manifest without
// asserting, and writes per-corpus and per-rule metrics. It is not a gate. Exit codes: 0 when every
// selected case was scored, 1 when a case could not be observed or scored, 2 for an unknown case or
// corpus.
//
//   node scripts/evaluate/corpus-evaluate.mjs [--binary PATH] [--corpus NAME] [--case NAME]
//     [--json FILE] [--markdown FILE]

import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { observeBehaviorCase } from "../check/lib/behavior-runner.mjs";
import { LANGUAGE_EXTENSIONS, observeParityLane, observeRubyGroup } from "../check/lib/parity-runner.mjs";
import { observeRecallCase } from "../check/lib/recall-runner.mjs";
import { BEHAVIOR_CORPUS, classifyBehaviorCase } from "./lib/classify-behavior.mjs";
import { classifyOracle } from "./lib/classify-oracle.mjs";
import { evaluateCase } from "./lib/classify.mjs";
import { loadCorpora } from "./lib/corpora.mjs";
import { aggregate, renderMarkdown } from "./lib/report.mjs";

const { values } = parseArgs({
  options: {
    binary: { type: "string", default: "target/release/cuke-dedup" },
    corpus: { type: "string" },
    case: { type: "string" },
    json: { type: "string" },
    markdown: { type: "string" },
  },
});

const binary = resolve(values.binary);
/** The paired corpus's name in the report. */
const PARITY_CORPUS = "language-parity";
/** The Ruby-parity corpus's name in the report. */
const RUBY_CORPUS = "ruby-parity";
/** How each corpus runs one of its cases and classifies the outcome; each returns its classified results. */
const runners = {
  /** Observes a recall case and classifies it against its expected and absent findings. */
  recall: async (item, scratch) => [await evaluateCase(item, () => observeRecallCase({
    binary, recallRoot: resolve("fixtures/recall"), scratch, testCase: item,
  }))],
  /** Observes a behavior case and classifies its per-rule counts. */
  behavior: async (item, scratch) => [await evaluateCase(item, () => observeBehaviorCase({
    binary, corpusRoot: resolve("fixtures/corpus"), scratch, testCase: item,
  }), classifyBehaviorCase, { corpus: BEHAVIOR_CORPUS })],
  /** Runs a paired case once per language against the same oracle and scores each run separately. */
  [PARITY_CORPUS]: async (item, scratch) => {
    const results = [];
    for (const [language, extension] of Object.entries(LANGUAGE_EXTENSIONS)) {
      results.push(await evaluateCase(
        { name: item.id },
        () => observeParityLane({ binary, directory: join(scratch, language), item, language, extension }),
        (named, observation) => classifyOracle({ corpus: PARITY_CORPUS, name: named.name, lane: language, oracle: item.expected, ownership: false }, observation),
        { corpus: PARITY_CORPUS, lane: language },
      ));
    }
    return results;
  },
  /** Runs a Ruby-parity group against its own oracle; every finding must have exactly one owner. */
  [RUBY_CORPUS]: async (item, scratch) => [await evaluateCase(
    item,
    () => observeRubyGroup({ binary, root: resolve("fixtures/ruby-parity"), scratch, oracle: item }),
    (group, observation) => classifyOracle({ corpus: RUBY_CORPUS, name: group.name, lane: null, oracle: group, ownership: true }, observation),
    { corpus: RUBY_CORPUS },
  )],
};

if (values.corpus !== undefined && !Object.hasOwn(runners, values.corpus)) {
  console.error(`no corpus named ${values.corpus}; expected one of ${Object.keys(runners).join(", ")}`);
  process.exit(2);
}
/** A case's name: recall and behavior cases and Ruby groups carry `name`, paired cases `id`. */
const nameOf = (item) => item.name ?? item.id;
const selected = (await loadCorpora())
  .filter((corpus) => values.corpus === undefined || corpus.name === values.corpus)
  .map((corpus) => ({ ...corpus, items: corpus.items.filter((item) => values.case === undefined || nameOf(item) === values.case) }));
if (values.case !== undefined && selected.every((corpus) => corpus.items.length === 0)) {
  console.error(`no case named ${values.case}${values.corpus ? ` in corpus ${values.corpus}` : ""}`);
  process.exit(2);
}

const scratch = await mkdtemp(join(tmpdir(), "cuke-dedup-evaluate-"));
try {
  const classified = [];
  for (const corpus of selected) {
    for (const item of corpus.items) {
      classified.push(...await runners[corpus.name](item, join(scratch, corpus.name, nameOf(item))));
    }
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
