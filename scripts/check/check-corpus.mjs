import assert from "node:assert/strict";
import {
  cp,
  mkdir,
  mkdtemp,
  readFile,
  rm,
} from "node:fs/promises";
import { readdirSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import {
  countMatches,
  expectedTotal as sumExpected,
  findingOwners,
  locationLabel,
} from "./lib/recall-oracle.mjs";

// The corpus check runs whatever binary is on disk. A binary older than the sources silently
// analyzes with stale behavior, so a passing or failing run reflects code that is no longer
// checked out — a repeatedly confusing failure mode locally. Warn (never fail: CI always builds
// fresh, and the warning is for the local edit-then-check loop) when the binary predates any
// source or is missing.
function newestSource() {
  let newest = { mtimeMs: 0, path: null };
  const consider = (path) => {
    const { mtimeMs } = statSync(path);
    if (mtimeMs > newest.mtimeMs) newest = { mtimeMs, path };
  };
  const walk = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const full = join(directory, entry.name);
      if (entry.isDirectory()) walk(full);
      else consider(full);
    }
  };
  walk(resolve("src"));
  consider(resolve("Cargo.toml"));
  // A dependency-only bump touches the lockfile without touching `src` or the manifest, and can
  // change analyzer behavior through a rebuilt dependency, so it counts toward staleness too.
  consider(resolve("Cargo.lock"));
  return newest;
}

function warnIfBinaryStale(binaryPath) {
  let binaryMtimeMs;
  try {
    binaryMtimeMs = statSync(binaryPath).mtimeMs;
  } catch {
    process.stderr.write(
      `\n⚠️  ${binaryPath} is missing — run \`cargo build --release\` before the corpus check.\n\n`,
    );
    return;
  }
  const newest = newestSource();
  if (newest.mtimeMs > binaryMtimeMs) {
    process.stderr.write(
      `\n⚠️  ${binaryPath} is older than ${newest.path} — it may analyze with stale behavior.\n` +
        `    Run \`cargo build --release\` before the corpus check.\n\n`,
    );
  }
}

const binary = resolve(process.argv[2] || "target/release/cuke-dedup");
warnIfBinaryStale(binary);
const corpus = resolve(process.argv[3] || "fixtures/corpus");
const manifest = JSON.parse(await readFile(join(corpus, "manifest.json"), "utf8"));
assert.equal(manifest.schemaVersion, 1);
const recallRoot = resolve("fixtures/recall");
const recallManifest = JSON.parse(
  await readFile(join(recallRoot, "manifest.json"), "utf8"),
);
assert.equal(recallManifest.schemaVersion, 1);

// Narrows a run to one recall case, so a deliberate break can be attributed to the case meant to
// catch it. The aggregate run stops at the first failure and cannot show that.
const onlyCase = process.env.CUKE_DEDUP_CORPUS_CASE;

