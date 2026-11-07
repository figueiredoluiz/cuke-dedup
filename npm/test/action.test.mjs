import { spawnSync } from "node:child_process";
import { fileURLToPath, pathToFileURL } from "node:url";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { chmod, copyFile, link, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, posix, resolve, win32 } from "node:path";
import test from "node:test";

import {
  buildArguments,
  buildEffectiveConfigArguments,
  download,
  releaseTarget,
  reportOutputs,
  resolveReportDirectory,
  runAction,
  validateArchiveMemberNames,
  verifyChecksum,
  verifyProvenance,
} from "../../.github/actions/cuke-dedup/index.js";

/** Verify the Action selects the expected Rust target for every supported platform and libc. */
test("action maps all eight release targets", () => {
  assert.equal(releaseTarget("linux", "x64", "gnu").rustTarget, "x86_64-unknown-linux-gnu");
  assert.equal(releaseTarget("linux", "x64", "musl").rustTarget, "x86_64-unknown-linux-musl");
  assert.equal(releaseTarget("linux", "arm64", "gnu").rustTarget, "aarch64-unknown-linux-gnu");
  assert.equal(releaseTarget("linux", "arm64", "musl").rustTarget, "aarch64-unknown-linux-musl");
  assert.equal(releaseTarget("darwin", "x64").rustTarget, "x86_64-apple-darwin");
  assert.equal(releaseTarget("darwin", "arm64").rustTarget, "aarch64-apple-darwin");
  assert.equal(releaseTarget("win32", "x64").rustTarget, "x86_64-pc-windows-msvc");
  assert.equal(releaseTarget("win32", "arm64").rustTarget, "aarch64-pc-windows-msvc");
});

/** Verify target metadata is validated before use and unsupported platforms are rejected. */
test("action rejects malformed release target metadata", () => {
  const valid = { ...releaseTarget("linux", "x64", "gnu") };
  assert.equal(
    releaseTarget("linux", "x64", "gnu", { "linux-x64-gnu": valid }),
    valid,
  );
  assert.throws(
    () => releaseTarget("linux", "x64", "gnu", {
      "linux-x64-gnu": { ...valid, binaryName: undefined },
    }),
    /invalid release target metadata/,
  );
  for (const override of [
    { platform: undefined },
    { platform: "win32" },
    { arch: undefined },
    { arch: "arm64" },
    { libc: "musl" },
    { binaryName: "../cuke-dedup" },
    { rustTarget: undefined },
    { rustTarget: 42 },
    { rustTarget: "" },
    { rustTarget: "../../other-release" },
    { rustTarget: "x86_64/other-release" },
  ]) {
    assert.throws(
      () => releaseTarget("linux", "x64", "gnu", {
        "linux-x64-gnu": { ...valid, ...override },
      }),
      /invalid release target metadata/,
    );
  }
  assert.throws(
    () => releaseTarget("freebsd", "x64", undefined, {}),
    /unsupported platform freebsd-x64/,
  );
});

/** Verify relative report paths use the analyzed root and absolute paths remain absolute. */
test("action resolves relative reports from the analyzed root", () => {
  const workspace = resolve("/workspace");
  const absoluteOutput = resolve("/tmp/reports");
  assert.equal(
    resolveReportDirectory(workspace, "tests/e2e", "reports/cuke-dedup"),
    resolve(workspace, "tests/e2e", "reports/cuke-dedup"),
  );
  assert.equal(
    resolveReportDirectory(workspace, "tests/e2e", absoluteOutput),
    absoluteOutput,
  );
});

/** Verify anchoring follows native path semantics without erasing filesystem traversal. */
test("action preserves native POSIX and Windows path semantics", () => {
  for (const [paths, cwd, root, output, expected] of [
    [posix, "/work", "/work", "alias/..", "/work/alias/.."],
    [posix, "/work", "analysis", "../reports", "/work/analysis/../reports"],
    [win32, "C:\\work", "C:\\work\\analysis", "C:reports\\cuke-dedup.json", "C:\\work\\reports\\cuke-dedup.json"],
    [win32, "C:\\work", "D:\\analysis", "\\reports", "D:\\reports"],
    [win32, "C:\\work", "D:\\analysis", "C:\\reports", "C:\\reports"],
    [win32, "C:\\work", "C:\\work\\analysis", "alias\\..", "C:\\work\\analysis\\alias\\.."],
    [win32, "C:\\work", "\\\\server\\share\\analysis", "\\reports", "\\\\server\\share\\reports"],
    [win32, "C:\\work", "C:\\analysis", "\\\\server\\share\\reports", "\\\\server\\share\\reports"],
  ]) assert.equal(resolveReportDirectory(cwd, root, output, paths), expected);
});

