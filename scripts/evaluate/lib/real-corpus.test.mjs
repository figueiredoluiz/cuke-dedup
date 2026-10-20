import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  assertSanitized,
  buildSnapshot,
  checkoutProblem,
  isWithin,
  localChanges,
  reportShapeProblem,
  compareSnapshots,
  formatSnapshot,
  observationProblem,
  renderDrift,
  runSuite,
  validateManifest,
} from "./real-corpus.mjs";

/** A valid suite, overridable. */
const suite = (over = {}) => ({
  name: "owner-repo",
  repo: "https://github.com/owner/repo",
  commit: "a".repeat(40),
  root: ".",
  framework: "playwright-bdd",
  expectedExits: [0, 1],
  ...over,
});

/** A manifest around the given suites. */
const manifest = (...suites) => ({ schemaVersion: 1, suites });

/** Checks each way a manifest can fail to pin a suite, with valid controls on both sides of each. */
test("validateManifest rejects unpinned, unknown and escaping entries", () => {
  const { name: _name, ...nameless } = suite();
  const rejected = [
    ["a branch name", suite({ commit: "main" })],
    ["HEAD", suite({ commit: "HEAD" })],
    ["an abbreviated revision", suite({ commit: "a".repeat(7) })],
    ["an upper-case revision", suite({ commit: "A".repeat(40) })],
    ["a missing revision", suite({ commit: undefined })],
    ["an unknown framework", suite({ framework: "cypress-cucumber" })],
    ["a parent root", suite({ root: "../elsewhere" })],
    ["a nested parent root", suite({ root: "a/../../b" })],
    ["an absolute root", suite({ root: "/etc" })],
    ["an empty root", suite({ root: "" })],
    ["an ssh repo", suite({ repo: "git@github.com:owner/repo.git" })],
    ["a repo with credentials", suite({ repo: "https://user:token@github.com/owner/repo" })],
    ["a repo with a query", suite({ repo: "https://github.com/owner/repo?x=1" })],
    ["a repo with a fragment", suite({ repo: "https://github.com/owner/repo#frag" })],
    ["an upper-case name", suite({ name: "Owner" })],
    ["no name", nameless],
    ["empty exits", suite({ expectedExits: [] })],
    ["string exits", suite({ expectedExits: ["1"] })],
  ];
  for (const [label, entry] of rejected) {
    assert.throws(() => validateManifest(manifest(entry)), /invalid real-corpus manifest/, label);
  }
  assert.throws(() => validateManifest(manifest(suite(), suite())), /duplicate name/);
  assert.throws(() => validateManifest({ schemaVersion: 2, suites: [suite()] }), /schemaVersion/);
  assert.throws(() => validateManifest(manifest()), /nonempty/);
  // Controls: the same shapes pass when pinned, and a nested root and a second framework are accepted.
  const [valid, nested] = validateManifest(manifest(suite(), suite({ name: "other", root: "packages/e2e", framework: "cucumber-js", expectedExits: undefined })));
  assert.deepEqual(valid.expectedExits, [0, 1]);
  assert.equal(nested.root, "packages/e2e");
  assert.deepEqual(nested.expectedExits, [0, 1]);
});

/** Checks modified, untracked and ignored files are listed, and an excluded node_modules directory is not. */
test("localChanges lists modified, untracked and ignored files but not node_modules", () => {
  assert.deepEqual(localChanges(""), []);
  assert.deepEqual(localChanges("!! node_modules/\n!! packages/a/node_modules/\n"), []);
  assert.deepEqual(localChanges("?? node_modules/\n!! node_modules\n?? node_modules\n!! .DS_Store\n!! docs/.DS_Store\n?? \"pkg dir/node_modules/\"\n!! \"a b/.DS_Store\"\n"), []);
  assert.deepEqual(localChanges("?? \"pkg dir/src.ts\"\n"), ["?? \"pkg dir/src.ts\""]);
  for (const line of [" M src/a.ts", "?? new.ts", "!! .features-gen/", "!! dist/", "A  staged.ts", "!! node_modules.bak/", "!! .DS_Store.ts"]) assert.deepEqual(localChanges(`${line}\n`), [line], line);
});

