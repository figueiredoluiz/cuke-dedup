// Loads every Ruby fixture suite under real Ruby with a recording Cucumber DSL and compares what
// Ruby registered with what the analyzer extracted. For a suite the analyzer reports complete, the
// registered and extracted matchers (kind, text, flags) must be the same multiset, every selected
// file must load, and no step keyword may be replaced. Locations are not compared: Cucumber
// records a handler's own location. Incomplete suites are counted, not judged: the analyzer keeps
// error-recovered definitions for them by contract. Owned control suites run first and must each
// produce their expected answer. Handler blocks never run.
//
// Usage: node scripts/check/check-ruby-execution.mjs [BINARY]
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import { materializeParityLane, materializeRubyGroup, rubyGroupSelection, within } from "./lib/parity-runner.mjs";
import { hiddenWhitelistConflict, ignoreFiles, ignoreLayoutConflict, problems, selectedFiles, undiscovered } from "./lib/ruby-execution.mjs";

const binary = resolve(process.argv[2] ?? "target/debug/cuke-dedup");
const harness = resolve("scripts/check/lib/ruby-load-harness.rb");
const keywords = resolve("src/ruby/step_keywords.rs");
const rubyRoot = resolve("fixtures/ruby-parity");
/** Read a UTF-8 file and parse its JSON. */
const readJson = async (path) => JSON.parse(await readFile(path, "utf8"));

// Suites whose loading depends on a value only runtime decides, with the exact problems they show.
// A changed problem list fails, so a new defect in an exempt suite is still reported.
const exempt = new Map([
  ["ruby-parity/review-gap-computed-key-mutation", [
    'extracted but not registered by Ruby: cucumber-expression "the parcel status is verified" at review-gap-computed-key-mutation/steps.rb:5',
    'extracted but not registered by Ruby: cucumber-expression "the parcel status is now verified" at review-gap-computed-key-mutation/steps.rb:6',
    "review-gap-computed-key-mutation/steps.rb:4 fails to load: NoMethodError: undefined method 'object_containing=' for nil",
  ]],
  ["ruby-parity/equivalent-provider-factory-mutation", [
    'extracted but not registered by Ruby: cucumber-expression "the parcel status is verified" at entry.rb:5',
    'extracted but not registered by Ruby: cucumber-expression "the parcel status is now verified" at entry.rb:8',
    "entry.rb:4 fails to load: NoMethodError: undefined method 'object_containing=' for nil",
  ]],
]);