/** Verify explicit inputs form separate CLI arguments and include the internal JSON reporter. */
test("action constructs an argument array and always requests JSON", () => {
  assert.deepEqual(
    buildArguments({
      config: ".cuke-dedup.json",
      path: "features/e2e",
      threshold: "5",
      exclude: "generated/**\nfixtures/vendor/**",
      reporters: "terminal,html,sarif",
      output: "reports",
      changedSince: "origin/main",
      baseline: "baseline.json",
      failOnNew: "0",
    }),
    [
      "features/e2e",
      "--threshold", "5",
      "--reporters", "terminal,html,sarif,json",
      "--output", "reports",
      "--config", ".cuke-dedup.json",
      "--exclude", "generated/**,fixtures/vendor/**",
      "--changed-since", "origin/main",
      "--baseline", "baseline.json",
      "--fail-on-new", "0",
    ],
  );
});

/** Verify requesting JSONL retains that reporter while adding the Action's JSON report. */
test("action preserves JSONL stdout while adding its internal JSON report", () => {
  assert.deepEqual(
    buildArguments({
      path: ".",
      threshold: "0",
      reporters: "jsonl",
      output: "reports",
    }),
    [
      ".",
      "--threshold", "0",
      "--reporters", "jsonl,json",
      "--output", "reports",
    ],
  );
});

/** Verify Git refs and zero allowances are forwarded literally as separate arguments. */
test("action forwards a Git baseline and zero allowance without shell interpretation", () => {
  const args = buildArguments({ baselineFromRef: "topic;echo unsafe", failOnNew: "0" });
  assert.deepEqual(args.slice(-4), ["--baseline-from-ref", "topic;echo unsafe", "--fail-on-new", "0"]);
  assert.ok(!args.includes("--baseline"));
});

/** Verify the deterministic-report input emits the no-metrics CLI flag. */
test("action exposes deterministic report generation", () => {
  assert.deepEqual(
    buildArguments({
      path: ".",
      reporters: "json",
      output: "reports",
      noMetrics: "true",
    }),
    [
      ".",
      "--reporters", "json",
      "--output", "reports",
      "--no-metrics",
    ],
  );
});

/** Verify incomplete analysis becomes fatal only when the Action input opts in. */
test("action can require complete analysis coverage", () => {
  assert.deepEqual(
    buildArguments({
      path: ".",
      reporters: "json",
      output: "reports",
      failOnIncomplete: "true",
    }),
    [
      ".",
      "--reporters", "json",
      "--output", "reports",
      "--fail-on-incomplete",
    ],
  );
  assert.ok(
    !buildArguments({ path: ".", reporters: "json", output: "reports" }).includes(
      "--fail-on-incomplete",
    ),
    "incomplete coverage stays non-fatal unless the input opts in",
  );
});

/** Verify omitted Action inputs retain resolved project reporters and output paths. */
test("action preserves resolved project config when inputs are omitted", () => {
  assert.deepEqual(
    buildArguments(
      { path: ".", threshold: "", reporters: "", output: "" },
      {
        threshold: 7.5,
        reporters: ["html", "sarif"],
        output: "quality/cuke-dedup",
      },
    ),
    [
      ".",
      "--reporters", "html,sarif,json",
      "--output", "quality/cuke-dedup",
    ],
  );
});

/** Verify explicit inputs are forwarded when requesting the effective project configuration. */
test("action applies explicit inputs while resolving project configuration", () => {
  assert.deepEqual(
    buildEffectiveConfigArguments({
      config: ".cuke-dedup.json",
      path: "features/e2e",
      threshold: "5",
      exclude: "generated/**\nfixtures/vendor/**",
      reporters: "terminal,html",
      output: "reports",
      noMetrics: "true",
    }),
    [
      "features/e2e",
      "--print-config",
      "--config", ".cuke-dedup.json",
      "--threshold", "5",
      "--reporters", "terminal,html",
      "--output", "reports",
      "--exclude", "generated/**,fixtures/vendor/**",
      "--no-metrics",
    ],
  );
});