const temporary = await mkdtemp(join(tmpdir(), "cuke-dedup-corpus-"));
try {
  for (const testCase of onlyCase ? [] : manifest.cases) {
    const caseRoot = join(temporary, testCase.name, "case");
    const output = join(temporary, testCase.name, "report");
    await cp(join(corpus, testCase.path), caseRoot, { recursive: true });
    for (const materialized of testCase.materialize || []) {
      const destination = join(caseRoot, materialized.destination);
      await mkdir(dirname(destination), { recursive: true });
      await cp(join(caseRoot, materialized.source), destination);
    }
    const run = spawnSync(
      binary,
      [".", ...testCase.arguments, "--reporters", "json", "--output", output],
      { cwd: caseRoot, encoding: "utf8" },
    );
    assert.equal(run.status, testCase.expectedExit, `${testCase.name}: ${run.stderr}`);
    const report = JSON.parse(await readFile(join(output, "cuke-dedup.json"), "utf8"));
    const expected = testCase.expected;
    assert.equal(report.summary.definitionsAnalyzed, expected.definitions, `${testCase.name}: definitions`);
    assert.equal(report.summary.featureStepsAnalyzed, expected.featureSteps, `${testCase.name}: feature steps`);
    assert.equal(report.summary.duplication.duplicatedDefinitions, expected.duplicatedDefinitions, `${testCase.name}: duplicated definitions`);
    assert.equal(report.summary.duplication.percentage, expected.percentage, `${testCase.name}: percentage`);
    assert.equal(report.summary.duplication.threshold, expected.threshold, `${testCase.name}: threshold`);
    assert.equal(report.summary.duplication.passed, expected.passed, `${testCase.name}: result`);
    assert.deepEqual(report.summary.byRule, expected.byRule, `${testCase.name}: rules`);
    for (const message of testCase.stderrIncludes || []) {
      assert.ok(run.stderr.includes(message), `${testCase.name}: missing stderr message ${message}`);
    }
  }

  const recall = await validateRecallCorpus(temporary);

  const parityCases = [
    { name: "threshold-group", root: join(corpus, "threshold-group"), exit: 0, errors: 2 },
    { name: "cluster-boundary", root: join(recallRoot, "exact-group-cluster-boundary"), exit: 0, clusters: 2 },
    { name: "warning", root: join(recallRoot, "gherkin-step-sources"), exit: 0, warnings: 1 },
    { name: "suppressed", root: join(corpus, "parity-suppressed"), exit: 0, suppressed: 1 },
    { name: "incomplete", root: join(corpus, "malformed-matcher"), exit: 2, incomplete: true },
  ];
  let parityFindings = 0;
  for (const parityCase of parityCases) {
    parityFindings += await validateReporterParity(parityCase, temporary);
  }
  console.log(
    `Validated ${manifest.cases.length} behavior cases, ${recall.cases} recall cases, and ${parityFindings} parity findings across terminal, JSON, JSONL, HTML, and SARIF.`,
  );
  console.log(
    `Recall baseline: ${recall.detected}/${recall.desired} desired findings (${(recall.ratio * 100).toFixed(1)}%); ${recall.knownMisses} explicit known misses.`,
  );
} finally {
  await rm(temporary, { recursive: true, force: true });
}

function parityFinding(finding) {
  return {
    rule: finding.rule,
    severity: finding.severity,
    message: finding.message,
    primary: finding.primary,
    related: finding.related,
    relatedLocationsTruncated: finding.relatedLocationsTruncated,
    suggestedAction: finding.suggestedAction,
    suppression: finding.suppression,
    evidence: {
      matcherSimilarity: finding.evidence.matcherSimilarity,
      handlerSimilarity: finding.evidence.handlerSimilarity,
      matcherDifference: finding.evidence.matcherDifference,
      handlerEvidence: finding.evidence.handlerEvidence,
      comparison: finding.evidence.comparison ?? null,
      cluster: finding.evidence.cluster ?? null,
    },
  };
}

function sarifLocation(location) {
  const physical = location.physicalLocation;
  return {
    path: physical.artifactLocation.uri,
    line: physical.region.startLine,
    column: physical.region.startColumn,
    endLine: physical.region.endLine,
    endColumn: physical.region.endColumn,
  };
}

function displayLocation(location) {
  return `${location.path}:${location.line}:${location.column}`;
}

function htmlEscape(value) {
  return value.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;").replaceAll("'", "&#39;");
}