/** Checks containment is by path segment, so a sibling sharing a prefix or a parent is outside. */
test("isWithin accepts the directory and its descendants only", () => {
  assert.equal(isWithin("/a/b", "/a/b"), true);
  assert.equal(isWithin("/a/b", "/a/b/c/d"), true);
  for (const outside of ["/a", "/a/bc", "/a/b/../c", "/x"]) assert.equal(isWithin("/a/b", outside), false, outside);
  assert.equal(isWithin("/a/b", "/a/..hidden"), false);
  assert.equal(isWithin("/a/b", "/a/b/..hidden"), true);
});

/** Checks a checkout backs its suite only when present, at the pin, clean and containing the root. */
test("checkoutProblem fails closed on every way a checkout can differ from the pin", () => {
  const good = { exists: true, repository: true, head: "a".repeat(40), changes: [], rootExists: true };
  assert.equal(checkoutProblem(suite(), good), null);
  assert.match(checkoutProblem(suite(), { ...good, exists: false }), /checkout missing; clone https:\/\/github.com\/owner\/repo/);
  assert.match(checkoutProblem(suite(), { ...good, repository: false }), /not the top of a git checkout/);
  assert.match(checkoutProblem(suite(), { ...good, head: "b".repeat(40) }), /pinned a{40}/);
  assert.match(checkoutProblem(suite(), { ...good, head: null }), /unreadable/);
  assert.match(checkoutProblem(suite(), { ...good, changes: ["?? stray.ts", "!! dist/"] }), /local changes \(\?\? stray\.ts; !! dist\/\)/);
  assert.match(checkoutProblem(suite(), { ...good, rootExists: false }), /root \. is not a directory/);
});

/** A parsed report with no findings, overridable. */
const reportOf = (over = {}) => ({
  summary: { definitionsAnalyzed: 3, featureStepsAnalyzed: 7 },
  corpus: { definitionFiles: 2, featureFiles: 1, incomplete: false },
  analysis: {
    truncated: false,
    candidateComparisonsEvaluated: 4,
    skippedCandidateComparisons: 0,
    truncatedStructuralClasses: 0,
    candidateSources: { matcherBlocking: { evaluated: 4, skipped: 0 } },
  },
  findings: [],
  findingsTruncated: 0,
  ...over,
});

/** A definition record at `path:line` with the given fingerprint. */
const definition = (path, line, fingerprint, framework = "playwright-bdd") => ({
  schemaVersion: "1",
  path,
  line,
  column: 1,
  framework,
  matcher: `SECRET-MATCHER-${line}`,
  definitionFingerprint: fingerprint,
});

/** A finding of `rule` between the given `path:line` places. */
const finding = (rule, [primary, ...related], over = {}) => ({
  rule,
  suppression: null,
  message: "message mentions /Users/someone/secret.ts",
  primary: { path: primary[0], line: primary[1], column: 1 },
  related: related.map(([path, line]) => ({ path, line, column: 1 })),
  relatedLocationsTruncated: false,
  evidence: { comparison: { leftMatcher: "SECRET-MATCHER" } },
  ...over,
});

/** The three definitions every observation below shares. */
const DEFINITIONS = [definition("a.ts", 1, "f1"), definition("a.ts", 9, "f2"), definition("b.ts", 3, "f3")];

/** An observation of a clean run with the given parts. */
const observe = ({ report = reportOf(), definitions = DEFINITIONS, pairs = [], status = 1 } = {}) => ({
  status,
  spawnFailure: null,
  reportText: JSON.stringify(report),
  fingerprintsText: definitions.map((record) => JSON.stringify(record)).join("\n"),
  pairsText: pairs.map((record) => JSON.stringify(record)).join("\n"),
});

