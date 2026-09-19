// Memory ratchet for matcher analysis.
//
// Governs *marginal bytes per definition* rather than peak RSS. Peak RSS mixes a fixed startup
// cost with the per-definition cost, so it moves when either does and cannot say which; the
// marginal figure isolates the cost that scales with corpus size, which is the one that decides
// whether a large suite is analyzable at all.
//
// Two sizes are measured and the slope between them is checked. That also catches a return to
// superlinear growth, which a single-size ceiling would miss entirely.
//
// The corpus deliberately mixes plain literals with typed Cucumber-Expression placeholders.
// Placeholders compile to substantially larger programs, and a literal-only corpus understates the
// very cost this gate exists to govern.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const binary = resolve(process.argv[2] || "target/release/cuke-dedup");

/// Marginal bytes per definition. **Ratchet: lower it when a change improves the figure, never
/// raise it to make a regression pass.**
///
/// Measured 11.2-11.7 KB across four consecutive local runs (a 4.5% spread), so the budget carries
/// roughly 20% headroom: tight enough to catch the order-of-magnitude regression this gate exists
/// for, loose enough not to fail on measurement noise. Before v0.6.0 the same corpus cost several
/// times this.
///
/// If CI reports a materially different figure, re-pin from CI rather than from a local run —
/// peak RSS is platform-dependent and macOS reports an upper bound, since libmalloc does not
/// eagerly return freed transients.
const BUDGET_BYTES_PER_DEFINITION = Number.parseInt(
  process.env.CUKE_DEDUP_MEMORY_BUDGET || "14000",
  10,
);
const SIZES = [1_000, 4_000];
const PLACEHOLDERS = ["{int}", "{word}", "{string}", "{float}"];
const NOUNS = ["gauge", "valve", "dial", "pump", "lamp", "rotor", "brake", "clamp", "shaft", "gear",
  "piston", "bearing", "seal", "spring", "cable", "latch", "hinge", "rod", "wheel", "chain"];
const VERBS = ["opens", "turns", "reads", "runs", "glows", "spins", "holds", "grips", "drives",
  "meshes", "strokes", "rolls", "closes", "loads", "tightens", "seats", "swings", "extends"];
/// Deliberate ambiguities, planted at a fixed count so the finding total does not scale with the
/// corpus. A finding count that grew with N would make this gate measure report storage instead of
/// matcher analysis, which is the thing being governed.
const PLANTED_OVERLAPS = 10;

/// Index-derived so the corpus is identical on every run and every machine. Vocabulary is varied so
/// unrelated definitions do not read as near-duplicates of each other; only the planted pairs
/// overlap.
function source(count, offset) {
  const lines = ['import { Given, When, Then } from "@cucumber/cucumber";', ""];
  for (let index = 0; index < count; index += 1) {
    const n = offset + index;
    const keyword = ["Given", "When", "Then"][n % 3];
    const noun = NOUNS[n % NOUNS.length];
    const verb = VERBS[(n * 7) % VERBS.length];
    // Roughly 40% carry a typed placeholder, the rest are plain literals.
    // Every token carries the index. Matcher blocking is shingle-based, so shared wording would
    // generate hundreds of thousands of pair candidates and truncate the run — work this gate does
    // not measure and which would make the two sizes incomparable.
    const matcher = n % 5 < 2
      ? `${noun}${n} ${verb}${n} at ${PLACEHOLDERS[n % PLACEHOLDERS.length]}`
      : `${noun}${n} ${verb}${n} at rest${n}`;
    // Each handler calls a uniquely named function, so no two are structurally comparable. Pair
    // candidate generation runs regardless of rule severity, and structurally similar bodies would
    // generate hundreds of thousands of candidates — work unrelated to matcher analysis that
    // truncates the run and makes the two sizes incomparable.
    lines.push(`${keyword}('${matcher}', () => action_${n}());`);
    lines.push("");
    // A regex twin for the first few definitions: each yields one overlap the compiled matchers
    // must be scanned to find, so the pass cannot pass this gate by skipping compilation.
    if (n < PLANTED_OVERLAPS) {
      lines.push(`${keyword}(/^${noun}${n} ${verb}${n} at .+$/, () => twin_${n}());`);
      lines.push("");
    }
  }
  return lines.join("\n");
}

async function buildCorpus(root, definitions) {
  await mkdir(join(root, "steps"), { recursive: true });
  await mkdir(join(root, "features"), { recursive: true });
  const perFile = 250;
  for (let written = 0; written < definitions; written += perFile) {
    await writeFile(
      join(root, "steps", `steps_${written / perFile}.ts`),
      source(Math.min(perFile, definitions - written), written),
    );
  }
  // One concrete step, so feature-usage analysis runs rather than short-circuiting.
  await writeFile(
    join(root, "features", "suite.feature"),
    "Feature: Ratchet\n  Scenario: Corpus\n    Given the gauge 2 reads steady\n",
  );
  await writeFile(
    join(root, ".cuke-dedup.json"),
    `${JSON.stringify({
      definitions: ["steps/**/*.ts"],
      features: ["features/**/*.feature"],
      threshold: 100,
      // `overlapping-matcher` is the consumer under test: it scans every compiled matcher. The pair
      // rules are off because they emit a finding per definition, which would make this gate
      // measure report size rather than matcher analysis.
      rules: {
        "duplicate-matcher": "off",
        "normalized-matcher": "off",
        "duplicate-handler": "off",
        "near-duplicate-step": "off",
        "parameterization-candidate": "off",
        "unused-definition": "off",
      },
    }, null, 2)}\n`,
  );
}