/** Verify JSON report metrics, analyzer exit status, and requested report paths become Action outputs. */
test("action exposes threshold metrics and requested report paths", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-dedup-action-"));
  const output = join(directory, "reports");
  await mkdir(output);
  const json = join(output, "cuke-dedup.json");
  await writeFile(json, JSON.stringify({
    schemaVersion: "3",
    summary: {
      duplication: {
        percentage: 12.5,
        duplicatedDefinitions: 2,
        totalDefinitions: 16,
      },
    },
  }));
  assert.deepEqual(reportOutputs(json, output, ["html", "sarif"], 1), {
    "exit-code": "1",
    "duplicate-rate": "12.5",
    "duplicate-definitions": "2",
    "total-definitions": "16",
    "json-report": json,
    "html-report": join(output, "cuke-dedup.html"),
    "sarif-report": join(output, "cuke-dedup.sarif"),
  });
});

/** Verify failed analysis never consumes even a well-formed pre-existing report. */
test("action ignores reports after an analysis error", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-action-error-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const json = join(directory, "cuke-dedup.json");
  for (const content of [JSON.stringify({
    schemaVersion: "3",
    summary: { duplication: { percentage: 75, duplicatedDefinitions: 3, totalDefinitions: 4 } },
  }), "invalid JSON"]) {
    await writeFile(json, content);
    for (const exitCode of [2, 9]) {
      const outputs = reportOutputs(json, directory, [], exitCode);
      assert.equal(outputs["exit-code"], String(exitCode));
      assert.equal(outputs["duplicate-rate"], "0");
      assert.equal(outputs["duplicate-definitions"], "0");
      assert.equal(outputs["total-definitions"], "0");
    }
  }
});

/** Verify unsupported schemas and invalid metric types cannot become Action outputs. */
test("action rejects invalid report metrics", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-action-schema-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const json = join(directory, "cuke-dedup.json");
  const valid = { percentage: 25, duplicatedDefinitions: 1, totalDefinitions: 4 };
  const reports = [null, {}, { schemaVersion: "3" },
    ...[undefined, 3, "2", "99"].map((schemaVersion) => ({
      schemaVersion, summary: { duplication: valid },
    })),
    ...Object.keys(valid).flatMap((key) => [undefined, null, true, "1", {}, [], -1, 1e309].map((value) => ({
      schemaVersion: "3", summary: { duplication: { ...valid, [key]: value } },
    }))),
    ...[{ percentage: 101 }, { duplicatedDefinitions: 1.5 },
      { totalDefinitions: 4.5 }, { duplicatedDefinitions: 5 },
      { totalDefinitions: Number.MAX_SAFE_INTEGER + 1 }].map((override) => ({
      schemaVersion: "3", summary: { duplication: { ...valid, ...override } },
    })),
  ];
  for (const content of [...reports.map((report) => JSON.stringify(report)),
    '{"schemaVersion":"3","summary":{"duplication":{"percentage":1e309,"duplicatedDefinitions":1,"totalDefinitions":4}}}',
    "invalid JSON"]) {
    await writeFile(json, content);
    const outputs = reportOutputs(json, directory, [], 0);
    assert.equal(outputs["duplicate-rate"], "0", content);
    assert.equal(outputs["duplicate-definitions"], "0", content);
    assert.equal(outputs["total-definitions"], "0", content);
  }
});