/** Checks every run outcome that must not produce a snapshot, with accepted outcomes as controls. */
test("observationProblem refuses runs that certify nothing", () => {
  assert.equal(observationProblem(suite(), observe()), null);
  assert.equal(observationProblem(suite(), observe({ status: 0 })), null);
  assert.match(observationProblem(suite(), observe({ status: 2 })), /exit 2, expected one of 0, 1/);
  assert.equal(observationProblem(suite({ expectedExits: [0, 1, 2] }), observe({ status: 2 })), null);
  assert.match(observationProblem(suite(), observe({ status: null })), /exit null/);
  assert.match(observationProblem(suite(), { ...observe(), spawnFailure: "killed by SIGKILL" }), /did not complete: killed by SIGKILL/);
  // The analyzer's own diagnostics reach the message, trimmed to the last lines.
  const noisy = observationProblem(suite(), { ...observe({ status: 2 }), stderr: "line1\nline2\nline3\nline4\nline5\nline6\nconfig error\n" });
  assert.match(noisy, /config error/);
  assert.ok(!noisy.includes("line1"));
  assert.match(observationProblem(suite(), { ...observe(), reportText: null }), /no JSON report/);
  assert.match(observationProblem(suite(), { ...observe(), fingerprintsText: null }), /no fingerprint file/);
  assert.match(observationProblem(suite(), { ...observe(), pairsText: null }), /no pair file/);
  assert.match(observationProblem(suite(), { ...observe(), reportText: "{" }), /not valid JSON/);
  assert.match(observationProblem(suite(), observe({ report: reportOf({ findingsTruncated: 3 }) })), /3 fewer findings/);
});

/** Checks a report missing or malforming a field the snapshot reads is refused, with the intact report as control. */
test("reportShapeProblem refuses reports whose fields would read as zero", () => {
  assert.equal(reportShapeProblem(reportOf()), null);
  const broken = [
    ["no truncation count", ({ findingsTruncated: _gone, ...rest }) => rest],
    ["a string truncation count", (r) => ({ ...r, findingsTruncated: "0" })],
    ["no findings", ({ findings: _gone, ...rest }) => rest],
    ["a finding without suppression", (r) => ({ ...r, findings: [{ rule: "x", primary: { path: "a", line: 1, column: 1 }, related: [] }] })],
    ["a finding without a primary", (r) => ({ ...r, findings: [{ rule: "x", suppression: null, related: [] }] })],
    ["a finding with a malformed related location", (r) => ({ ...r, findings: [{ rule: "x", suppression: null, primary: { path: "a", line: 1, column: 1 }, related: [{}] }] })],
    ["no summary", ({ summary: _gone, ...rest }) => rest],
    ["no corpus", ({ corpus: _gone, ...rest }) => rest],
    ["a non-boolean incomplete flag", (r) => ({ ...r, corpus: { ...r.corpus, incomplete: 0 } })],
    ["no analysis", ({ analysis: _gone, ...rest }) => rest],
    ["a renamed candidate count", (r) => ({ ...r, analysis: { ...r.analysis, skippedCandidateComparisons: undefined } })],
    ["a source without counts", (r) => ({ ...r, analysis: { ...r.analysis, candidateSources: { matcherBlocking: {} } } })],
  ];
  for (const [label, damage] of broken) assert.match(reportShapeProblem(damage(reportOf())), /lacks or malforms/, label);
  assert.match(observationProblem(suite(), observe({ report: { ...reportOf(), findingsTruncated: undefined } })), /findingsTruncated/);
});

/** Two pair findings and one unused-definition finding over the shared definitions. */
const FINDINGS = [
  finding("duplicate-handler", [["a.ts", 1], ["b.ts", 3]]),
  finding("near-duplicate-step", [["a.ts", 1], ["a.ts", 9]]),
  finding("unused-definition", [["a.ts", 9]]),
];

/** Checks the census, per-rule digests and suppression handling of a snapshot. */
test("buildSnapshot counts findings per rule and leaves suppressed ones out of the digests", () => {
  const suppressed = finding("duplicate-handler", [["a.ts", 9], ["b.ts", 3]], { suppression: { reason: "baseline" } });
  const pairs = [
    { rules: ["duplicate-handler"], candidateSources: ["identicalHandler", "matcherBlocking"] },
    { rules: [], candidateSources: ["matcherBlocking"] },
  ];
  const { snapshot, locations } = buildSnapshot(suite(), observe({ report: reportOf({ findings: [...FINDINGS, suppressed] }), pairs }));
  assert.equal(snapshot.census.definitions, 3);
  assert.deepEqual(snapshot.census.findings, { "duplicate-handler": 1, "near-duplicate-step": 1, "unused-definition": 1 });
  assert.equal(snapshot.census.suppressed, 1);
  assert.deepEqual(snapshot.census.frameworks, { "playwright-bdd": 3 });
  assert.deepEqual(snapshot.census.pairs, {
    evaluated: 2,
    byRule: { "duplicate-handler": 1 },
    bySource: { identicalHandler: 1, matcherBlocking: 2 },
  });
  assert.deepEqual(snapshot.census.candidates, {
    truncated: false,
    evaluated: 4,
    skipped: 0,
    truncatedStructuralClasses: 0,
    sources: { matcherBlocking: { evaluated: 4, skipped: 0 } },
  });
  assert.equal(Object.values(snapshot.findings).flat().length, 3);
  assert.deepEqual([...locations.values()].map((places) => places.length).sort(), [1, 2, 2]);
  assertSanitized(snapshot);
});

