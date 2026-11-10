// Run explicitly against a locally built analyzer; this matrix does not execute fixture modules.
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve, sep } from "node:path";
import test from "node:test";
import { observeBehaviorCase } from "../check/lib/behavior-runner.mjs";
import { parseRecords } from "../check/lib/negative-isolation.mjs";
import { validateAgainstRecords } from "../check/lib/manifest-schema.mjs";
import { renderSeed } from "./lib/corpus-seed.mjs";
import { replayProject } from "./lib/corpus-replay.mjs";

const binary = resolve(process.env.CUKE_DEDUP_BINARY || "target/release/cuke-dedup");

/** Compare complete findings by logical owner and reject missing, additional or truncated evidence. */
function checkObserved(project, { report, fingerprintsText, status, spawnFailure }) {
  assert.equal(spawnFailure, null);
  assert.equal(status, project.expected.exit);
  assert.equal(report.corpus.incomplete, false);
  assert.equal(report.analysis.truncated, false);
  assert.equal(report.corpus.featureFilesParsed, 1);
  assert.equal(report.summary.featureStepsAnalyzed, project.expected.steps.length);
  const records = parseRecords(fingerprintsText);
  validateAgainstRecords({ ...project.metadata, name: project.seed, expectedDefinitions: project.expected.definitions.length }, records);
  /** Resolve one observed location to its unique logical participant. */
  const owner = (location) => {
    const matches = Object.entries(project.locations).filter(([, point]) => point.path === location.path.split(sep).join("/") && point.line === location.line);
    assert.equal(matches.length, 1, JSON.stringify(location));
    return matches[0][0];
  };
  for (const record of records) assert.equal(record.matcher, project.expected.matcher);
  assert.deepEqual(records.map(owner).sort(), [...project.expected.definitions].sort());
  assert.equal(report.findings.length, project.expected.groups.length);
  for (const group of project.expected.groups) {
    const findings = report.findings.filter((item) => item.rule === group.rule);
    assert.equal(findings.length, 1);
    const [finding] = findings;
    assert.equal(finding.relatedLocationsTruncated, false);
    assert.equal(finding.severity, group.severity);
    let members = finding.related.map(owner);
    if (group.step) assert.equal(owner(finding.primary), group.step);
    else members = [owner(finding.primary), ...members];
    assert.deepEqual(members.sort(), [...group.members].sort());
  }
}

for (const operations of [[], ["extract-helper"], ["move-module", "extract-helper", "value=café bleu", "outline"], ["disable-ambiguity"], ["disable-ambiguity", "restore-ambiguity"], ["malformed-config"]]) {
  /** Run the saved input through file discovery, configuration loading and the JSON reporter. */
  test(`concrete analyzer replay: ${operations.join(", ") || "reviewed seed"}`, async (t) => {
    const bundle = renderSeed(operations);
    const root = await replayProject(JSON.parse(JSON.stringify(bundle)));
    t.after(() => rm(root, { recursive: true, force: true }));
    const scratch = await mkdtemp(join(tmpdir(), "cuke-observe-seed-"));
    t.after(() => rm(scratch, { recursive: true, force: true }));
    const { project } = bundle;
    const result = await observeBehaviorCase({
      binary, corpusRoot: dirname(root), scratch, timeout: 5000,
      testCase: { name: project.seed, path: basename(root), arguments: project.arguments },
    });
    if (project.applicability === "invalid") {
      assert.equal(result.spawnFailure, null);
      assert.equal(result.status, project.expected.exit);
      assert.equal(result.report, null);
      assert.match(result.stderr, /failed to parse [^\n]*\.cuke-dedup\.json/);
    } else checkObserved(project, result);
  });
}