/** Verify each invocation publishes only newly written metrics while preserving native status and configured paths. */
test("action binds metrics to the current run", { skip: process.platform === "win32" }, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-action-run-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const root = join(directory, "analysis");
  const output = join(root, "reports");
  await mkdir(output, { recursive: true });
  const json = join(output, "cuke-dedup.json");
  const binary = join(directory, "analyzer.cjs");
  const githubOutput = join(directory, "github-output");
  /** Build a schema-3 report with four definitions and the requested duplication metrics. */
  const report = (percentage, duplicatedDefinitions) => JSON.stringify({
    schemaVersion: "3", summary: { duplication: { percentage, duplicatedDefinitions, totalDefinitions: 4 } },
  });
  for (const exitCode of [0, 1, 2]) {
    for (const writesReport of [false, true]) {
      await writeFile(json, report(75, 3));
      await writeFile(githubOutput, "");
      await writeFile(binary, `#!/usr/bin/env node
const fs = require("node:fs");
if (process.argv.includes("--print-config")) {
  console.log(${JSON.stringify(JSON.stringify({ root, output: "reports", reporters: ["html"] }))});
} else {
  ${writesReport ? `fs.writeFileSync(${JSON.stringify(json)}, ${JSON.stringify(report(25, 1))});` : ""}
  process.exitCode = ${exitCode};
}
`);
      await chmod(binary, 0o755);
      assert.equal(await runAction({ cwd: directory, env: {
        CUKE_DEDUP_BINARY: binary, GITHUB_OUTPUT: githubOutput,
      } }), exitCode);
      const outputs = Object.fromEntries(
        [...(await readFile(githubOutput, "utf8")).matchAll(/([^\n]+)<<([^\n]+)\n([^\n]*)\n\2\n/g)]
          .map((match) => [match[1], match[3]]),
      );
      assert.equal(outputs["exit-code"], String(exitCode));
      const accepted = writesReport && exitCode !== 2;
      assert.equal(outputs["duplicate-rate"], accepted ? "25" : "0");
      assert.equal(outputs["duplicate-definitions"], accepted ? "1" : "0");
      assert.equal(outputs["total-definitions"], accepted ? "4" : "0");
      assert.equal(outputs["json-report"], json);
      assert.equal(outputs["html-report"], join(output, "cuke-dedup.html"));
    }
  }
  await rm(json);
  await mkdir(json);
  const sentinel = join(json, "keep.txt");
  await writeFile(sentinel, "preserve this directory");
  await assert.rejects(runAction({ cwd: directory, env: {
    CUKE_DEDUP_BINARY: binary, GITHUB_OUTPUT: githubOutput,
  } }), (error) => ["EISDIR", "ERR_FS_EISDIR", "EPERM"].includes(error.code));
  assert.equal(await readFile(sentinel, "utf8"), "preserve this directory");
});

/** A failed native invocation must leave an external report behind a checkout directory link intact. */
test("action preserves external reports through linked parents", { skip: process.platform === "win32" }, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-action-output-link-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const root = join(directory, "project");
  const outside = join(directory, "external");
  await mkdir(root);
  await mkdir(outside);
  await symlink(outside, join(root, "reports"), "dir");
  const sentinel = join(outside, "cuke-dedup.json");
  await writeFile(sentinel, "preserve external report");
  const binary = join(directory, "analyzer.cjs");
  await writeFile(binary, `#!/usr/bin/env node
if (process.argv.includes("--print-config")) console.log(JSON.stringify({ root: process.cwd(), output: "reports" }));
else process.exitCode = 2;
`);
  await chmod(binary, 0o755);
  await assert.rejects(runAction({ cwd: root, env: {
    CUKE_DEDUP_BINARY: binary, GITHUB_OUTPUT: join(directory, "github-output"),
  } }), /real directory/);
  assert.equal(await readFile(sentinel, "utf8"), "preserve external report");
});