/** Checks the suite's framework must be seen in the run, so a suite analyzed as another kind fails closed. */
test("buildSnapshot refuses a run that recognised no definition of the declared framework", () => {
  const other = DEFINITIONS.map((record) => ({ ...record, framework: "cucumber-js" }));
  assert.throws(() => buildSnapshot(suite(), observe({ definitions: other })), /no definition was recognised as playwright-bdd/);
  assert.throws(() => buildSnapshot(suite(), observe({ definitions: [] })), /no definition was recognised/);
  // Controls: the same definitions satisfy a cucumber-js suite, and a mixed run satisfies either.
  assert.equal(buildSnapshot(suite({ framework: "cucumber-js" }), observe({ definitions: other })).snapshot.framework, "cucumber-js");
  const mixed = [...DEFINITIONS, definition("c.ts", 1, "f4", "cucumber-js")];
  assert.equal(buildSnapshot(suite({ framework: "cucumber-js" }), observe({ definitions: mixed })).snapshot.census.frameworks["cucumber-js"], 1);
});

/** Checks a pair finding naming a location with no definition record fails closed, and other rules do not. */
test("buildSnapshot fails closed on a pair finding with a missing reference", () => {
  const dangling = finding("duplicate-handler", [["a.ts", 1], ["gone.ts", 5]]);
  assert.throws(() => buildSnapshot(suite(), observe({ report: reportOf({ findings: [dangling] }) })), /no fingerprint record/);
  // Other rules tolerate it: their locations can be feature steps, which have no definition record.
  const loose = finding("unused-definition", [["gone.ts", 5]]);
  const { snapshot } = buildSnapshot(suite(), observe({ report: reportOf({ findings: [loose] }) }));
  assert.equal(Object.keys(snapshot.findings).length, 1);
});

/** Digests of the one finding in a run over `definitions`. */
const digestOf = (found, definitions = DEFINITIONS) => buildSnapshot(suite(), observe({ report: reportOf({ findings: [found] }), definitions })).snapshot.findings[found.rule][0];

/** Checks `id` follows rule and locations, `fp` follows rule and definition content, and neither follows report order. */
test("finding digests separate location from content and ignore order", () => {
  const base = finding("duplicate-handler", [["a.ts", 1], ["b.ts", 3]]);
  const reference = digestOf(base);
  // The order of related locations is not part of either digest, and the primary location is not part of `fp`.
  assert.deepEqual(digestOf(finding("duplicate-handler", [["a.ts", 1], ["b.ts", 3], ["a.ts", 9]])), digestOf(finding("duplicate-handler", [["a.ts", 1], ["a.ts", 9], ["b.ts", 3]])));
  assert.equal(digestOf(finding("duplicate-handler", [["b.ts", 3], ["a.ts", 1]])).fp, reference.fp);
  const edited = DEFINITIONS.map((record) => (record.line === 3 ? { ...record, definitionFingerprint: "changed" } : record));
  assert.equal(digestOf(base, edited).id, reference.id);
  assert.notEqual(digestOf(base, edited).fp, reference.fp);
  // Which location is primary is part of the id.
  assert.notEqual(digestOf(finding("duplicate-handler", [["b.ts", 3], ["a.ts", 1]])).id, reference.id);
  const moved = finding("duplicate-handler", [["a.ts", 1], ["a.ts", 9]]);
  assert.notEqual(digestOf(moved).id, reference.id);
  assert.notEqual(digestOf(finding("near-duplicate-step", [["a.ts", 1], ["b.ts", 3]])).id, reference.id);
  const truncated = finding("duplicate-handler", [["a.ts", 1], ["b.ts", 3]], { relatedLocationsTruncated: true, evidence: { cluster: { memberCount: 400, definitionFingerprints: ["f1", "f3", "unlisted"] } } });
  assert.notEqual(digestOf(truncated).id, reference.id);
  // The fingerprints of members the report did not list still reach `fp`.
  const unlisted = finding("duplicate-handler", [["a.ts", 1], ["b.ts", 3]], { relatedLocationsTruncated: true, evidence: { cluster: { memberCount: 400, definitionFingerprints: ["f1", "f3", "changed"] } } });
  assert.equal(digestOf(unlisted).id, digestOf(truncated).id);
  assert.notEqual(digestOf(unlisted).fp, digestOf(truncated).fp);
});

