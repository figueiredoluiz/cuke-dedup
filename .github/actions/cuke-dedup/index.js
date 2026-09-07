import {
  appendFileSync,
  chmodSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  readFileSync,
} from "node:fs";
import { createHash, randomUUID } from "node:crypto";
import { basename, isAbsolute, join, resolve } from "node:path";
import { pipeline } from "node:stream/promises";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const manifest = JSON.parse(
  readFileSync(new URL("../../../npm/prebuilt-targets.json", import.meta.url), "utf8"),
);
const actionPackage = JSON.parse(
  readFileSync(new URL("../../../package.json", import.meta.url), "utf8"),
);

export function detectLibc(report = process.report) {
  try {
    return report?.getReport()?.header?.glibcVersionRuntime ? "gnu" : "musl";
  } catch {
    return "musl";
  }
}

export function releaseTarget(
  platform = process.platform,
  arch = process.arch,
  libc = platform === "linux" ? detectLibc() : undefined,
) {
  const key = platform === "linux" ? `${platform}-${arch}-${libc}` : `${platform}-${arch}`;
  const target = manifest.targets[key];
  if (!target) {
    throw new Error(`unsupported runner ${key}`);
  }
  return target;
}

export function buildArguments(inputs, effectiveConfig = {}) {
  const reporters = splitList(inputs.reporters || effectiveConfig.reporters?.join(",") || "terminal");
  if (!reporters.includes("json")) {
    reporters.push("json");
  }
  const args = [inputs.path || "."];
  if (inputs.threshold !== undefined && inputs.threshold !== "") {
    args.push("--threshold", inputs.threshold);
  }
  args.push(
    "--reporters",
    reporters.join(","),
    "--output",
    inputs.output || effectiveConfig.output || "reports/cuke-dedup",
  );
  if (inputs.config) {
    args.push("--config", inputs.config);
  }
  const exclusions = splitList(inputs.exclude);
  if (exclusions.length > 0) {
    args.push("--exclude", exclusions.join(","));
  }
  if (inputs.changedSince) {
    args.push("--changed-since", inputs.changedSince);
  }
  if (inputs.baseline) {
    args.push("--baseline", inputs.baseline);
  }
  if (inputs.failOnNew !== undefined && inputs.failOnNew !== "") {
    args.push("--fail-on-new", inputs.failOnNew);
  }
  if (String(inputs.noMetrics).toLowerCase() === "true") {
    args.push("--no-metrics");
  }
  return args;
}

export function buildEffectiveConfigArguments(inputs) {
  const args = [inputs.path || ".", "--print-config"];
  if (inputs.config) {
    args.push("--config", inputs.config);
  }
  if (inputs.threshold !== undefined && inputs.threshold !== "") {
    args.push("--threshold", inputs.threshold);
  }
  if (inputs.reporters) {
    args.push("--reporters", splitList(inputs.reporters).join(","));
  }
  if (inputs.output) {
    args.push("--output", inputs.output);
  }
  const exclusions = splitList(inputs.exclude);
  if (exclusions.length > 0) {
    args.push("--exclude", exclusions.join(","));
  }
  if (String(inputs.noMetrics).toLowerCase() === "true") {
    args.push("--no-metrics");
  }
  return args;
}

export async function runAction({ env = process.env, cwd = process.cwd() } = {}) {
  const inputs = {
    config: readInput(env, "config", ""),
    path: readInput(env, "path", "."),
    threshold: readInput(env, "threshold", ""),
    exclude: readInput(env, "exclude", ""),
    reporters: readInput(env, "reporters", ""),
    output: readInput(env, "output", ""),
    changedSince: readInput(env, "changed-since", ""),
    baseline: readInput(env, "baseline", ""),
    failOnNew: readInput(env, "fail-on-new", ""),
    noMetrics: readInput(env, "no-metrics", "false"),
    version: readInput(env, "version", "") || actionPackage.version,
  };
  const binary = env.CUKE_DEDUP_BINARY
    ? resolve(env.CUKE_DEDUP_BINARY)
    : await installRelease(inputs.version, env);
  const effectiveConfig = readEffectiveConfig(binary, inputs, cwd);
  const args = buildArguments(inputs, effectiveConfig);
  const result = spawnSync(binary, args, { cwd, stdio: "inherit" });
  let exitCode = result.status ?? 2;
  if (result.error) {
    console.error(`cuke-dedup: failed to start ${binary}: ${result.error.message}`);
    exitCode = 2;
  } else if (result.signal) {
    console.error(`cuke-dedup: native process terminated by ${result.signal}`);
    exitCode = 2;
  }

  const output = inputs.output || effectiveConfig.output || "reports/cuke-dedup";
  const outputDirectory = resolveReportDirectory(
    cwd,
    effectiveConfig.root || inputs.path,
    output,
  );
  const jsonReport = join(outputDirectory, "cuke-dedup.json");
  const requestedReporters = splitList(
    inputs.reporters || effectiveConfig.reporters?.join(",") || "terminal",
  );
  const values = reportOutputs(jsonReport, outputDirectory, requestedReporters, exitCode);
  for (const [name, value] of Object.entries(values)) {
    setOutput(env, name, value);
  }
  return exitCode;
}