/** Verify baseline aliases are rejected without deleting input or starting analysis, while distinct paths still publish metrics. */
test("action protects baseline inputs from report cleanup", { skip: process.platform === "win32" }, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-action-baseline-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const root = join(directory, "analysis");
  const output = join(root, "reports");
  await mkdir(output, { recursive: true });
  const json = join(output, "cuke-dedup.json");
  const baseline = '{"schemaVersion":3,"fingerprints":{}}';
  const marker = join(directory, "analyzed");
  const binary = join(directory, "analyzer.cjs");
  const githubOutput = join(directory, "github-output");
  await writeFile(binary, `#!/usr/bin/env node
const fs = require("node:fs");
if (process.argv.includes("--print-config")) {
  console.log(${JSON.stringify(JSON.stringify({ root, output: "reports" }))});
} else {
  fs.writeFileSync(${JSON.stringify(marker)}, "ran");
  const baselineIndex = process.argv.indexOf("--baseline");
  const baselinePath = require("node:path").resolve(${JSON.stringify(root)}, process.argv[baselineIndex + 1]);
  if (!fs.existsSync(baselinePath)) {
    process.exitCode = 2;
  } else {
    fs.writeFileSync(${JSON.stringify(json)}, JSON.stringify({ schemaVersion: "3", summary: {
      duplication: { percentage: 25, duplicatedDefinitions: 1, totalDefinitions: 4 }
    } }));
  }
}
`);
  await chmod(binary, 0o755);
  await symlink(json, join(root, "baseline-link.json"));
  await symlink(output, join(root, "report-alias"), "dir");
  await mkdir(join(output, "child"));
  await symlink(join(output, "child"), join(root, "traversal-alias"), "dir");
  const separate = join(directory, "cuke-dedup.json");
  await writeFile(separate, baseline);
  await writeFile(json, baseline);
  await link(json, join(root, "baseline-hardlink.json"));
  const collisions = ["reports/cuke-dedup.json", "reports/../reports/cuke-dedup.json",
    json, "baseline-link.json", "report-alias/cuke-dedup.json", "baseline-hardlink.json",
    "traversal-alias/../cuke-dedup.json"];
  if (existsSync(join(root, "REPORTS", "CUKE-DEDUP.JSON"))) {
    collisions.push("REPORTS/CUKE-DEDUP.JSON");
  }
  for (const input of [...collisions, separate, "missing-baseline.json", "missing-report", "missing-both"]) {
    await writeFile(json, baseline);
    await writeFile(githubOutput, "");
    await rm(marker, { force: true });
    if (input === "missing-report" || input === "missing-both") await rm(json);
    const invocation = runAction({ cwd: directory, env: {
      CUKE_DEDUP_BINARY: binary, GITHUB_OUTPUT: githubOutput, INPUT_PATH: "analysis",
      INPUT_BASELINE: input === "missing-report" ? separate
        : input === "missing-both" ? "reports/cuke-dedup.json" : input,
    } });
    if (!collisions.includes(input)) {
      const missingBaseline = input === "missing-baseline.json" || input === "missing-both";
      assert.equal(await invocation, missingBaseline ? 2 : 0);
      assert.equal(await readFile(marker, "utf8"), "ran");
      assert.equal(await readFile(separate, "utf8"), baseline);
      if (missingBaseline) {
        await assert.rejects(readFile(json), { code: "ENOENT" });
        assert.match(await readFile(githubOutput, "utf8"), /exit-code<<[^\n]+\n2\n/);
        assert.match(await readFile(githubOutput, "utf8"), /duplicate-rate<<[^\n]+\n0\n/);
      } else {
        assert.equal(JSON.parse(await readFile(json, "utf8")).summary.duplication.percentage, 25);
        assert.match(await readFile(githubOutput, "utf8"), /duplicate-rate<<[^\n]+\n25\n/);
      }
    } else {
      await assert.rejects(invocation, /baseline path conflicts with the JSON report path/, input);
      assert.equal(await readFile(json, "utf8"), baseline, input);
      await assert.rejects(readFile(marker), { code: "ENOENT" });
      assert.equal(await readFile(githubOutput, "utf8"), "");
    }
  }
  const sentinel = join(root, "cuke-dedup.json");
  await writeFile(sentinel, "unrelated input");
  await writeFile(json, baseline);
  await writeFile(githubOutput, "");
  const before = await readFile(json, "utf8");
  await assert.rejects(runAction({ cwd: directory, env: {
    CUKE_DEDUP_BINARY: binary, GITHUB_OUTPUT: githubOutput, INPUT_PATH: "analysis",
    INPUT_OUTPUT: "traversal-alias/..", INPUT_BASELINE: separate,
  } }), /real directory/);
  assert.equal(await readFile(sentinel, "utf8"), "unrelated input");
  assert.equal(await readFile(json, "utf8"), before);
  assert.equal(await readFile(githubOutput, "utf8"), "");
});

/** Verify valid archive checksums pass while malformed digests and mismatched bytes fail. */
test("action validates release checksums and fails closed", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-dedup-checksum-"));
  const archive = join(directory, "release.tar.gz");
  const checksum = `${archive}.sha256`;
  await writeFile(archive, "release bytes");
  const digest = "ff7a5e6429d2c8511521e4abf41cd54a3e525ef4a1f24f8d1c67ede9d17874dd";
  await writeFile(checksum, `${digest}  release.tar.gz\n`);
  verifyChecksum(archive, checksum);
  await writeFile(checksum, "not-a-digest  release.tar.gz\n");
  assert.throws(() => verifyChecksum(archive, checksum), /invalid checksum file/);
  await writeFile(checksum, `${"0".repeat(64)}  release.tar.gz\n`);
  assert.throws(() => verifyChecksum(archive, checksum), /checksum mismatch/);
});