/** Checks a path holding a newline cannot alias two locations, since the digest input is encoded. */
test("finding ids do not alias locations joined by a newline", () => {
  const split = finding("unused-definition", [["p\nq", 1], ["z", 2]]);
  const joined = finding("unused-definition", [["p\nq:1:1\nz", 2]]);
  assert.notEqual(digestOf(split).id, digestOf(joined).id);
});

/** Checks the snapshot holds none of the text the report and fingerprint records carry. */
test("buildSnapshot output carries no matcher text, message text or path", () => {
  const { snapshot } = buildSnapshot(suite(), observe({ report: reportOf({ findings: FINDINGS }) }));
  const text = formatSnapshot(snapshot);
  for (const leaked of ["SECRET", "/Users/", "a.ts", "b.ts", "message mentions"]) assert.ok(!text.includes(leaked), leaked);
  // Control: the inputs do contain them, so the assertions above can fail.
  const input = observe({ report: reportOf({ findings: FINDINGS }) });
  assert.ok(input.fingerprintsText.includes("SECRET") && input.reportText.includes("/Users/") && input.reportText.includes("a.ts"));
});

/** A sanitized snapshot to damage. */
const cleanSnapshot = () => buildSnapshot(suite(), observe({ report: reportOf({ findings: FINDINGS }) })).snapshot;

/** Checks each way a snapshot could hold free text or a path is rejected, and a clean one is not. */
test("assertSanitized rejects free text, paths and unknown keys", () => {
  assertSanitized(cleanSnapshot());
  const damaged = [
    ["an extra top-level key", (s) => ({ ...s, note: "x" })],
    ["a missing top-level key", ({ exit: _exit, ...s }) => s],
    ["an absolute root", (s) => ({ ...s, root: "/Users/me/repo" })],
    ["a drive root", (s) => ({ ...s, root: "C:\\repo" })],
    ["a parent root", (s) => ({ ...s, root: "../repo" })],
    ["a repo with credentials", (s) => ({ ...s, repo: "https://user:token@github.com/o/r" })],
    ["a path as the suite name", (s) => ({ ...s, suite: "../etc/passwd" })],
    ["a string exit status", (s) => ({ ...s, exit: "/Users/me" })],
    ["a negative exit status", (s) => ({ ...s, exit: -1 })],
    ["a short revision", (s) => ({ ...s, commit: "abc1234" })],
    ["a non-https repo", (s) => ({ ...s, repo: "/Users/me/repo" })],
    ["an unknown framework", (s) => ({ ...s, framework: "other" })],
    ["a string census value", (s) => ({ ...s, census: { ...s.census, definitions: "a.ts" } })],
    ["a path as a census key", (s) => ({ ...s, census: { ...s.census, frameworks: { "src/steps.ts": 1 } } })],
    ["matcher text as a census key", (s) => ({ ...s, census: { ...s.census, frameworks: { "I have {int} cukes": 1 } } })],
    ["an extra finding field", (s) => ({ ...s, findings: { "duplicate-handler": [{ ...s.findings["duplicate-handler"][0], matcher: "x" }] } })],
    ["a non-hex finding id", (s) => ({ ...s, findings: { "duplicate-handler": [{ id: "a.ts:1", fp: "0".repeat(16) }] } })],
    ["a long digest", (s) => ({ ...s, findings: { "duplicate-handler": [{ id: "0".repeat(32), fp: "0".repeat(16) }] } })],
    ["a non-array rule", (s) => ({ ...s, findings: { "duplicate-handler": "x" } })],
    ["a path as a rule", (s) => ({ ...s, findings: { "a/b.ts": [] } })],
  ];
  for (const [label, damage] of damaged) assert.throws(() => assertSanitized(damage(cleanSnapshot())), /not sanitized/, label);
});