function readEffectiveConfig(binary, inputs, cwd) {
  const args = buildEffectiveConfigArguments(inputs);
  const result = spawnSync(binary, args, { cwd, encoding: "utf8" });
  if (result.error) {
    throw new Error(`failed to resolve CukeDedup configuration: ${result.error.message}`);
  }
  if (result.status !== 0) {
    throw new Error(
      `failed to resolve CukeDedup configuration: ${result.stderr?.trim() || `exit ${result.status}`}`,
    );
  }
  try {
    return JSON.parse(result.stdout);
  } catch (error) {
    throw new Error(`CukeDedup returned invalid effective configuration: ${error.message}`);
  }
}

export function resolveReportDirectory(cwd, analysisPath, output) {
  return isAbsolute(output) ? output : resolve(cwd, analysisPath, output);
}

export function reportOutputs(jsonPath, outputDirectory, requestedReporters, exitCode) {
  const defaults = {
    "exit-code": String(exitCode),
    "duplicate-rate": "0",
    "duplicate-definitions": "0",
    "total-definitions": "0",
    "json-report": jsonPath,
    "html-report": requestedReporters.includes("html")
      ? join(outputDirectory, "cuke-dedup.html")
      : "",
    "sarif-report": requestedReporters.includes("sarif")
      ? join(outputDirectory, "cuke-dedup.sarif")
      : "",
  };
  if (!existsSync(jsonPath)) {
    return defaults;
  }
  const report = JSON.parse(readFileSync(jsonPath, "utf8"));
  const duplication = report?.summary?.duplication;
  if (!duplication) {
    return defaults;
  }
  return {
    ...defaults,
    "duplicate-rate": String(duplication.percentage ?? 0),
    "duplicate-definitions": String(duplication.duplicatedDefinitions ?? 0),
    "total-definitions": String(duplication.totalDefinitions ?? 0),
  };
}

async function installRelease(rawVersion, env) {
  const version = rawVersion.replace(/^v/, "");
  if (!/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`invalid release version ${rawVersion}`);
  }
  const target = releaseTarget();
  const extension = target.platform === "win32" ? "zip" : "tar.gz";
  const assetName = `cuke-dedup-v${version}-${target.rustTarget}.${extension}`;
  const installDirectory = join(
    env.RUNNER_TEMP || env.TMPDIR || ".",
    "cuke-dedup-action",
    version,
    target.rustTarget,
  );
  const binary = join(installDirectory, target.binaryName);
  if (existsSync(binary)) {
    return binary;
  }
  mkdirSync(installDirectory, { recursive: true });
  const archive = join(installDirectory, assetName);
  const checksum = `${archive}.sha256`;
  const base = `https://github.com/figueiredoluiz/cuke-dedup/releases/download/v${version}`;
  await download(`${base}/${assetName}`, archive);
  await download(`${base}/${assetName}.sha256`, checksum);
  verifyChecksum(archive, checksum);
  const extracted = spawnSync("tar", ["-xf", archive, "-C", installDirectory], {
    stdio: "inherit",
  });
  if (extracted.status !== 0 || extracted.error) {
    throw new Error(`failed to extract ${basename(archive)}`);
  }
  if (!existsSync(binary)) {
    throw new Error(`release archive did not contain ${target.binaryName}`);
  }
  if (target.platform !== "win32") {
    chmodSync(binary, 0o755);
  }
  return binary;
}

async function download(url, destination) {
  const response = await fetch(url, {
    redirect: "follow",
    headers: { "user-agent": "cuke-dedup-action" },
  });
  if (!response.ok || !response.body) {
    throw new Error(`failed to download ${url}: HTTP ${response.status}`);
  }
  await pipeline(response.body, createWriteStream(destination));
}

export function verifyChecksum(archive, checksumFile) {
  const expected = readFileSync(checksumFile, "utf8").trim().split(/\s+/)[0];
  if (!/^[0-9a-f]{64}$/i.test(expected)) {
    throw new Error(`invalid checksum file for ${basename(archive)}`);
  }
  const actual = createHash("sha256").update(readFileSync(archive)).digest("hex");
  if (actual.toLowerCase() !== expected.toLowerCase()) {
    throw new Error(`checksum mismatch for ${basename(archive)}`);
  }
}

function splitList(value = "") {
  return value
    .split(/[\n,]+/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

function readInput(env, name, fallback) {
  const exact = `INPUT_${name.toUpperCase()}`;
  const portable = exact.replaceAll("-", "_");
  return env[exact] ?? env[portable] ?? fallback;
}

function setOutput(env, name, value) {
  if (env.GITHUB_OUTPUT) {
    const delimiter = `cuke_dedup_${randomUUID()}`;
    appendFileSync(env.GITHUB_OUTPUT, `${name}<<${delimiter}\n${value}\n${delimiter}\n`);
  } else {
    console.log(`${name}=${value}`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    process.exitCode = await runAction();
  } catch (error) {
    console.error(`cuke-dedup action: ${error.message}`);
    process.exitCode = 2;
  }
}