async function validateReporterParity({ name, root, exit, errors, warnings, suppressed, clusters, incomplete }, temporary) {
  const output = join(temporary, "reporter-parity", name);
  const run = spawnSync(binary, [".", "--reporters", "terminal,json,html,sarif", "--output", output],
    { cwd: root, encoding: "utf8" });
  const stream = spawnSync(binary, [".", "--reporters", "jsonl"],
    { cwd: root, encoding: "utf8" });
  assert.equal(run.status, exit, `${name}: terminal/report exit: ${run.stderr}`);
  assert.equal(stream.status, exit, `${name}: JSONL exit: ${stream.stderr}`);
  const json = JSON.parse(await readFile(join(output, "cuke-dedup.json"), "utf8"));
  const html = await readFile(join(output, "cuke-dedup.html"), "utf8");
  const sarif = JSON.parse(await readFile(join(output, "cuke-dedup.sarif"), "utf8"));
  const jsonl = stream.stdout.trimEnd().split("\n").map((line) => JSON.parse(line));
  const records = jsonl.slice(0, -1);
  const summary = jsonl.at(-1);
  const embedded = html.match(/<script type="application\/json" id="report-data">([\s\S]*?)<\/script>/);
  assert.ok(embedded, `${name}: HTML embedded JSON missing`);
  assert.deepEqual(JSON.parse(embedded[1]), json, `${name}: HTML and JSON differ`);
  assert.equal(summary.type, "summary", `${name}: JSONL summary missing`);
  assert.equal(summary.schemaVersion, "3", `${name}: JSONL schema`);
  assert.equal(typeof summary.toolVersion, "string", `${name}: JSONL tool version`);
  assert.ok(summary.metrics.filesDiscovered > 0, `${name}: discovered files`);
  assert.equal(summary.recordCount, jsonl.length, `${name}: JSONL record count`);
  assert.equal(summary.truncated, false, `${name}: JSONL stream truncated`);
  assert.deepEqual(summary.summary, json.summary, `${name}: JSONL summary differs`);
  assert.deepEqual(summary.corpus, json.corpus, `${name}: JSONL corpus diagnostics differ`);
  assert.deepEqual(summary.analysis, json.analysis, `${name}: JSONL analysis diagnostics differ`);
  assert.equal(summary.corpus.incomplete, incomplete === true, `${name}: incomplete fixture control`);

  const active = json.findings.filter((finding) => finding.suppression === null);
  assert.equal(json.findingsTruncated, 0, `${name}: buffered JSON truncated`);
  assert.equal(json.summary.findings, active.length, `${name}: active count`);
  assert.equal(json.summary.suppressed, json.findings.length - active.length, `${name}: suppressed count`);
  if (errors !== undefined) assert.equal(json.summary.errors, errors, `${name}: error control`);
  if (warnings !== undefined) assert.equal(json.summary.warnings, warnings, `${name}: warning control`);
  if (suppressed !== undefined) assert.equal(json.summary.suppressed, suppressed, `${name}: suppression control`);
  if (clusters !== undefined) assert.equal(json.findings.filter((finding) => finding.evidence.cluster).length,
    clusters, `${name}: cluster control`);
  assert.equal(records.length, json.findings.length, `${name}: JSONL findings count`);
  assert.deepEqual(records.map(parityFinding).sort(byJson), json.findings.map(parityFinding).sort(byJson),
    `${name}: JSONL and JSON finding metadata differ`);
  for (const record of records) {
    assert.equal(record.type, "finding", `${name}: JSONL record type`);
    assert.equal(record.schemaVersion, "3", `${name}: JSONL finding schema`);
    assert.equal(record.toolVersion, summary.toolVersion, `${name}: JSONL finding tool version`);
    assert.equal(typeof record.contributesToThreshold, "boolean", `${name}: threshold participation`);
    assert.match(record.fingerprint, /^[0-9a-f]{32}$/, `${name}: JSONL identity`);
    assert.equal(record.active, record.suppression === null, `${name}: JSONL active state`);
    assert.deepEqual(record.truncatedFields, [], `${name}: fixture exceeds JSONL text limit`);
  }

  const results = sarif.runs[0].results;
  const invocation = sarif.runs[0].invocations[0];
  assert.equal(results.length, active.length, `${name}: SARIF active count`);
  assert.equal(invocation.executionSuccessful, exit === 0, `${name}: SARIF execution status`);
  assert.equal(invocation.properties.activeFindings, json.summary.findings, `${name}: SARIF active summary`);
  assert.equal(invocation.properties.suppressedFindings, json.summary.suppressed, `${name}: SARIF suppressed summary`);
  assert.equal(invocation.properties.findingsTruncated, json.findingsTruncated, `${name}: SARIF truncation`);
  assert.equal(invocation.properties.thresholdPassed, json.summary.duplication.passed, `${name}: SARIF threshold`);
  assert.equal(invocation.properties.duplicatedDefinitions, json.summary.duplication.duplicatedDefinitions,
    `${name}: SARIF duplicated definitions`);
  assert.equal(invocation.properties.totalDefinitions, json.summary.duplication.totalDefinitions,
    `${name}: SARIF total definitions`);
  assert.equal(invocation.properties.duplicationPercentage, json.summary.duplication.percentage,
    `${name}: SARIF duplication percentage`);
  assert.deepEqual(invocation.properties.contributingRules, json.summary.duplication.rules,
    `${name}: SARIF contributing rules`);
  assert.equal(invocation.properties.retainedFindings, results.length, `${name}: SARIF retained count`);
  assert.deepEqual(invocation.properties.corpus, json.corpus, `${name}: SARIF corpus diagnostics`);
  assert.deepEqual(invocation.properties.analysis, json.analysis, `${name}: SARIF analysis diagnostics`);

  const activeRecords = records.filter((record) => record.active);
  assert.equal(activeRecords.length, active.length, `${name}: JSONL active count`);
  assert.deepEqual(results.map((result) => result.partialFingerprints["cukeDedupFingerprint/v3"]).sort(),
    activeRecords.map((record) => record.fingerprint).sort(), `${name}: SARIF identities differ`);
  for (const result of results) {
    const fingerprint = result.partialFingerprints["cukeDedupFingerprint/v3"];
    const record = activeRecords.find((candidate) => candidate.fingerprint === fingerprint);
    assert.ok(record, `${name}: SARIF identity ${fingerprint} missing from JSONL`);
    assert.equal(result.ruleId, record.rule, `${name}: SARIF rule`);
    assert.equal(result.level, record.severity, `${name}: SARIF severity`);
    assert.equal(result.message.text, record.message, `${name}: SARIF message`);
    assert.deepEqual(sarifLocation(result.locations[0]), record.primary, `${name}: SARIF primary location`);
    assert.deepEqual(result.relatedLocations.map(sarifLocation), record.related,
      `${name}: SARIF related locations`);
    assert.equal(result.properties.relatedLocationsTruncated, record.relatedLocationsTruncated,
      `${name}: SARIF related truncation`);
    assert.equal(result.properties.clusterSize, record.evidence.cluster?.memberCount ?? null,
      `${name}: SARIF cluster size`);
  }
  assert.equal((run.stdout.match(/^  \[(?:error|warning)\]/gm) || []).length, active.length,
    `${name}: terminal visible count`);
  const cards = html.match(/<article class="finding"[\s\S]*?<\/article>/g) || [];
  assert.equal(cards.length, active.length,
    `${name}: HTML visible count`);
  for (const finding of active) {
    assert.match(run.stdout, new RegExp(escapeRegex(`[${finding.severity}] ${finding.message}`)),
      `${name}: terminal severity/message`);
    assert.ok(run.stdout.includes(`--> ${displayLocation(finding.primary)}`), `${name}: terminal primary location`);
    for (const related of finding.related) {
      assert.ok(run.stdout.includes(`related: ${displayLocation(related)}`), `${name}: terminal related location`);
    }
    const card = cards.find((item) => item.includes(`data-rule="${finding.rule}"`)
      && item.includes(`data-severity="${finding.severity}"`)
      && item.includes(`<h2>${htmlEscape(finding.message)}</h2>`)
      && item.includes(`<p class="location">${htmlEscape(displayLocation(finding.primary))}</p>`));
    assert.ok(card, `${name}: HTML severity/message/primary location`);
    for (const related of finding.related) {
      assert.ok(card.includes(`<li><code>${htmlEscape(displayLocation(related))}</code></li>`),
        `${name}: HTML related location`);
    }
  }
  assert.match(run.stdout, new RegExp(`Found ${json.summary.findings} findings?\\.`), `${name}: terminal summary`);
  assert.match(run.stdout, new RegExp(`Analyzed ${json.summary.definitionsAnalyzed} definitions? and ${json.summary.featureStepsAnalyzed} feature steps?: ${json.summary.errors} errors?, ${json.summary.warnings} warnings?, ${json.summary.suppressed} suppressed`),
    `${name}: terminal counts`);
  assert.match(html, new RegExp(`${json.summary.definitionsAnalyzed} definitions?`), `${name}: HTML definition count`);
  assert.match(html, new RegExp(`${json.summary.featureStepsAnalyzed} feature steps?`), `${name}: HTML feature count`);
  assert.match(html, new RegExp(`${json.summary.errors} errors?`), `${name}: HTML error count`);
  assert.match(html, new RegExp(`${json.summary.warnings} warnings?`), `${name}: HTML warning count`);
  assert.match(html, new RegExp(`${json.summary.suppressed} suppressed`), `${name}: HTML suppressed count`);
  return json.findings.length;
}