// Owned controls for the Ruby harness itself, each with what it must observe: a registration at
// its block's location, a load error that stops the file, a replaced keyword, a registration only
// runtime makes, an opaque helper rejected as a pattern, a String mutated after registering, an
// `eval` block located at its call, support files loading before step files and in reverse order,
// an undefined keyword, and a load path. Comparison rules are
// covered by lib/ruby-execution.test.mjs.
const controls = [
  {
    name: "control/registration",
    files: { "steps.rb": "Given('a step') { work() }\nThen(/\\Aother\\z/i) { work() }\n" },
    paths: [],
    expected: { registrations: ["steps.rb:1 cucumber-expression a step", "steps.rb:2 regular-expression \\Aother\\z/i"], loadErrors: [], replaced: [] },
  },
  {
    name: "control/load-error",
    files: { "steps.rb": "Given('a step') { work() }\nraise 'stop'\nThen('never') { work() }\n" },
    paths: [],
    expected: { registrations: ["steps.rb:1 cucumber-expression a step"], loadErrors: ["steps.rb:2 RuntimeError"], replaced: [] },
  },
  {
    name: "control/replaced-keyword",
    files: { "steps.rb": "Cucumber::Glue::Dsl.send(:define_method, :Then) { |*| nil }\nThen('hidden') { work() }\n" },
    paths: [],
    expected: { registrations: [], loadErrors: [], replaced: ["Then"] },
  },
  {
    name: "control/runtime-registration",
    files: { "steps.rb": "['dynamic'].each { |text| Then(text) { work() } }\n" },
    paths: [],
    expected: { registrations: ["steps.rb:1 cucumber-expression dynamic"], loadErrors: [], replaced: [] },
  },
  {
    name: "control/opaque-pattern",
    files: { "steps.rb": "Given(helper_pattern) { work() }\n" },
    paths: [],
    expected: { registrations: [], loadErrors: ["steps.rb:1 ArgumentError"], replaced: [] },
  },
  {
    name: "control/mutated-pattern",
    files: { "steps.rb": "pattern = +'first'\nGiven(pattern) { work() }\npattern.replace('second')\n" },
    paths: [],
    expected: { registrations: ["steps.rb:2 cucumber-expression first"], loadErrors: [], replaced: [] },
  },
  {
    name: "control/eval-block",
    files: { "steps.rb": "handler = eval('proc { work() }')\nGiven('evaluated', &handler)\n" },
    paths: [],
    expected: { registrations: ["steps.rb:2 cucumber-expression evaluated"], loadErrors: [], replaced: [] },
  },
  {
    // Only `support/env.rb` loads first; a step file named `env.rb` waits for support files.
    name: "control/load-order",
    files: { "step_definitions/env.rb": "Given(HELPER) { work() }\n", "support/helper.rb": "HELPER = 'from support'\n" },
    load: ["step_definitions/env.rb", "support/helper.rb"],
    paths: [],
    expected: { registrations: ["step_definitions/env.rb:1 cucumber-expression from support"], loadErrors: [], replaced: [] },
  },
  {
    name: "control/undefined-keyword",
    files: { "steps.rb": "Cucumber::Glue::Dsl.send(:undef_method, :When)\n" },
    paths: [],
    expected: { registrations: [], loadErrors: [], replaced: ["When"] },
  },
  {
    // Cucumber-Ruby loads `support/env.*` first, then other support files in reverse path order.
    name: "control/support-order",
    files: { "support/a.rb": "Given(SHARED) { work() }\n", "support/b.rb": "SHARED = LOCAL + ' b'\n", "support/env.local.rb": "LOCAL = 'env'\n" },
    load: ["support/a.rb", "support/b.rb", "support/env.local.rb"],
    paths: [],
    expected: { registrations: ["support/a.rb:1 cucumber-expression env b"], loadErrors: [], replaced: [] },
  },
  {
    name: "control/load-path",
    files: { "steps.rb": "require 'shared'\n", "lib/shared.rb": "Given('shared step') { work() }\n" },
    paths: ["lib"],
    expected: { registrations: ["lib/shared.rb:1 cucumber-expression shared step"], loadErrors: [], replaced: [] },
  },
];

// Owned controls for file selection, judged end to end: each must report no problem. A raising
// file that selection must leave out makes a broken rule fail as a load error and as
// `selected but not discovered`; dropping `steps.rb` fails as an unregistered definition.
const step = "Given('a step') { work() }\n";
const selectionControls = [
  { name: "selection/ignore-file", selection: ["--definitions", "*.rb"],
    files: { ".gitignore": "broken.rb\n", "broken.rb": "raise 'loaded'\n", "steps.rb": step } },
  // The `ignore` crate matches case-sensitively; git on macOS defaults `core.ignoreCase` on.
  { name: "selection/ignore-case", selection: ["--definitions", "*.rb"],
    files: { ".gitignore": "STEPS.rb\n", "steps.rb": step } },
  // Discovery tests feature globs first, so a `.rb` file they match is not a definition. The file
  // is parsed as Gherkin, so it holds a feature; selecting it reports it undiscovered.
  { name: "selection/feature-glob", selection: ["--features", "pages/**", "--definitions", "**/*.rb"],
    files: { "pages/helper.rb": "Feature: claimed by the feature glob\n", "steps.rb": step } },
  // The walk never enters `.hidden`, so its `.gitignore` does not conflict with the root's
  // higher-ranked `.cuke-dedupignore`.
  { name: "selection/pruned-ignore-file", selection: ["--definitions", "*.rb"],
    files: { ".cuke-dedupignore": "broken.rb\n", "broken.rb": "raise 'loaded'\n", ".hidden/.gitignore": "steps.rb\n", "steps.rb": step } },
];

