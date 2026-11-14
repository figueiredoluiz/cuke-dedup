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
import { readFileSync } from "node:fs";
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { homedir, tmpdir, userInfo } from "node:os";
import { basename, dirname, isAbsolute, join, resolve, sep } from "node:path";
import { materializeParityLane, materializeRubyGroup, rubyGroupSelection, within } from "./lib/parity-runner.mjs";
import { globalExcludesPath, hiddenWhitelistConflict, ignoreFiles, ignoreLayoutConflict, problems, ruleText, selectedFiles, undiscovered } from "./lib/ruby-execution.mjs";

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

// Owned controls for file selection, judged end to end: each must report no problem, or fail with
// `error` (kind and reason) when selection cannot be computed. A raising
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
  // Discovery reads `core.excludesFile` from `GIT_CONFIG_GLOBAL`. `broken.rb` registers a step
  // before raising, so the analyzer listing it fails as a registration mismatch and Ruby loading it
  // fails as a load error.
  { name: "selection/global-excludes", selection: ["--definitions", "*.rb"], globalExcludes: "broken.rb\n",
    files: { "broken.rb": "Given('a broken step') { work() }\nraise 'loaded'\n", "steps.rb": step } },
  // A global negation lets discovery take a hidden file that selection prunes. The hidden file
  // registers a step, so leaving the global rules out of the conflict check reports it as
  // extracted but not registered instead of this error.
  { name: "selection/global-hidden-whitelist", selection: ["--definitions", "*.rb"], globalExcludes: "!.hidden.rb\n",
    error: ["ignore rules", "negation rule with hidden file .hidden.rb"],
    files: { ".hidden.rb": "Given('a hidden step') { work() }\n", "steps.rb": step } },
  // The configured excludes file is missing, so the crate applies no global rules. Git, left
  // without `core.excludesFile`, would read the XDG default, which ignores `steps.rb`.
  { name: "selection/missing-global-excludes", selection: ["--definitions", "*.rb"], globalExcludes: null, xdgIgnore: "steps.rb\n",
    files: { "steps.rb": step } },
  // The crate removes each ignore file's leading BOM; git removes one only at the start of the
  // file it reads, so a BOM inside the combined mirror file would turn `broken.rb` into a
  // different pattern.
  { name: "selection/bom-per-file", selection: ["--definitions", "*.rb"],
    files: { ".gitignore": "# first\n", ".ignore": "\uFEFFbroken.rb\n", "broken.rb": "raise 'loaded'\n", "steps.rb": step } },
  // Discovery does not enter a directory its rules ignore, so `vendor/.gitignore` is not read and
  // cannot conflict with the root's higher-ranked `.cuke-dedupignore`. `vendor/broken.rb` raises,
  // so a mirror that let the deeper file un-ignore it fails as a load error.
  { name: "selection/ignored-directory", selection: ["--definitions", "**/*.rb"],
    files: { ".cuke-dedupignore": "vendor/\n", "vendor/.gitignore": "!broken.rb\n", "vendor/broken.rb": "raise 'loaded'\n", "steps.rb": step } },
  // Control: the same layout with `vendor` entered is a precedence the mirror cannot reproduce.
  { name: "selection/entered-directory-conflict", selection: ["--definitions", "**/*.rb"],
    error: ["ignore-file layout", ".cuke-dedupignore over vendor/.gitignore"],
    files: { ".cuke-dedupignore": "other/\n", "vendor/.gitignore": "x.rb\n", "vendor/steps.rb": step } },
  // A directory's own rules do not decide whether the walk enters it: `*` here ignores the
  // directory's contents, not the directory, so the negation over `.hidden.rb` is read.
  { name: "selection/self-ignoring-directory", selection: ["--definitions", "**/*.rb"],
    error: ["ignore rules", "negation rule with hidden file vendor/.hidden.rb"],
    files: { "vendor/.gitignore": "*\n!steps.rb\n!.hidden.rb\n", "vendor/.hidden.rb": "Given('a hidden step') { work() }\n", "vendor/steps.rb": step } },
  // Names with line breaks cannot be read back from discovery's line-based report.
  { name: "selection/line-break-name", selection: ["--definitions", "*.rb"],
    error: ["file name", '"helper\\nx.rb"'],
    files: { "helper\nx.rb": "", "steps.rb": step } },
  // A BOM before a global negation still makes it a negation.
  { name: "selection/bom-negation", selection: ["--definitions", "*.rb"], globalExcludes: "\uFEFF!.hidden.rb\n",
    error: ["ignore rules", "negation rule with hidden file .hidden.rb"],
    files: { ".hidden.rb": "Given('a hidden step') { work() }\n", "steps.rb": step } },
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
 * Ignore files applied through git. Discovery reads `.gitignore`, `.ignore` and
 * `.cuke-dedupignore` in each directory it enters, later files taking precedence, plus the user's
 * global git excludes, so a mirror holds one `.gitignore` per directory with their rules in that
 * order and keeps the global excludes. Matching stays case-sensitive, as in the `ignore` crate,
 * also where git defaults `core.ignoreCase` on. A directory the rules ignore is not entered, so its
 * ignore files are dropped: their rules reach only paths under it, which git reports ignored
 * already. A layout among the entered files whose cross-directory precedence the mirror cannot
 * reproduce throws.
 * @param {string} project The project directory.
 * @param {{path: string, rank: number, directory: string, text: Buffer}[]} files From `ignoreFiles`, with each `ruleText`.
 * @param {string[]} candidates Files to test.
 * @param {Buffer | undefined} excludes The global excludes file's bytes, from `globalExcludes`.
 * @returns {Promise<{entered: typeof files, ignored: Set<string>}>} The ignore files discovery
 *   reads and the ignored candidates.
 */
