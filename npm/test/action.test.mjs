import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";

import {
  buildArguments,
  buildEffectiveConfigArguments,
  download,
  releaseTarget,
  reportOutputs,
  resolveReportDirectory,
  validateArchiveMemberNames,
  verifyChecksum,
  verifyProvenance,
} from "../../.github/actions/cuke-dedup/index.js";

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

test("action forwards a Git baseline and zero allowance without shell interpretation", () => {
  const args = buildArguments({ baselineFromRef: "topic;echo unsafe", failOnNew: "0" });
  assert.deepEqual(args.slice(-4), ["--baseline-from-ref", "topic;echo unsafe", "--fail-on-new", "0"]);
  assert.ok(!args.includes("--baseline"));
});

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

test("action exposes threshold metrics and requested report paths", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-dedup-action-"));
  const output = join(directory, "reports");
  await mkdir(output);
  const json = join(output, "cuke-dedup.json");
  await writeFile(json, JSON.stringify({
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

test("action does not retry destination write failures", async (t) => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-download-write-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const fetchMock = t.mock.method(globalThis, "fetch", async () => new Response("asset bytes"));
  await assert.rejects(download("https://example.invalid/asset", directory, async () => {
    assert.fail("write failures must not trigger a retry");
  }), (error) => ["EISDIR", "EPERM", "EACCES"].includes(error.code));
  assert.equal(fetchMock.mock.callCount(), 1);
});
