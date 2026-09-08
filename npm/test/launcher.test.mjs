import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { detectLibc, resolveBinary, run, targetFor } from "../lib/launcher.mjs";

test("maps every supported platform to its native package", () => {
  assert.equal(
    targetFor("linux", "x64", "gnu").packageName,
    "cuke-dedup-linux-x64-gnu",
  );
  assert.equal(
    targetFor("linux", "x64", "musl").packageName,
    "cuke-dedup-linux-x64-musl",
  );
  assert.equal(
    targetFor("linux", "arm64", "gnu").packageName,
    "cuke-dedup-linux-arm64-gnu",
  );
  assert.equal(
    targetFor("linux", "arm64", "musl").packageName,
    "cuke-dedup-linux-arm64-musl",
  );
  assert.equal(
    targetFor("darwin", "arm64").packageName,
    "cuke-dedup-darwin-arm64",
  );
  assert.equal(
    targetFor("darwin", "x64").packageName,
    "cuke-dedup-darwin-x64",
  );
  assert.equal(
    targetFor("win32", "x64").binaryName,
    "cuke-dedup.exe",
  );
  assert.equal(
    targetFor("win32", "x64").packageName,
    "cuke-dedup-windows-x64-msvc",
  );
  assert.equal(
    targetFor("win32", "arm64").packageName,
    "cuke-dedup-windows-arm64-msvc",
  );
});

test("rejects unsupported platforms clearly", () => {
  assert.throws(
    () => targetFor("freebsd", "x64"),
    /unsupported platform freebsd-x64.*darwin-arm64.*win32-x64/,
  );
});

test("detects GNU and musl Linux runtimes", () => {
  assert.equal(
    detectLibc({ getReport: () => ({ header: { glibcVersionRuntime: "2.39" } }) }),
    "gnu",
  );
  assert.equal(detectLibc({ getReport: () => ({ header: {} }) }), "musl");
  assert.equal(
    detectLibc({ getReport: () => { throw new Error("blocked"); } }),
    "musl",
  );
});

test("reports a missing optional native package", () => {
  assert.throws(
    () =>
      resolveBinary({
        platform: "linux",
        arch: "x64",
        libc: "gnu",
        override: "",
        resolvePackage() {
          throw new Error("missing");
        },
      }),
    /native package cuke-dedup-linux-x64-gnu is missing.*optional dependencies/,
  );
});

test("accepts an explicit executable override", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-dedup-launcher-"));
  const binary = join(directory, "custom-cuke-dedup");
  await writeFile(binary, "#!/bin/sh\nexit 0\n");
  await chmod(binary, 0o755);
  assert.equal(
    resolveBinary({ platform: "linux", override: binary }),
    binary,
  );
});

test("resolves the executable from a platform package layout", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cuke-dedup-platform-"));
  const manifest = join(directory, "package.json");
  const binary = join(directory, "bin", "cuke-dedup");
  await mkdir(join(directory, "bin"), { recursive: true });
  await writeFile(manifest, "{}\n");
  await writeFile(binary, "#!/bin/sh\nexit 0\n");
  await chmod(binary, 0o755);
  assert.equal(
    resolveBinary({
      platform: "linux",
      arch: "x64",
      libc: "gnu",
      override: "",
      resolvePackage(specifier) {
        assert.equal(specifier, "cuke-dedup-linux-x64-gnu/package.json");
        return manifest;
      },
    }),
    binary,
  );
});

test("forwards arguments and the native exit code", () => {
  let invocation;
  const exitCode = run(["check", ".", "--reporters", "json"], {
    binary: "/controlled/cuke-dedup",
    spawn(binary, args, options) {
      invocation = { binary, args, options };
      return { status: 17, signal: null, error: undefined };
    },
  });
  assert.deepEqual(invocation, {
    binary: "/controlled/cuke-dedup",
    args: ["check", ".", "--reporters", "json"],
    options: { stdio: "inherit" },
  });
  assert.equal(exitCode, 17);
});

test("uses operational exit code 2 for launch failures", () => {
  let diagnostic = "";
  const exitCode = run([], {
    binary: "/controlled/cuke-dedup",
    spawn() {
      return { status: null, signal: null, error: new Error("permission denied") };
    },
    stderr: { write(value) { diagnostic += value; } },
  });
  assert.equal(exitCode, 2);
  assert.match(diagnostic, /failed to start.*permission denied/);
});

test("preserves conventional exit status for child signals", () => {
  let diagnostic = "";
  const exitCode = run([], {
    binary: "/controlled/cuke-dedup",
    spawn() {
      return { status: null, signal: "SIGINT", error: undefined };
    },
    stderr: { write(value) { diagnostic += value; } },
  });
  assert.equal(exitCode, 130);
  assert.match(diagnostic, /terminated by SIGINT/);
});
