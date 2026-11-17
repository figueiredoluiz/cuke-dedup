import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { setTimeout as sleep } from "node:timers/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { TARGETS } from "../../npm/lib/targets.mjs";

const defaultRoot = fileURLToPath(new URL("../../", import.meta.url));

const registry = "https://registry.npmjs.org";
const repository = "figueiredoluiz/cuke-dedup";
const workflow = `${repository}/.github/workflows/release.yml`;
const provenanceType = "https://slsa.dev/provenance/v1";
const maxMetadataBytes = 512 * 1024;
const visibilityDelays = [0, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000];

/** Requires a completed subprocess before its output can be consumed. */
function checkResult(result, operation) {
  if (result.error) throw result.error;
  if (result.status !== 0 || result.signal) {
    throw new Error(`${operation} failed (${result.status ?? result.signal})`);
  }
}

/** Arguments for publishing the prepared tarball with the release workflow's npm identity. */
export function npmPublishArguments(tarball) {
  return ["publish", tarball, "--workspaces=false", "--ignore-scripts", "--access", "public",
    "--provenance", `--registry=${registry}`];
}

/** Packs a manifest at the release version and records the exact tarball to publish. */
async function packPackage(packageDirectory, { root, runNpm, scratch, releaseTag }) {
  const { name, version } = JSON.parse(await readFile(resolve(root, packageDirectory, "package.json")));
  if (releaseTag !== `v${version}`) throw new Error(`package version differs from ${releaseTag}: ${name}`);
  const result = runNpm(
    ["pack", packageDirectory, "--json", "--workspaces=false", "--ignore-scripts", "--pack-destination", scratch],
    { capture: true, root },
  );
  checkResult(result, `npm pack for ${name}`);
  const packages = JSON.parse(result.stdout);
  const filename = `${name}-${version}.tgz`;
  if (packages.length !== 1 || packages[0].filename !== filename || basename(filename) !== filename) {
    throw new Error(`npm pack returned an unexpected filename for ${name}`);
  }
  const path = join(scratch, filename);
  const bytes = await readFile(path);
  const digest = createHash("sha512").update(bytes).digest();
  return { name, version, spec: `${name}@${version}`, path, digest, size: bytes.length };
}

/** Reads a bounded response directly from the npm registry. */
async function registryBytes(url, limit, fetchResource) {
  const target = new URL(url);
  if (target.origin !== registry || target.username || target.password) {
    throw new Error("unexpected npm registry URL");
  }
  const response = await fetchResource(target, { redirect: "error", signal: AbortSignal.timeout(15_000) });
  if (response.status === 404) return null;
  if (!response.ok || !response.body) throw new Error(`npm registry request failed (${response.status})`);
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > limit) throw new Error("npm registry response does not match the expected size");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks, size);
}

/** Looks up the exact version and checks that registry metadata agrees with the prepared tarball. */
async function lookupPackage(pkg, fetchResource) {
  const bytes = await registryBytes(`${registry}/${encodeURIComponent(pkg.name)}/${pkg.version}`, maxMetadataBytes, fetchResource);
  if (bytes === null) return null;
  const { name, version, dist } = JSON.parse(bytes);
  if (name !== pkg.name || version !== pkg.version ||
      dist?.integrity !== `sha512-${pkg.digest.toString("base64")}` || typeof dist.tarball !== "string") {
    throw new Error(`registry package does not match the prepared tarball for ${pkg.spec}`);
  }
  return dist;
}

/** Verifies the package subject and delegates signed release identity checks to GitHub CLI. */
async function verifyRegistryPackage(pkg, dist, { fetchResource, runGh, scratch, releaseTag, releaseSha }) {
  if (!dist.attestations?.url) return false;
  const tarball = await registryBytes(dist.tarball, pkg.size, fetchResource);
  if (tarball === null) return false;
  if (!createHash("sha512").update(tarball).digest().equals(pkg.digest)) {
    throw new Error(`registry tarball does not match the prepared package for ${pkg.spec}`);
  }
  const bytes = await registryBytes(dist.attestations.url, maxMetadataBytes, fetchResource);
  if (bytes === null) return false;
  const { attestations } = JSON.parse(bytes);
  const entries = attestations.filter(({ predicateType }) => predicateType === provenanceType);
  if (entries.length !== 1) throw new Error(`expected one provenance bundle for ${pkg.spec}`);
  const [{ bundle }] = entries;
  const { subject, predicateType } = JSON.parse(Buffer.from(bundle.dsseEnvelope.payload, "base64"));
  if (predicateType !== provenanceType || subject.length !== 1 ||
      subject[0].name !== `pkg:npm/${pkg.spec}` || subject[0].digest?.sha512 !== pkg.digest.toString("hex")) {
    throw new Error(`provenance subject does not match ${pkg.spec}`);
  }
  const bundlePath = join(scratch, `${pkg.name}.provenance.json`);
  await writeFile(bundlePath, JSON.stringify(bundle));
  checkResult(runGh([
    "attestation", "verify", pkg.path, "--bundle", bundlePath, "--digest-alg", "sha512",
    "--repo", repository, "--signer-workflow", workflow, "--source-ref", `refs/tags/${releaseTag}`,
    "--source-digest", releaseSha, "--deny-self-hosted-runners",
  ]), `provenance verification for ${pkg.spec}`);
  return true;
}