/** A harness result reduced to comparable strings. */
const summarize = (ruby) => ({
  registrations: ruby.registrations.map((item) => `${item.path}:${item.line} ${item.matcherKind} ${item.matcher}${item.matcherFlags ? `/${item.matcherFlags}` : ""}`),
  loadErrors: ruby.loadErrors.map((error) => `${error.path}:${error.line} ${error.error}`),
  replaced: ruby.replaced,
});

/** Every Ruby suite: the ruby-parity groups and the Ruby lane of each paired language case. */
async function suites(scratch) {
  const manifest = await readJson(join(rubyRoot, "manifest.json"));
  const groups = { ...manifest.groupOracles };
  for (const file of manifest.groupManifests ?? []) Object.assign(groups, (await readJson(within(rubyRoot, file))).groupOracles);
  const result = Object.entries(groups).map(([name, oracle]) => ({
    name: `ruby-parity/${name}`,
    prepare: async () => ({
      project: await materializeRubyGroup({ root: rubyRoot, scratch: join(scratch, name), oracle }),
      selection: rubyGroupSelection(oracle),
      expectsIncomplete: oracle.complete === false,
    }),
  }));
  for (const item of (await readJson("fixtures/language-parity/manifest.json")).cases) {
    const directory = join(scratch, "language-parity", item.id);
    result.push({
      name: `language-parity/${item.id}`,
      prepare: async () => ({ project: directory, selection: await materializeParityLane({ directory, item, language: "ruby", extension: "rb" }) }),
    });
  }
  return result;
}

/**
 * Analyzer view: completeness, extracted registrations and the files discovery listed. Exit 2 is
 * accepted only for a suite whose oracle expects incompleteness and whose report says so.
 */
async function analyze(project, selection, expectsIncomplete) {
  const run = spawnSync(binary, [".", ...selection, "--reporters", "jsonl", "--no-metrics", "--explain-discovery",
    "--emit-fingerprints", "fingerprints.jsonl"], { cwd: project, encoding: "utf8", timeout: 30_000, maxBuffer: 16 * 1024 * 1024 });
  if (run.error) throw new Error(`cannot run the analyzer: ${run.error.message}`);
  const rows = run.stdout.split("\n").filter((line) => line.startsWith("{")).map((line) => JSON.parse(line));
  const summary = rows.find((row) => row.type === "summary");
  if (run.status === 2 && expectsIncomplete && summary?.corpus.incomplete) return { complete: false };
  if (run.error || run.signal || ![0, 1].includes(run.status)) {
    throw new Error(`analyzer failed (${run.error?.message ?? run.signal ?? `exit ${run.status}`}): ${run.stderr.trim().split("\n").at(-1)}`);
  }
  if (!summary) throw new Error("analyzer report has no summary record");
  const listed = run.stderr.split("\n").flatMap((line) => line.match(/^cuke-dedup: definition (.+)$/)?.[1] ?? [])
    // Discovery prints native separators; selected paths use `/`.
    .map((path) => path.split(sep).join("/"));
  const registrations = (await readFile(join(project, "fingerprints.jsonl"), "utf8")).split("\n").filter(Boolean)
    .map((line) => JSON.parse(line))
    .map(({ path, line, matcherKind, matcher, matcherFlags }) =>
      ({ path, line, matcherKind, matcher, matcherFlags: [...matcherFlags].filter((flag) => flag !== "u").sort().join("") }));
  return { complete: !summary.corpus.incomplete, listed, registrations };
}

/** The suite's merged configuration, as `--print-config` reports it. */
function effectiveConfig(project, selection) {
  const run = spawnSync(binary, [".", ...selection, "--print-config"], { cwd: project, encoding: "utf8", timeout: 30_000 });
  if (run.status !== 0) throw new Error(`analyzer --print-config failed: ${run.stderr.trim()}`);
  return JSON.parse(run.stdout);
}