/** Checks the stable text parses back to the snapshot, is idempotent and lists one finding per line. */
test("formatSnapshot round-trips with one finding per line", () => {
  const { snapshot } = buildSnapshot(suite(), observe({ report: reportOf({ findings: FINDINGS }) }));
  const text = formatSnapshot(snapshot);
  assert.deepEqual(JSON.parse(text), snapshot);
  assert.equal(text.split("\n").filter((line) => /^\s+\{"id":/.test(line)).length, 3);
  assert.ok(text.endsWith("}\n"));
  const empty = buildSnapshot(suite(), observe()).snapshot;
  assert.deepEqual(JSON.parse(formatSnapshot(empty)), empty);
  assert.match(formatSnapshot(empty), /\n  "findings": \{\}\n\}\n$/);
});

/** A snapshot with the given findings per rule and census overrides. */
const snap = (findings, census = {}, over = {}) => ({
  ...cleanSnapshot(),
  findings,
  census: { ...cleanSnapshot().census, findings: Object.fromEntries(Object.entries(findings).map(([rule, list]) => [rule, list.length])), ...census },
  ...over,
});

/** A finding digest. */
const entry = (id, fp = "0".repeat(16)) => ({ id: id.padEnd(16, "0"), fp });

/** Checks drift direction and size per rule, with an identical snapshot as the control. */
test("compareSnapshots reports gained, lost and changed findings per rule", () => {
  const base = snap({ "duplicate-handler": [entry("1"), entry("2")], "near-duplicate-step": [entry("3")] });
  assert.deepEqual(compareSnapshots(base, base), { clean: true, census: [], rules: [] });
  const gained = snap({ "duplicate-handler": [entry("1"), entry("2"), entry("4")], "near-duplicate-step": [entry("3")] });
  assert.deepEqual(compareSnapshots(base, gained).rules, [{ rule: "duplicate-handler", committed: 2, fresh: 3, added: 1, removed: 0, changed: 0 }]);
  const lost = snap({ "duplicate-handler": [entry("1")], "near-duplicate-step": [entry("3")] });
  assert.deepEqual(compareSnapshots(base, lost).rules, [{ rule: "duplicate-handler", committed: 2, fresh: 1, added: 0, removed: 1, changed: 0 }]);
  // A swapped finding keeps the count, so only added and removed reveal it.
  const swapped = snap({ "duplicate-handler": [entry("1"), entry("5")], "near-duplicate-step": [entry("3")] });
  assert.deepEqual(compareSnapshots(base, swapped).rules, [{ rule: "duplicate-handler", committed: 2, fresh: 2, added: 1, removed: 1, changed: 0 }]);
  // A repeated id with another multiplicity is one gain, not a gain and a change.
  const doubled = snap({ "duplicate-handler": [entry("1"), entry("1", "e".repeat(16)), entry("2")], "near-duplicate-step": [entry("3")] });
  assert.deepEqual(compareSnapshots(base, doubled).rules, [{ rule: "duplicate-handler", committed: 2, fresh: 3, added: 1, removed: 0, changed: 0 }]);
  // The same location with different definition content is a change, not a gain and a loss.
  const edited = snap({ "duplicate-handler": [entry("1"), entry("2", "f".repeat(16))], "near-duplicate-step": [entry("3")] });
  assert.deepEqual(compareSnapshots(base, edited).rules, [{ rule: "duplicate-handler", committed: 2, fresh: 2, added: 0, removed: 0, changed: 1 }]);
  // A rule present on one side only.
  const vanished = snap({ "duplicate-handler": [entry("1"), entry("2")] });
  assert.deepEqual(compareSnapshots(base, vanished).rules, [{ rule: "near-duplicate-step", committed: 1, fresh: 0, added: 0, removed: 1, changed: 0 }]);
  assert.deepEqual(compareSnapshots(vanished, base).rules, [{ rule: "near-duplicate-step", committed: 0, fresh: 1, added: 1, removed: 0, changed: 0 }]);
});

