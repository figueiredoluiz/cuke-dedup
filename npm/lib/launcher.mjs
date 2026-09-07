import { accessSync, constants, existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { constants as osConstants } from "node:os";

const require = createRequire(import.meta.url);

const targetManifest = JSON.parse(
  readFileSync(new URL("../prebuilt-targets.json", import.meta.url), "utf8"),
);

export const TARGETS = Object.freeze(targetManifest.targets);

export function detectLibc(report = process.report) {
  try {
    return report?.getReport()?.header?.glibcVersionRuntime ? "gnu" : "musl";
  } catch {
    return "musl";
  }
}

export function targetFor(
  platform = process.platform,
  arch = process.arch,
  libc = platform === "linux" ? detectLibc() : undefined,
) {
  const key = platform === "linux" ? `${platform}-${arch}-${libc}` : `${platform}-${arch}`;
  const target = TARGETS[key];
  if (!target) {
    throw new Error(
      `unsupported platform ${key}; supported platforms: ${Object.keys(TARGETS).join(", ")}`,
    );
  }
  return target;
}

export function resolveBinary({
  platform = process.platform,
  arch = process.arch,
  libc = platform === "linux" ? detectLibc() : undefined,
  override = process.env.CUKE_DEDUP_BINARY,
  resolvePackage = require.resolve,
} = {}) {
  if (override) {
    const binary = resolve(override);
    assertUsableBinary(binary, platform);
    return binary;
  }

  const target = targetFor(platform, arch, libc);
  let manifest;
  try {
    manifest = resolvePackage(`${target.packageName}/package.json`);
  } catch (error) {
    throw new Error(
      `native package ${target.packageName} is missing; reinstall cuke-dedup with optional dependencies enabled`,
      { cause: error },
    );
  }
  const binary = resolve(dirname(manifest), "bin", target.binaryName);
  assertUsableBinary(binary, platform);
  return binary;
}

function assertUsableBinary(binary, platform) {
  if (!existsSync(binary)) {
    throw new Error(`cuke-dedup binary was not found at ${binary}`);
  }
  if (platform !== "win32") {
    try {
      accessSync(binary, constants.X_OK);
    } catch (error) {
      throw new Error(`cuke-dedup binary is not executable at ${binary}`, {
        cause: error,
      });
    }
  }
}

export function run(
  args,
  {
    binary = undefined,
    spawn = spawnSync,
    stderr = process.stderr,
  } = {},
) {
  let resolvedBinary;
  try {
    resolvedBinary = binary ?? resolveBinary();
  } catch (error) {
    stderr.write(`cuke-dedup: ${error.message}\n`);
    return 2;
  }

  const result = spawn(resolvedBinary, args, { stdio: "inherit" });
  if (result.error) {
    stderr.write(`cuke-dedup: failed to start ${resolvedBinary}: ${result.error.message}\n`);
    return 2;
  }
  if (result.signal) {
    stderr.write(`cuke-dedup: native process terminated by ${result.signal}\n`);
    return 128 + (osConstants.signals[result.signal] ?? 0);
  }
  return result.status ?? 2;
}