/** Ruby view: registrations, load errors and replaced keywords. A crash or timeout is a load failure. */
async function execute(project, files, paths) {
  // RUBYOPT can select parse.y and RUBYLIB adds load paths; the suite's own paths are passed.
  const { RUBYOPT: _options, RUBYLIB: _library, ...env } = process.env;
  const output = join(project, ".ruby-execution.json");
  const run = spawnSync("ruby", ["--disable-gems", "--parser=prism", "-W0", harness, output, project, keywords, JSON.stringify(paths), ...files],
    { cwd: project, env, encoding: "utf8", timeout: 30_000, maxBuffer: 16 * 1024 * 1024 });
  if (run.error?.code === "ENOENT") throw new Error(`cannot run ruby: ${run.error.message}`);
  if (run.error || run.status !== 0) {
    const error = run.error ? run.error.code ?? run.error.message : run.signal ? `crashed with ${run.signal}` : `exited with ${run.status}`;
    return { registrations: [], loadErrors: [{ path: "(suite)", line: null, error, message: run.stderr.trim().split("\n")[0] ?? "" }], replaced: [] };
  }
  return readJson(output);
}

/**
 * The candidates that ignore files exclude, asked of git. Discovery reads `.gitignore`, `.ignore`
 * and `.cuke-dedupignore` in each directory it enters, later files taking precedence, plus the
 * user's global git excludes, so a mirror holds one `.gitignore` per directory with their rules in
 * that order and keeps the global excludes. Matching stays case-sensitive, as in the `ignore`
 * crate, also where git defaults `core.ignoreCase` on. A layout whose cross-directory precedence
 * the mirror cannot reproduce throws.
 * @param {string} project The project directory.
 * @param {{path: string, rank: number, directory: string}[]} files From `ignoreFiles`.
 * @param {string[]} candidates Files to test.
 * @returns {Promise<Set<string>>} The ignored candidates.
 */
async function ignoredByFiles(project, files, candidates) {
  const conflict = ignoreLayoutConflict(files);
  if (conflict) throw new Error(`unsupported ignore-file layout in ${project}: ${conflict}`);
  if (candidates.length === 0) return new Set();
  const mirror = await mkdtemp(join(tmpdir(), "cuke-ruby-ignore-"));
  try {
    const rules = new Map();
    for (const { path, rank, directory } of files) {
      rules.set(directory, [...(rules.get(directory) ?? []), [rank, await readFile(join(project, path), "utf8")]]);
    }
    for (const [directory, sources] of rules) {
      await mkdir(join(mirror, directory), { recursive: true });
      await writeFile(join(mirror, directory, ".gitignore"), sources.sort(([a], [b]) => a - b).map(([, text]) => `${text}\n`).join(""));
    }
    for (const candidate of candidates) {
      await mkdir(dirname(join(mirror, candidate)), { recursive: true });
      await writeFile(join(mirror, candidate), "");
    }
    // Inherited repository variables (a pre-push hook sets GIT_DIR) would redirect git elsewhere.
    const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_")));
    /** Runs git in the mirror, outside the caller's repository. */
    const git = (args, input) => spawnSync("git", ["-c", "core.ignoreCase=false", ...args], { cwd: mirror, env, input, encoding: "utf8" });
    // No template: its `info/exclude` is not a file discovery reads.
    if (git(["init", "-q", "--template="]).status !== 0) throw new Error("git init failed");
    const run = git(["check-ignore", "--no-index", "-z", "--stdin"], candidates.join("\0"));
    if (![0, 1].includes(run.status)) throw new Error(`git check-ignore failed: ${run.stderr.trim()}`);
    return new Set(run.stdout.split("\0").filter(Boolean));
  } finally {
    await rm(mirror, { recursive: true, force: true });
  }
}