/** Allows time for a visible version's tarball and provenance to reach the registry. */
async function waitForRegistryVerification(pkg, options, initialDist) {
  for (const backoff of visibilityDelays) {
    if (backoff > 0) await options.delay(backoff);
    const dist = backoff === 0 && initialDist ? initialDist : await lookupPackage(pkg, options.fetchResource);
    if (dist !== null && await verifyRegistryPackage(pkg, dist, options)) return true;
  }
  return false;
}

/** Publishes prepared packages after checking the complete existing set against the tagged release. */
export async function publishNpmPackages({
  root = defaultRoot,
  runNpm = runNpmCommand,
  fetchResource = fetch,
  runGh = runGhCommand,
  delay = sleep,
  releaseTag = process.env.RELEASE_TAG,
  releaseSha = process.env.GITHUB_SHA,
} = {}) {
  if (!/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(releaseTag) || !/^[0-9a-f]{40}$/.test(releaseSha)) {
    throw new Error("npm publishing requires the release tag and its commit SHA");
  }
  const scratch = await mkdtemp(join(tmpdir(), "cuke-dedup-npm-"));
  try {
    const options = { root, runNpm, fetchResource, runGh, scratch, releaseTag, releaseSha, delay };
    const directories = Object.values(TARGETS).map(({ packageDirectory }) => `npm/platforms/${packageDirectory}`);
    directories.push(".");
    const missing = [];
    for (const directory of directories) {
      const pkg = await packPackage(directory, options);
      const dist = await lookupPackage(pkg, fetchResource);
      if (dist === null) {
        missing.push(pkg);
        continue;
      }
      if (!await waitForRegistryVerification(pkg, options, dist)) {
        throw new Error(`registry package is missing release verification data for ${pkg.spec}`);
      }
      console.log(`Verified ${pkg.spec}`);
    }
    for (const pkg of missing) {
      console.log(`Publishing ${pkg.spec}`);
      checkResult(runNpm(npmPublishArguments(pkg.path), { capture: false, root }), `npm publish for ${pkg.spec}`);
      if (!await waitForRegistryVerification(pkg, options)) {
        throw new Error(`registry package could not be verified after publication: ${pkg.spec}`);
      }
    }
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

/** Runs GitHub CLI against a local provenance bundle. */
function runGhCommand(args) {
  // pattern-ok: gh inherits the workflow token and runner configuration.
  return spawnSync("gh", args, { timeout: 30_000, stdio: ["ignore", "ignore", "inherit"] });
}

/** Runs npm in `root`, capturing output when `capture` is set and inheriting it otherwise. */
export function runNpmCommand(arguments_, { capture, root }) {
  const { executable, prefixArguments } = npmInvocation();
  // pattern-ok: npm inherits the release workflow's OIDC and npm configuration.
  return spawnSync(executable, [...prefixArguments, ...arguments_], {
    cwd: root,
    encoding: "utf8",
    timeout: 120_000,
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
}

/** How to launch npm on this platform: directly, or through Node and `npm-cli.js` on Windows. */
export function npmInvocation({
  platform = process.platform,
  nodeExecutable = process.execPath,
  npmExecutablePath = process.env.npm_execpath,
} = {}) {
  if (platform !== "win32") {
    return { executable: "npm", prefixArguments: [] };
  }

  const npmCli = npmExecutablePath
    ?? resolve(dirname(nodeExecutable), "node_modules/npm/bin/npm-cli.js");
  return { executable: nodeExecutable, prefixArguments: [npmCli] };
}

const invokedModule = process.argv[1]
  ? pathToFileURL(resolve(process.argv[1])).href
  : undefined;
if (invokedModule === import.meta.url) {
  await publishNpmPackages();
}