function byJson(left, right) {
  return JSON.stringify(left).localeCompare(JSON.stringify(right));
}

/**
 * Runs every recall case and returns the corpus-wide recall figures.
 *
 * Recall is deliberately a ratio of *desired* findings rather than a pass/fail count: a case may
 * assert a finding the analyzer cannot yet produce, recorded as a known miss, and the ratio is what
 * keeps those visible instead of letting them read as absence.
 *
 * @param {string} temporary Scratch directory each case is copied into before it runs.
 * @returns {Promise<{cases: number, detected: number, desired: number, ratio: number,
 *   knownMisses: number}>} Per-run totals; `ratio` is `detected / desired`.
 */
async function validateRecallCorpus(temporary) {
  const knownIds = new Set();
  let detected = 0;
  let knownMisses = 0;

  // The recall baseline is only asserted over a full run, since a subset cannot meet a
  // corpus-wide ratio.
  const only = onlyCase;
  const cases = only
    ? recallManifest.cases.filter((testCase) => testCase.name === only)
    : recallManifest.cases;
  assert.ok(cases.length > 0, `no recall case named ${only}`);

  // Counting cases protects the count, not the coverage. Without these two, a case carrying real
  // expectations could be deleted and replaced by a vacuous one — or by a copy of an easier case
  // under a new name — leaving `minimumCases` satisfied and the recall ratio unharmed.
  const names = cases.map((testCase) => testCase.name);
  const repeated = names.filter((name, index) => names.indexOf(name) !== index);
  assert.deepEqual(repeated, [], `duplicate recall case name(s): ${repeated.join(", ")}`);
  for (const testCase of cases) {
    // `findingMatches` compares `rule` first, so an expectation without one can never match any
    // finding: as an absence it is satisfied vacuously, and as a positive it can only fail. Either
    // way it asserts nothing, which counting entries alone would not notice — and a malformed
    // expectation is far likelier as an authoring slip than as an attempt to game the floor.
    const asserted = [
      ...(testCase.expectedFindings || []),
      ...(testCase.expectedAbsent || []),
      ...(testCase.knownMisses || []),
    ];
    for (const expectation of asserted) {
      assert.ok(
        expectation.rule,
        `${testCase.name}: an expectation names no rule, so it can never match a finding: ${JSON.stringify(expectation)}`,
      );
    }
    assert.ok(
      asserted.length > 0,
      `${testCase.name}: asserts no finding, non-finding or known miss, so it occupies a corpus slot without pinning anything`,
    );
  }

  for (const testCase of cases) {
    const caseRoot = join(temporary, "recall", testCase.name, "case");
    const output = join(temporary, "recall", testCase.name, "report");
    await cp(join(recallRoot, testCase.path), caseRoot, { recursive: true });
    const run = spawnSync(
      binary,
      [".", ...(testCase.arguments || []), "--reporters", "json", "--output", output],
      { cwd: caseRoot, encoding: "utf8" },
    );
    assert.equal(run.status, testCase.expectedExit || 0, `${testCase.name}: ${run.stderr}`);
    const report = JSON.parse(await readFile(join(output, "cuke-dedup.json"), "utf8"));
    assert.equal(
      report.summary.definitionsAnalyzed,
      testCase.expectedDefinitions,
      `${testCase.name}: definitions`,
    );
    assert.ok(Number.isSafeInteger(testCase.expectedFeatureSteps) && testCase.expectedFeatureSteps >= 0,
      `${testCase.name}: expectedFeatureSteps must declare a nonnegative integer`);
    assert.equal(report.summary.featureStepsAnalyzed, testCase.expectedFeatureSteps,
      `${testCase.name}: extracted feature steps`);
    for (const [source, expected] of Object.entries(testCase.expectedCandidateSources || {})) {
      assert.equal(
        report.analysis.candidateSources[source]?.evaluated,
        expected,
        `${testCase.name}: ${source} candidate count`,
      );
    }

    const activeFindings = report.findings.filter((finding) => finding.suppression === null);
    const expectedFindings = testCase.expectedFindings || [];
    // `count` lets one expectation stand for several findings sharing every asserted field. It
    // defaults to 1 so existing entries keep their exact meaning.
    const expectedTotal = sumExpected(expectedFindings);
    for (const expected of expectedFindings) {
      assertFindingCount(
        activeFindings,
        expected,
        expected.count ?? 1,
        `${testCase.name}: expected finding`,
      );
    }
    // Counting each expectation independently is not enough: one finding can satisfy a broad and a
    // narrow expectation at once, leaving room for a second unclassified finding while the totals
    // still balance. Requiring a one-to-one mapping closes that, and `count` still lets a single
    // expectation own several findings.
    for (const finding of activeFindings) {
      const owners = findingOwners(finding, expectedFindings);
      assert.equal(
        owners.length,
        1,
        `${testCase.name}: each finding needs exactly one expectation, ${
          owners.length === 0 ? "none" : owners.length
        } matched ${JSON.stringify({
          rule: finding.rule,
          primary: locationLabel(finding.primary),
          related: finding.related.map(locationLabel),
        })}`,
      );
    }

    for (const expected of testCase.expectedAbsent || []) {
      assertFindingCount(activeFindings, expected, 0, `${testCase.name}: deliberate non-finding`);
    }
    for (const miss of testCase.knownMisses || []) {
      assert.ok(miss.reason, `${testCase.name}: known miss ${miss.id} needs a reason`);
      assert.ok(!knownIds.has(miss.id), `duplicate known-miss id ${miss.id}`);
      knownIds.add(miss.id);
      assertFindingCount(
        activeFindings,
        miss,
        0,
        `${testCase.name}: known miss ${miss.id} was fixed; move it to expectedFindings`,
      );
      knownMisses += 1;
    }
    assert.equal(
      activeFindings.length,
      expectedTotal,
      `${testCase.name}: unclassified active findings`,
    );
    detected += expectedTotal;
  }

  const desired = detected + knownMisses;
  const ratio = desired === 0 ? 1 : detected / desired;
  if (!only) {
    // Recall is a ratio, so deleting an unsatisfied case raises it. Exact equality — matching how
    // `knownMissBaseline` below is asserted — makes the corpus ratchet in one direction only. A
    // `>=` floor would let the corpus grow to 60 and then shed all seven additions while staying
    // green, so additions would never become durable. Raising the floor is one line, and it is the
    // line that records the gain.
    assert.equal(
      cases.length,
      recallManifest.minimumCases,
      `recall corpus has ${cases.length} cases against a floor of ${recallManifest.minimumCases}; `
        + "raise minimumCases when adding a case, and never lower it to drop one",
    );
    assert.equal(
      knownMisses,
      recallManifest.knownMissBaseline,
      "known-miss baseline changed; fixes must move entries to expectedFindings and reduce the baseline",
    );
    assert.ok(
      ratio >= recallManifest.minimumRecall,
      `recall ${(ratio * 100).toFixed(1)}% is below ${(recallManifest.minimumRecall * 100).toFixed(1)}%`,
    );
  }
  return { cases: cases.length, detected, desired, ratio, knownMisses };
}

/**
 * Asserts that exactly `count` findings match an expectation.
 *
 * The same helper serves positives, deliberate non-findings (`count` 0) and known misses, so all
 * three are held to one matching rule and cannot drift apart.
 *
 * @param {object[]} findings Active findings from one case.
 * @param {object} expected Expectation to match against, as written in the manifest.
 * @param {number} count Exact number of findings that must match.
 * @param {string} context Message prefix identifying the case and the kind of expectation.
 */
function assertFindingCount(findings, expected, count, context) {
  assert.equal(countMatches(findings, expected), count, `${context}: ${JSON.stringify(expected)}`);
}

/**
 * Escapes regex metacharacters so a literal string can be embedded in a pattern.
 *
 * @param {string} value Literal text.
 * @returns {string} Text safe to interpolate into a `RegExp`.
 */
function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