/** Every regular file under `directory`, root-relative and `/`-separated; links are not followed. */
async function listFiles(root, directory = "") {
  const entries = await readdir(join(root, directory), { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const path = directory ? `${directory}/${entry.name}` : entry.name;
    if (entry.isDirectory()) return listFiles(root, path);
    return entry.isFile() ? [path] : [];
  }));
  return nested.flat();
}

/**
 * Problems for one prepared suite, or `null` when the analyzer reports it incomplete. The files
 * Ruby loads come from the effective configuration and ignore files, not from discovery, and a
 * selected file that discovery did not list is a problem of its own.
 */
async function judge({ project, selection, expectsIncomplete }) {
  const analyzer = await analyze(project, selection, expectsIncomplete);
  if (!analyzer.complete) return null;
  const config = effectiveConfig(project, selection);
  const paths = await listFiles(project);
  const read = ignoreFiles(paths, config);
  const whitelist = hiddenWhitelistConflict(paths, await Promise.all(read.map(({ path }) => readFile(join(project, path), "utf8"))), config);
  if (whitelist) throw new Error(`unsupported ignore rules in ${project}: ${whitelist}`);
  const configured = selectedFiles(paths, config);
  const ignored = await ignoredByFiles(project, read, configured);
  const files = configured.filter((file) => !ignored.has(file));
  return [...undiscovered(files, analyzer.listed), ...problems(analyzer, await execute(project, files, config.rubyLoadPaths ?? []))];
}

const scratch = await mkdtemp(join(tmpdir(), "cuke-ruby-execution-"));
const failures = [];
const counts = { matched: 0, exempt: 0, incomplete: 0 };
try {
  for (const control of controls) {
    const project = join(scratch, control.name);
    for (const [file, source] of Object.entries(control.files)) {
      await mkdir(dirname(join(project, file)), { recursive: true });
      await writeFile(join(project, file), source);
    }
    const observed = summarize(await execute(project, control.load ?? ["steps.rb"], control.paths));
    try {
      assert.deepEqual(observed, control.expected);
    } catch {
      failures.push(`${control.name}: expected ${JSON.stringify(control.expected)}, observed ${JSON.stringify(observed)}`);
    }
  }
  for (const control of selectionControls) {
    const project = join(scratch, control.name);
    for (const [file, source] of Object.entries(control.files)) {
      await mkdir(dirname(join(project, file)), { recursive: true });
      await writeFile(join(project, file), source);
    }
    const observed = await judge({ project, selection: control.selection }).catch((error) => [`error: ${error.message}`]);
    if (observed?.length !== 0) failures.push(`${control.name}: expected no problems, observed ${JSON.stringify(observed)}`);
  }
  const seen = new Set();
  for (const suite of await suites(scratch)) {
    const observed = await suite.prepare().then(judge).catch((error) => [`error: ${error.message}`]);
    if (observed === null) {
      counts.incomplete += 1;
    } else if (exempt.has(suite.name)) {
      seen.add(suite.name);
      counts.exempt += 1;
      try {
        assert.deepEqual(observed, exempt.get(suite.name));
      } catch {
        failures.push(`${suite.name} (exempt): problems changed\n  ${observed.join("\n  ") || "(none: remove the exemption)"}`);
      }
    } else if (observed.length > 0) {
      failures.push(`${suite.name} (reported complete)\n  ${observed.join("\n  ")}`);
    } else {
      counts.matched += 1;
    }
  }
  for (const name of exempt.keys()) if (!seen.has(name)) failures.push(`${name}: exempt entry names no complete suite`);
  if (counts.matched === 0) failures.push("no complete Ruby suite was judged");
} finally {
  await rm(scratch, { recursive: true, force: true });
}
for (const failure of failures) console.error(failure);
console.log(`check-ruby-execution: ${counts.matched} complete Ruby suites match Ruby, ${failures.length} failures, ` +
  `${counts.exempt} exempt, ${counts.incomplete} incomplete not judged; ${controls.length + selectionControls.length} controls`);
process.exit(failures.length === 0 ? 0 : 1);
