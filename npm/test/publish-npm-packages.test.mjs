import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import test from "node:test";

import {
  npmPublishArguments,
  npmInvocation,
  publishNpmPackages,
  runNpmCommand,
} from "../../scripts/release/publish-npm-packages.mjs";
import { TARGETS } from "../lib/targets.mjs";

const platformPackages = Object.values(TARGETS).map(({ packageName }) => packageName);
const version = JSON.parse(readFileSync(new URL("../../package.json", import.meta.url))).version;
const releaseTag = `v${version}`;
const releaseSha = "0123456789abcdef0123456789abcdef01234567";

/** Simulates npm packing, registry propagation and provenance verification without publishing. */
function registryFixture({ existing = [], mismatch, wrongBytes, rejectedProvenance, visibility = new Map() } = {}) {
  const available = new Set(existing);
  const published = [];
  const verified = [];
  return {
    published,
    verified,
    options: {
      releaseTag,
      releaseSha,
      delay: async () => {},
      runNpm(args, { root }) {
        if (args[0] === "pack") {
          const manifest = JSON.parse(readFileSync(join(root, args[1], "package.json")));
          const filename = `${manifest.name}-${version}.tgz`;
          writeFileSync(join(args[args.indexOf("--pack-destination") + 1], filename), manifest.name);
          return { status: 0, stdout: JSON.stringify([{ filename }]) };
        }
        assert.equal(args[0], "publish");
        const name = args[1].split(/[\\/]/).at(-1).replace(`-${version}.tgz`, "");
        if (name === "cuke-dedup") assert.deepEqual(verified.slice(-platformPackages.length), platformPackages);
        published.push(name);
        available.add(name);
        return { status: 0 };
      },
      async fetchResource(url) {
        const [, kind, name] = new URL(url).pathname.match(/^\/(?:(tarball|attestation)\/)?([^/]+)\//) ?? [];
        assert.ok(name, url);
        const content = Buffer.from(name === mismatch ? "different package" : name);
        const sha512 = createHash("sha512").update(content).digest();
        if (kind === "tarball") return new Response(name === wrongBytes ? content.map(byte => byte ^ 1) : content);
        if (kind === "attestation") {
          const predicateType = "https://slsa.dev/provenance/v1";
          const payload = { predicateType, subject: [{ name: `pkg:npm/${name}@${version}`, digest: { sha512: sha512.toString("hex") } }] };
          return Response.json({ attestations: [{ predicateType, bundle: { dsseEnvelope: { payload: Buffer.from(JSON.stringify(payload)).toString("base64") } } }] });
        }
        const remaining = visibility.get(name) ?? 0;
        if (available.has(name) && remaining > 0) visibility.set(name, remaining - 1);
        if (!available.has(name) || remaining > 0) return new Response(null, { status: 404 });
        return Response.json({ name, version, dist: {
          integrity: `sha512-${sha512.toString("base64")}`,
          tarball: `https://registry.npmjs.org/tarball/${name}/package.tgz`,
          attestations: { url: `https://registry.npmjs.org/attestation/${name}/bundle` },
        } });
      },
      runGh(args) {
        const bundle = JSON.parse(readFileSync(args[args.indexOf("--bundle") + 1]));
        const payload = JSON.parse(Buffer.from(bundle.dsseEnvelope.payload, "base64"));
        const name = payload.subject[0].name.slice("pkg:npm/".length).split("@")[0];
        assert.equal(args[args.indexOf("--source-ref") + 1], `refs/tags/${releaseTag}`);
        assert.equal(args[args.indexOf("--source-digest") + 1], releaseSha);
        if (name === rejectedProvenance) return { status: 1, stderr: "verification failed" };
        verified.push(name);
        return { status: 0 };
      },
    },
  };
}

/** Verify recovery reuses matching packages and waits for missing platforms before publishing the launcher. */
test("publishes the launcher after every platform is verified", async () => {
  const fixture = registryFixture({ existing: platformPackages.slice(0, -1), visibility: new Map([[platformPackages.at(-1), 2]]) });
  await publishNpmPackages(fixture.options);
  assert.deepEqual(fixture.published, [platformPackages.at(-1), "cuke-dedup"]);
  assert.deepEqual(fixture.verified, platformPackages);
});

/** Verify package equality and provenance checks run before any missing package can be published. */
test("checks the complete existing package set before publishing anything", async () => {
  for (const failure of [
    { mismatch: platformPackages.at(-1) },
    { wrongBytes: platformPackages.at(-1) },
    { rejectedProvenance: "cuke-dedup" },
  ]) {
    const fixture = registryFixture({ existing: [...platformPackages.slice(1), "cuke-dedup"], ...failure });
    await assert.rejects(publishNpmPackages(fixture.options), /does not match|provenance verification.*failed/i);
    assert.deepEqual(fixture.published, []);
    assert.ok(fixture.verified.includes(platformPackages[1]));
  }
});

/** Verify a delayed package is never published twice and the launcher waits for verification. */
test("stops after bounded registry visibility checks", async () => {
  const missing = platformPackages.at(-1);
  const fixture = registryFixture({ existing: platformPackages.slice(0, -1), visibility: new Map([[missing, 100]]) });
  await assert.rejects(publishNpmPackages(fixture.options), /could not be verified after publication/);
  assert.deepEqual(fixture.published, [missing]);
  assert.deepEqual(fixture.verified, platformPackages.slice(0, -1));
});

/** Verify the default npm runner executes successfully and captures a version beginning with three numeric components. */
test("release publishing has a working default npm runner", () => {
  const result = runNpmCommand(["--version"], {
    capture: true,
    root: process.cwd(),
  });

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^\d+\.\d+\.\d+/);
});

/** Verify Windows invokes the npm CLI script through the supplied Node executable. */
test("release publishing invokes npm through Node on Windows", () => {
  assert.deepEqual(
    npmInvocation({
      platform: "win32",
      nodeExecutable: "C:\\node\\node.exe",
      npmExecutablePath: "C:\\node\\node_modules\\npm\\bin\\npm-cli.js",
    }),
    {
      executable: "C:\\node\\node.exe",
      prefixArguments: ["C:\\node\\node_modules\\npm\\bin\\npm-cli.js"],
    },
  );
});

/** Verify registry errors stop publication instead of looking like absent packages. */
test("release publishing requires a reliable registry response", async () => {
  const fixture = registryFixture();
  fixture.options.fetchResource = async () => new Response(null, { status: 503 });
  await assert.rejects(publishNpmPackages(fixture.options), /registry request failed \(503\)/);
  assert.deepEqual(fixture.published, []);
});

/** Verify publication sends the prepared tarball with scoped npm settings. */
test("release publishing sends the prepared tarball", () => {
  assert.deepEqual(
    npmPublishArguments("platform.tgz"),
    [
      "publish",
      "platform.tgz",
      "--workspaces=false",
      "--ignore-scripts",
      "--access",
      "public",
      "--provenance",
      "--registry=https://registry.npmjs.org",
    ],
  );
});