/** Verify provenance verification specifies the repository, tagged source, signer workflow, and hosted-runner requirement. */
test("action pins provenance verification to the tagged hosted build", () => {
  let invocation;
  verifyProvenance("release.tar.gz", "release.intoto.jsonl", "1.2.3", (...args) => {
    invocation = args;
    return { status: 0 };
  });
  assert.deepEqual(invocation, [
    "gh",
    [
      "attestation",
      "verify",
      "release.tar.gz",
      "--repo",
      "figueiredoluiz/cuke-dedup",
      "--bundle",
      "release.intoto.jsonl",
      "--signer-workflow",
      "figueiredoluiz/cuke-dedup/.github/workflows/release.yml",
      "--source-ref",
      "refs/tags/v1.2.3",
      "--deny-self-hosted-runners",
    ],
    { stdio: "inherit" },
  ]);
});

/** Verify archive listings contain exactly the expected binary and license in the required order. */
test("action accepts only the exact release archive structure", () => {
  validateArchiveMemberNames("cuke-dedup\nLICENSE\n", "cuke-dedup");
  assert.throws(
    () => validateArchiveMemberNames("../../cuke-dedup\nLICENSE\n", "cuke-dedup"),
    /unexpected release archive members/,
  );
  assert.throws(
    () => validateArchiveMemberNames("cuke-dedup\n", "cuke-dedup"),
    /unexpected release archive members/,
  );
  assert.throws(
    () => validateArchiveMemberNames("LICENSE\ncuke-dedup\n", "cuke-dedup"),
    /unexpected release archive members/,
  );
  assert.throws(
    () => validateArchiveMemberNames(
      "cuke-dedup\nLICENSE\nextra\n",
      "cuke-dedup",
    ),
    /unexpected release archive members/,
  );
});

/** Verify missing assets retry with capped backoff while other HTTP, network, and stream failures stop. */
test("action retries only missing release assets with bounded backoff", async (t) => {
  for (const [statuses, error] of [
    [[200], null],
    [[404, 200], null],
    [["cancel", 200], null],
    [[...Array(9).fill(404), 200], null],
    [Array(10).fill(404), "HTTP 404"],
    [[403], "HTTP 403"],
    [[500], "HTTP 500"],
    [[404, 503], "HTTP 503"],
    [[204], "HTTP 204"],
    [["network"], "network failure"],
    [["stream"], "stream failure"],
  ]) {
    await t.test(statuses.join(","), async (t) => {
      const directory = await mkdtemp(join(tmpdir(), "cuke-download-"));
      t.after(() => rm(directory, { recursive: true, force: true }));
      const destination = join(directory, "asset");
      const url = "https://example.invalid/release/asset";
      const waits = [];
      let calls = 0;
      let cancelled = 0;
      t.mock.method(globalThis, "fetch", async (requested, options) => {
        assert.equal(requested, url);
        assert.equal(options.redirect, "follow");
        const status = statuses[calls++];
        assert.notEqual(status, undefined, "unexpected extra download attempt");
        if (status === "network") throw new Error("network failure");
        if (status === "stream") return new Response(new ReadableStream({
          start(controller) { controller.error(new Error("stream failure")); },
        }));
        const response = new Response(status === 204 ? null : "asset bytes", { status: status === "cancel" ? 404 : status });
        if (!response.ok) {
          const cancel = response.body.cancel.bind(response.body);
          t.mock.method(response.body, "cancel", async () => {
            cancelled++;
            await cancel();
            if (status === "cancel") throw new Error("cleanup failed");
          });
        }
        return response;
      });
      const result = download(url, destination, async (delay) => { waits.push(delay); });
      if (error) {
        await assert.rejects(result, (cause) => {
          assert.equal(cause.message, error.startsWith("HTTP")
            ? `failed to download ${url}: ${error}` : error);
          return true;
        });
        if (statuses[0] !== "stream") await assert.rejects(readFile(destination), { code: "ENOENT" });
      } else {
        await result;
        assert.equal(await readFile(destination, "utf8"), "asset bytes");
      }
      assert.equal(calls, statuses.length);
      assert.deepEqual(waits, [1000, 2000, 4000, 8000, 10000, 10000, 10000, 10000, 10000].slice(0, calls - 1));
      assert.equal(cancelled, statuses.filter((s) => s === "cancel" || (typeof s === "number" && s >= 400)).length);
    });
  }
});