/** Checks census, exit and revision changes are drift even when no finding changes. */
test("compareSnapshots reports census, exit and revision changes", () => {
  const base = snap({ "duplicate-handler": [entry("1")] });
  const fewer = snap({ "duplicate-handler": [entry("1")] }, { definitions: base.census.definitions - 1 });
  const drift = compareSnapshots(base, fewer);
  assert.equal(drift.clean, false);
  assert.deepEqual(drift.census, [{ field: "definitions", committed: 3, fresh: 2 }]);
  assert.deepEqual(compareSnapshots(base, { ...base, exit: 0 }).census, [{ field: "exit", committed: 1, fresh: 0 }]);
  assert.equal(compareSnapshots(base, { ...base, commit: "b".repeat(40) }).census[0].field, "commit");
  const skipped = snap({ "duplicate-handler": [entry("1")] }, { candidates: { ...base.census.candidates, skipped: 9 } });
  assert.deepEqual(compareSnapshots(base, skipped).census.map(({ field }) => field), ["candidates.skipped"]);
});

/** Checks the rendered drift names the rule and the direction columns, and a clean run says so. */
test("renderDrift tables per-rule drift and says when there is none", () => {
  const base = snap({ "duplicate-handler": [entry("1")] });
  assert.match(renderDrift("s", compareSnapshots(base, base)), /no drift/);
  const text = renderDrift("s", compareSnapshots(base, snap({}, { definitions: 9 })));
  assert.match(text, /\| duplicate-handler \| 1 \| 0 \| 0 \| 1 \| 0 \|/);
  assert.match(text, /- definitions: 3 -> 9/);
});

const COMMITTED = fileURLToPath(new URL("../../../evaluation/real/", import.meta.url));
const BINARY = fileURLToPath(new URL(`../../../target/release/cuke-dedup${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
const SMOKE_ROOT = fileURLToPath(new URL("../../../fixtures/recall/cross-package", import.meta.url));

/** Checks the real binary's report, fingerprint and pair outputs still have the fields the snapshot reads; skipped without a built binary. */
test("a real run of the built binary yields a sanitized snapshot", { skip: !existsSync(BINARY) }, async () => {
  const smoke = suite({ framework: "cucumber-js" });
  const observation = await runSuite({ binary: BINARY, root: SMOKE_ROOT });
  assert.equal(observationProblem(smoke, observation), null);
  const { snapshot } = buildSnapshot(smoke, observation);
  assertSanitized(snapshot);
  assert.equal(snapshot.census.definitions, 2);
  assert.deepEqual(snapshot.census.findings, { "duplicate-matcher": 1 });
  assert.ok(snapshot.census.pairs.evaluated > 0 && snapshot.census.candidates.evaluated > 0);
});

/** Checks every committed snapshot is sanitized, free of absolute paths and pinned like the manifest. */
test("committed snapshots are sanitized and match the manifest pins", () => {
  const suites = validateManifest(JSON.parse(readFileSync(join(COMMITTED, "manifest.json"), "utf8")));
  const directory = join(COMMITTED, "snapshots");
  const files = existsSync(directory) ? readdirSync(directory) : [];
  // Every snapshot belongs to a manifest suite; a suite may not have one yet.
  for (const file of files) assert.ok(suites.some((entry) => `${entry.name}.json` === file), `${file} has no manifest suite`);
  for (const file of files) {
    const text = readFileSync(join(COMMITTED, "snapshots", file), "utf8");
    const parsed = JSON.parse(text);
    assertSanitized(parsed);
    assert.equal(text, formatSnapshot(parsed), `${file} is not in the canonical form`);
    assert.ok(!/(^|[^A-Za-z0-9])(\/Users\/|\/home\/|\/private\/|\/tmp\/|\/var\/|[A-Za-z]:\\)/.test(text), `${file} holds an absolute path`);
    const pinned = suites.find((entry) => entry.name === parsed.suite);
    assert.equal(parsed.commit, pinned.commit);
    assert.equal(parsed.framework, pinned.framework);
  }
});
