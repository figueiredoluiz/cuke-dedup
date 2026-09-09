import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { TARGETS } from "../../npm/lib/targets.mjs";

const defaultRoot = fileURLToPath(new URL("../../", import.meta.url));

export function registryVersionState(result, expectedVersion) {
  if (result.error) {
    throw result.error;
  }

  if (result.status === 0) {
    let publishedVersion;
    try {
      publishedVersion = JSON.parse(result.stdout);
    } catch (error) {
      throw new Error("npm returned invalid JSON while checking a published version", {
        cause: error,
      });
    }
    if (publishedVersion !== expectedVersion) {
      throw new Error(
        `npm returned version ${JSON.stringify(publishedVersion)} while checking ${expectedVersion}`,
      );
    }
    return "published";
  }

  const output = `${result.stdout ?? ""}\n${result.stderr ?? ""}`;
  if (output.includes("E404")) {
    return "missing";
  }

  throw new Error(
    `npm view failed with exit code ${result.status}; refusing to publish without a reliable registry check`,
  );
}

export function npmPublishArguments(packageDirectory) {
  const location = packageDirectory === "." ? ["--workspaces=false"] : [packageDirectory];
  return ["publish", ...location, "--access", "public", "--provenance"];
}

export async function publishNpmPackages({ root = defaultRoot, runNpm = runNpmCommand } = {}) {
  const packageDirectories = Object.values(TARGETS).map(
    ({ packageDirectory }) => `npm/platforms/${packageDirectory}`,
  );
  packageDirectories.push(".");

  for (const packageDirectory of packageDirectories) {
    const packageManifest = JSON.parse(
      await readFile(resolve(root, packageDirectory, "package.json"), "utf8"),
    );
    const packageSpec = `${packageManifest.name}@${packageManifest.version}`;
    const viewResult = runNpm(
      ["view", packageSpec, "version", "--json"],
      { capture: true, root },
    );

    if (registryVersionState(viewResult, packageManifest.version) === "published") {
      console.log(`Skipping ${packageSpec}: already published`);
      continue;
    }

    console.log(`Publishing ${packageSpec}`);
    const publishResult = runNpm(npmPublishArguments(packageDirectory), {
      capture: false,
      root,
    });
    if (publishResult.error) {
      throw publishResult.error;
    }
    if (publishResult.status !== 0) {
      throw new Error(`npm publish failed for ${packageSpec} with exit code ${publishResult.status}`);
    }
  }
}

export function runNpmCommand(arguments_, { capture, root }) {
  const { executable, prefixArguments } = npmInvocation();
  return spawnSync(executable, [...prefixArguments, ...arguments_], {
    cwd: root,
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
}

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
