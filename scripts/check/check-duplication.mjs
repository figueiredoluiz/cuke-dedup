#!/usr/bin/env node
// Copy-paste duplication ratchet.
//
// ## Why a ratchet and not a target
//
// The thresholds below are the values measured today, not the values we want. A gate pinned at an
// aspirational number would be red on arrival, and a gate that is red on arrival gets ignored —
// which is worse than no gate, because it also teaches everyone that the number does not matter.
// Lower a threshold when a change improves the figure; never raise one to make a regression pass.
//
// ## This gate asserts a floor on work performed
//
// "0.00% duplication" and "jscpd matched no files" print almost identically, and jscpd skips any
// file over `--max-lines` lines or `--max-size` bytes **silently**. That is not hypothetical here:
// a measurement in this repository once reported 0.71% when the real figure was 2.88%, because the
// three largest files had been skipped without a word.
//
// So each scope asserts the report exists, that jscpd analysed **exactly** the files staged for it
// — not "at least" — and that the staged line count clears a floor. Any of those failing is a
// failure, not a pass.
//
// ## The tool is pinned
//
// `jscpd@4.0.5` with `--min-tokens 50 --min-lines 5`. A floating version would let the threshold
// drift with someone else's tokenizer, and a threshold that drifts is not a ratchet. The limits are
// raised far past anything in this repository so nothing is skipped.
//
// It is installed from `scripts/check/tools/`, which has its own manifest and lockfile, so the
// whole dependency graph is pinned by integrity hash rather than re-resolved on each run. It is not
// a root `devDependency` because the root declares eight platform packages for other operating
// systems and architectures, and keeping `npm install` unnecessary there is deliberate
// (CONTRIBUTING.md). Fetching it per-run with `npx` was the other option and was rejected: it pins
// only the top-level version, leaving transitive versions and integrity unlocked in CI.
//
// ## Known limitation
//
// Scopes are split by file: `tests.rs` files and `tests/` are the test scope, everything else in
// `src/` is production. Eight `#[cfg(test)] mod tests` blocks remain inline in production files and
// are therefore counted in the production figure. Splitting those needs a Rust-aware scanner with a
// partition invariant; until then the production number is an upper bound, which fails safe.
import assert from "node:assert/strict";
import { cp, mkdir, mkdtemp, readFile, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { spawnSync } from "node:child_process";

/// Every Rust source in the repository, which the scopes below must account for exactly once.
///
/// Discovered rather than listed, so a Rust file added anywhere — a new benchmark, a new fuzz
/// target — fails the partition assertion until someone decides which scope measures it. Listing
/// the scopes' own globs here instead would make that assertion vacuous: it could only ever compare
/// the scopes against themselves.
///
/// Asking git rather than filtering a glob by directory name: git already knows which paths are
/// build output, because they are ignored. Guessing at `target` and `node_modules` segments both
/// missed real sources under a directory that merely shares the name, and could never be complete.
/// `--cached --others --exclude-standard` covers tracked and new-but-not-ignored files alike, so a
/// source is measured before it is committed.
const ALL_RUST_SOURCES = [
  ...new Set(
    spawnSync(
      "git",
      ["ls-files", "--cached", "--others", "--exclude-standard", "--", "*.rs"],
      { encoding: "utf8" },
    ).stdout.split("\n").filter(Boolean),
  ),
]
  // `--cached` lists index entries, so two ordinary states need handling. During an unresolved
  // merge a conflicted path appears once per stage — three times, verified — which would otherwise
  // trip the scope-overlap assertion with an overlap that does not exist; the Set above collapses
  // that. And a file deleted without staging the deletion is still in the index, which would fail
  // the run with ENOENT while staging it rather than with a duplication verdict.
  //
  // Measuring what is on disk is also the correct reading: the working tree is what the ceilings
  // describe. A local deletion therefore shifts the figures rather than being ignored, which the
  // stale-ceiling check reports.
  .filter((path) => existsSync(path));

/// How far below its ceiling a scope may sit before the ceiling is treated as stale.
///
/// Exact equality would be unusable: the figure is duplicated lines over total lines, so adding any
/// non-duplicated code lowers it and would fail a gate that demanded an exact match. A margin keeps
/// the instruction "lower the ceiling after an improvement" enforceable while tolerating that
/// drift, and bounds how far a ceiling can lag the best figure actually achieved.
const STALE_CEILING_MARGIN = 0.05;

/// Below these, jscpd legitimately omits a source from its statistics — there is nothing to
/// tokenise. Measured: a 2-line file is omitted, a 4-line file is reported.
///
/// Both dimensions, because newlines alone do not bound content: a multi-megabyte source written on
/// one physical line would exceed jscpd's size limit, be skipped silently, and be waved through as
/// "trivially small" by a line count of one.
const MINIMUM_REPORTABLE_LINES = 4;
const MINIMUM_REPORTABLE_BYTES = 4_096;

/// A scope is a set of files, the duplication ceilings measured today, and floors proving the
/// measurement actually looked at something.
///
/// Two ceilings, because a percentage alone can be gamed by growth: duplicated lines over total
/// lines falls whenever unique code is added, so copied code can accumulate indefinitely as long as
/// the file grows faster. Roughly 7,200 unique lines would currently mask 50 new duplicated ones.
/// The absolute ceiling is the one that actually says "no more copy-paste than today"; the
/// percentage keeps the figure comparable as the codebase grows.
const SCOPES = [
  {
    name: "production",
    // `basename`, not `endsWith`: `endsWith("tests.rs")` would also exclude a production file
    // named `contests.rs`, which the test glob does not match either, so it would be measured by
    // neither scope. The partition assertion below is the backstop for that whole class.
    files: () => ALL_RUST_SOURCES.filter(
      (path) => path.startsWith("src/") && basename(path) !== "tests.rs",
    ),
    maximumPercentage: 0.69,
    maximumDuplicatedLines: 121,
    minimumFiles: 30,
    minimumLines: 15_000,
  },
  {
    name: "tests",
    // Fuzz targets are harnesses, not shipped code, so they are measured here rather than with
    // production sources.
    files: () => ALL_RUST_SOURCES.filter(
      (path) =>
        basename(path) === "tests.rs"
        || path.startsWith("tests/")
        || path.startsWith("fuzz/"),
    ),
    maximumPercentage: 5.93,
    maximumDuplicatedLines: 1224,
    minimumFiles: 12,
    minimumLines: 15_000,
  },
];

// Every Rust source lands in exactly one scope. Without this a file matching neither pattern is
// simply never measured, and the per-scope file-count assertions still pass because each scope
// staged precisely what it selected.
const partition = SCOPES.flatMap((scope) => scope.files());
const duplicated = partition.filter((path, index) => partition.indexOf(path) !== index);
assert.deepEqual(duplicated, [], `sources claimed by more than one scope: ${duplicated.join(", ")}`);
const unclaimed = ALL_RUST_SOURCES.filter((path) => !partition.includes(path));
assert.deepEqual(
  unclaimed,
  [],
  `Rust sources measured by no scope: ${unclaimed.join(", ")} — add them to a scope or the gate silently ignores them`,
);

/// Resolved from the isolated tool manifest, never from the ambient environment.
const JSCPD_BINARY = join("scripts", "check", "tools", "node_modules", "jscpd", "bin", "jscpd");
assert.ok(
  existsSync(JSCPD_BINARY),
  `jscpd is not installed: run \`npm ci --prefix scripts/check/tools\` (its manifest and lockfile pin the whole graph)`,
);

const temporary = await mkdtemp(join(tmpdir(), "cuke-dedup-duplication-"));
let failures = 0;
try {
  for (const scope of SCOPES) {
    const files = scope.files();
    assert.ok(
      files.length >= scope.minimumFiles,
      `${scope.name}: staged ${files.length} files, floor is ${scope.minimumFiles} — the glob stopped matching`,
    );
    const staged = join(temporary, scope.name);
    for (const file of files) {
      await mkdir(join(staged, dirname(file)), { recursive: true });
      await cp(file, join(staged, file));
    }

    const output = join(temporary, `report-${scope.name}`);
    const result = spawnSync(
      process.execPath,
      [
        JSCPD_BINARY, staged,
        "--reporters", "json", "--output", output,
        // Raised far past anything here: jscpd skips larger files without saying so.
        "--max-lines", "50000", "--max-size", "10mb",
        "--min-tokens", "50", "--min-lines", "5",
        "--silent",
      ],
      { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
    );
    assert.ok(
      result.status === 0,
      `${scope.name}: jscpd exited ${result.status}: ${result.stderr || result.stdout}`,
    );

    const report = JSON.parse(await readFile(join(output, "jscpd-report.json"), "utf8"));
    const total = report.statistics.total;
    const analysedPaths = new Set(
      Object.values(report.statistics.formats ?? {})
        .flatMap((format) => Object.keys(format.sources ?? {}))
        .map((path) => path.slice(staged.length + 1)),
    );
    const analysed = analysedPaths.size;

    // The failure mode is jscpd skipping a *large* file for exceeding a limit, silently, which
    // lowers the percentage. It also omits sources too short to tokenise, which is legitimate — so
    // rather than demand an exact count, every staged file that went unanalysed must be trivially
    // small, and any that is not gets named.
    const unanalysed = [];
    for (const file of files) {
      if (analysedPaths.has(file)) continue;
      const contents = await readFile(file, "utf8");
      const lines = contents.split("\n").length;
      const bytes = Buffer.byteLength(contents);
      if (lines >= MINIMUM_REPORTABLE_LINES || bytes >= MINIMUM_REPORTABLE_BYTES) {
        unanalysed.push(`${file} (${lines} lines, ${bytes} bytes)`);
      }
    }
    assert.deepEqual(
      unanalysed,
      [],
      `${scope.name}: jscpd skipped ${unanalysed.length} non-trivial staged file(s) silently: ${unanalysed.join(", ")}`,
    );
    assert.ok(
      total.lines >= scope.minimumLines,
      `${scope.name}: only ${total.lines} lines measured, floor is ${scope.minimumLines}`,
    );

    // Two decimals with no slack: jscpd is pinned and the detection is deterministic, so there is
    // no noise to absorb.
    const percentage = Number(total.percentage.toFixed(2));
    const verdict = percentage > scope.maximumPercentage
      || total.duplicatedLines > scope.maximumDuplicatedLines
      ? "REGRESSED"
      : percentage < scope.maximumPercentage - STALE_CEILING_MARGIN
        ? "STALE CEILING"
        : "ok";
    console.log(
      `  ${scope.name.padEnd(11)} ${percentage.toFixed(2).padStart(6)}%  `
        + `(ceiling ${scope.maximumPercentage.toFixed(2)}%)  `
        + `${total.duplicatedLines} of ${total.lines} lines, ${total.clones} clones, ${analysed} files  ${verdict}`,
    );
    if (total.duplicatedLines > scope.maximumDuplicatedLines) {
      failures += 1;
      console.log(
        `  ${" ".repeat(11)} duplicated lines rose to ${total.duplicatedLines} from ${scope.maximumDuplicatedLines}`,
      );
    }
    if (percentage > scope.maximumPercentage) {
      failures += 1;
    } else if (percentage < scope.maximumPercentage - STALE_CEILING_MARGIN) {
      // Otherwise an improvement never tightens anything: 0.69 could improve to 0.60, drift back to
      // 0.68, and pass throughout. Failing here forces the ceiling down so the gain is kept.
      failures += 1;
      console.log(
        `  ${" ".repeat(11)} ceiling is stale — lower ${scope.name} to ${percentage.toFixed(2)} in this script to keep the improvement`,
      );
    }
  }
  assert.equal(failures, 0, `${failures} scope(s) regressed or left a stale ceiling`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