function measure(root, output, definitions) {
  const binaryArguments = [".", "--reporters", "json", "--output", output];
  const memoryFile = join(root, ".memory");
  let command = binary;
  let commandArguments = binaryArguments;
  let format = null;
  if (existsSync("/usr/bin/time") && process.platform === "linux") {
    command = "/usr/bin/time";
    commandArguments = ["-f", "%M", "-o", memoryFile, binary, ...binaryArguments];
    format = "linux-kib";
  } else if (existsSync("/usr/bin/time") && process.platform === "darwin") {
    command = "/usr/bin/time";
    commandArguments = ["-l", binary, ...binaryArguments];
    format = "darwin-bytes";
  }
  if (format === null) return null;

  const result = spawnSync(command, commandArguments, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 8 * 1024 * 1024,
  });
  // A signal-killed child (an OOM, most likely) reports status null, so name what happened rather
  // than asserting on an empty message.
  assert.ok(
    result.status === 0 || result.status === 1,
    `analysis of ${definitions} definitions exited with status ${result.status}`
      + `${result.signal ? ` (signal ${result.signal})` : ""}: ${result.stderr || result.stdout || "no output"}`,
  );
  const report = JSON.parse(readFileSync(join(output, "cuke-dedup.json"), "utf8"));
  let peak;
  if (format === "linux-kib") {
    const kibibytes = Number.parseInt(readFileSync(memoryFile, "utf8").trim(), 10);
    assert.ok(Number.isFinite(kibibytes), "GNU time did not report peak memory");
    peak = kibibytes * 1024;
  } else {
    const matched = result.stderr.match(/^\s*(\d+)\s+maximum resident set size\s*$/m);
    assert.ok(matched, `BSD time did not report peak memory; stderr was: ${result.stderr}`);
    peak = Number.parseInt(matched[1], 10);
  }
  return {
    peak,
    definitionsAnalyzed: report.summary.definitionsAnalyzed,
    findings: report.summary.findings,
    truncated: report.analysis.truncated,
    analysis: report.analysis,
  };
}

const temporary = await mkdtemp(join(tmpdir(), "cuke-dedup-memory-"));
try {
  const measurements = [];
  for (const size of SIZES) {
    const root = join(temporary, `corpus-${size}`);
    await buildCorpus(root, size);
    const measured = measure(root, join(temporary, `report-${size}`), size);
    if (measured === null) {
      // Skipping is fine on a developer machine without `/usr/bin/time`, but in CI it would make
      // this gate silently decorative — the exact failure mode the gate was added to close. CI
      // runners that cannot measure must fail loudly rather than report success.
      assert.ok(
        !process.env.CI,
        `no peak-memory source on ${process.platform}: the memory ratchet cannot run in CI, so it would pass without measuring anything`,
      );
      console.log(`Memory ratchet skipped: no peak-memory source on ${process.platform}.`);
      process.exit(0);
    }
    // Work floor. Without this the gate would pass most cheaply by analyzing nothing, which is
    // exactly the regression it is meant to catch.
    // The planted regex twins are extra definitions on top of the requested size.
    const expectedDefinitions = size + PLANTED_OVERLAPS;
    assert.equal(
      measured.definitionsAnalyzed,
      expectedDefinitions,
      `expected ${expectedDefinitions} definitions to be analyzed, got ${measured.definitionsAnalyzed}`,
    );
    // A fixed floor, not merely "> 0": the planted overlaps must actually be found, so the gate
    // cannot be satisfied by a build that compiles matchers and then never scans them.
    assert.ok(
      measured.findings >= PLANTED_OVERLAPS,
      `expected at least ${PLANTED_OVERLAPS} findings from the planted overlaps, got ${measured.findings}`,
    );
    assert.equal(
      measured.truncated,
      false,
      `analysis truncated at ${size} definitions, so the two sizes did unequal work and the slope is meaningless: ${JSON.stringify(measured.analysis.candidateSources)}`,
    );
    measurements.push({ size, ...measured });
  }

  const [small, large] = measurements;
  const marginal = (large.peak - small.peak) / (large.size - small.size);
  for (const { size, peak, findings } of measurements) {
    console.log(
      `  ${String(size).padStart(6)} definitions: ${(peak / 2 ** 20).toFixed(1)} MB peak, ${findings} findings`,
    );
  }
  console.log(
    `Memory ratchet: ${marginal.toFixed(0)} bytes per definition (budget ${BUDGET_BYTES_PER_DEFINITION}).`,
  );
  assert.ok(
    marginal <= BUDGET_BYTES_PER_DEFINITION,
    `marginal memory ${marginal.toFixed(0)} B/definition exceeds the ${BUDGET_BYTES_PER_DEFINITION} B budget; matcher analysis regressed`,
  );
} finally {
  await rm(temporary, { recursive: true, force: true });
}