async function applyIgnoreFiles(project, files, candidates, excludes) {
  const mirror = await mkdtemp(join(tmpdir(), "cuke-ruby-ignore-"));
  try {
    const rules = new Map();
    for (const { rank, directory, text } of files) rules.set(directory, [...(rules.get(directory) ?? []), [rank, text]]);
    for (const [directory, sources] of rules) {
      await mkdir(join(mirror, directory), { recursive: true });
      await writeFile(join(mirror, directory, ".gitignore"), Buffer.concat(sources.sort(([a], [b]) => a - b).flatMap(([, text]) => [text, Buffer.from("\n")])));
    }
    for (const candidate of candidates) {
      await mkdir(dirname(join(mirror, candidate)), { recursive: true });
      await writeFile(join(mirror, candidate), "");
    }
    // Inherited variables (a pre-push hook sets GIT_DIR) would redirect git. Git resolves
    // `core.excludesFile` differently from the analyzer's `ignore` crate, so its own config is
    // replaced by a copy of the file the crate resolves. The copy is always set, empty when there is
    // none, since an unset `core.excludesFile` makes git read its XDG default.
    const empty = join(mirror, ".empty-gitconfig");
    await writeFile(empty, "");
    const global = join(mirror, ".global-excludes");
    await writeFile(global, excludes ?? "");
    const env = { ...Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_"))), GIT_CONFIG_GLOBAL: empty, GIT_CONFIG_NOSYSTEM: "1" };
    const config = ["-c", "core.ignoreCase=false", "-c", `core.excludesFile=${global}`];
    /** Runs git in the mirror, outside the caller's repository; a launch error, signal or status outside `ok` throws. */
    const git = (args, input, ok = [0]) => {
      const run = spawnSync("git", [...config, ...args], { cwd: mirror, env, input, encoding: "utf8" });
      if (run.error) throw new Error(`cannot run git: ${run.error.message}`);
      if (run.signal || !ok.includes(run.status)) throw new Error(`git ${args[0]} failed (${run.signal ?? run.status}): ${run.stderr.trim()}`);
      return run.stdout;
    };
    /** The paths git's rules ignore; a path under an ignored directory counts. */
    const ignored = (paths) => new Set(paths.length === 0 ? [] : git(["check-ignore", "--no-index", "-z", "--stdin"], paths.join("\0"), [0, 1]).split("\0").filter(Boolean));
    // No template: its `info/exclude` is not a file discovery reads.
    git(["init", "-q", "--template="]);
    // Probed without the trailing `/`: git then decides from the parent's rules, as the walk does
    // before entering, and a `dir/` form would also apply the directory's own `.gitignore`.
    const pruned = ignored([...new Set(files.map(({ directory }) => directory.slice(0, -1)).filter(Boolean))]);
    const entered = files.filter(({ directory }) => !pruned.has(directory.slice(0, -1)));
    const conflict = ignoreLayoutConflict(entered);
    if (conflict) throw new Error(`unsupported ignore-file layout in ${project}: ${conflict}`);
    return { entered, ignored: ignored(candidates) };
  } finally {
    await rm(mirror, { recursive: true, force: true });
  }
}

/** `path` from the analyzer's working directory; `..` is left to the OS, as symlinks need. */
const anchor = (project, path) => (isAbsolute(path) ? path : `${project}${sep}${path}`);

/** The file at `path` from the analyzer's working directory, or `undefined` when unreadable. */
function readAnchored(project, path) {
  try {
    return readFileSync(anchor(project, path));
  } catch {
    return undefined;
  }
}

/**
 * The bytes of the global excludes file discovery applies in `project`, or `undefined` when there
 * is none or it is unreadable, which leaves no rules, as in the crate. Rust's `home_dir` reads an
 * empty HOME as unset and asks the password database.
 * @param {string} project The project directory, the analyzer's working directory.
 * @returns {Buffer | undefined} The file's `ruleText`.
 */
function globalExcludes(project) {
  /** The file at `candidate` from the analyzer's working directory. */
  const read = (candidate) => readAnchored(project, candidate);
  const path = globalExcludesPath({ env: process.env, home: homedir() || userInfo().homedir, read });
  const bytes = path ? read(path) : undefined;
  return bytes && ruleText(bytes);
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
  // Discovery reports definitions one per stderr line, so a name holding a line break cannot be
  // compared; globset would still select it.
  const unlisted = paths.find((path) => /[\n\r]/.test(path));
  if (unlisted) throw new Error(`unsupported file name in ${project}: ${JSON.stringify(unlisted)}`);
  const read = ignoreFiles(paths, config);
  // Every rule source discovery reads, each read once: the project's ignore files and the global
  // excludes file.
  const texts = await Promise.all(read.map(async ({ path }) => ruleText(await readFile(join(project, path)))));
  const excludes = globalExcludes(project);
  const configured = selectedFiles(paths, config);
  const { entered, ignored } = await applyIgnoreFiles(project, read.map((file, index) => ({ ...file, text: texts[index] })), configured, excludes);
  const whitelist = hiddenWhitelistConflict(paths, [...entered.map(({ text }) => text), ...(excludes ? [excludes] : [])].map((text) => text.toString("utf8")), config);
  if (whitelist) throw new Error(`unsupported ignore rules in ${project}: ${whitelist}`);
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
    const saved = { GIT_CONFIG_GLOBAL: process.env.GIT_CONFIG_GLOBAL, XDG_CONFIG_HOME: process.env.XDG_CONFIG_HOME };
    if (control.globalExcludes !== undefined) {
      // The crate rejects an `excludesfile` value with whitespace, which a temporary directory may
      // hold, so the value is relative to the project, the analyzer's working directory. `null`
      // names a file that does not exist.
      const excludes = `${project}.excludes`;
      if (control.globalExcludes !== null) await writeFile(excludes, control.globalExcludes);
      await writeFile(`${excludes}.gitconfig`, `[core]\n\texcludesFile = ../${basename(excludes)}\n`);
      process.env.GIT_CONFIG_GLOBAL = `${excludes}.gitconfig`;
    }
    if (control.xdgIgnore !== undefined) {
      await mkdir(`${project}.xdg/git`, { recursive: true });
      await writeFile(`${project}.xdg/git/ignore`, control.xdgIgnore);
      process.env.XDG_CONFIG_HOME = `${project}.xdg`;
    }
    const observed = await judge({ project, selection: control.selection }).catch((error) => [`error: ${error.message}`]);
    for (const [name, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
    const expected = control.error ? [`error: unsupported ${control.error[0]} in ${project}: ${control.error[1]}`] : [];
    try {
      assert.deepEqual(observed, expected);
    } catch {
      failures.push(`${control.name}: expected ${JSON.stringify(expected)}, observed ${JSON.stringify(observed)}`);
    }
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