/** Verify a destination write failure propagates after one fetch without invoking retry backoff. */
test("action does not retry destination write failures", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-download-write-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const fetchMock = t.mock.method(globalThis, "fetch", async () => new Response("asset bytes"));
  await assert.rejects(download("https://example.invalid/asset", directory, async () => {
    assert.fail("write failures must not trigger a retry");
  }), (error) => ["EISDIR", "EPERM", "EACCES"].includes(error.code));
  assert.equal(fetchMock.mock.callCount(), 1);
});

/** A real directory may contain an old report, but cleanup must refuse a Windows junction. */
test("report preparation refuses Windows junctions", { skip: process.platform !== "win32" }, async (t) => {
  const root = await mkdtemp(join(tmpdir(), "report-junction-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const target = join(root, "external");
  await mkdir(target);
  await writeFile(join(target, "cuke-dedup.json"), "old");
  const alias = join(root, "reports");
  await symlink(target, alias, "junction");
  const helper = fileURLToPath(new URL("../../.github/actions/cuke-dedup/prepare-report.mjs", import.meta.url));
  const run = spawnSync(process.execPath, [helper, alias], { encoding: "utf8" });
  assert.equal(run.status, 2, run.stderr);
  assert.equal(await readFile(join(target, "cuke-dedup.json"), "utf8"), "old");
  const regular = spawnSync(process.execPath, [helper, target], { encoding: "utf8" });
  assert.equal(regular.status, 0, regular.stderr);
  assert.equal(existsSync(join(target, "cuke-dedup.json")), false);
});

/** Missing or relative helper arguments fail with an intentional diagnostic. */
test("report preparation validates its directory argument", () => {
  const helper = fileURLToPath(new URL("../../.github/actions/cuke-dedup/prepare-report.mjs", import.meta.url));
  for (const args of [[], [""], ["relative"]]) {
    const run = spawnSync(process.execPath, [helper, ...args], { encoding: "utf8" });
    assert.equal(run.status, 2, run.stderr);
    assert.equal(JSON.parse(run.stderr).message, "expected an absolute report directory");
  }
});

/** A helper startup error retains raw stderr, while structured errors preserve their code. */
test("action retains report preparation diagnostics", { skip: process.platform === "win32" }, async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "action-preparation-error-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const action = join(directory, ".github/actions/cuke-dedup");
  const targets = join(directory, "npm/lib");
  await mkdir(action, { recursive: true });
  await mkdir(targets, { recursive: true });
  await copyFile(new URL("../../.github/actions/cuke-dedup/index.js", import.meta.url), join(action, "index.mjs"));
  await copyFile(new URL("../lib/targets.mjs", import.meta.url), join(targets, "targets.mjs"));
  await copyFile(new URL("../prebuilt-targets.json", import.meta.url), join(directory, "npm/prebuilt-targets.json"));
  const binary = join(directory, "analyzer.cjs");
  await writeFile(binary, `#!/usr/bin/env node
if (process.argv.includes("--print-config")) console.log(JSON.stringify({root: process.cwd(), output:"reports"}));
else process.exitCode = 2;
`);
  await chmod(binary, 0o755);
  const isolated = await import(pathToFileURL(join(action, "index.mjs")).href);
  for (const [stderr, message, code] of [
    ['{"message":"owned diagnostic","code":"EACCES"}', "owned diagnostic", "EACCES"],
    ["  helper startup failed\n", "helper startup failed", undefined],
    ["", "failed to prepare JSON report directory", undefined],
    ["null", "null", undefined],
  ]) {
    await writeFile(join(action, "prepare-report.mjs"),
      `process.stderr.write(${JSON.stringify(stderr)}); process.exitCode = 2;`);
    await assert.rejects(isolated.runAction({ cwd: directory, env: { CUKE_DEDUP_BINARY: binary } }),
      (error) => error.message === message && error.code === code);
  }
});
