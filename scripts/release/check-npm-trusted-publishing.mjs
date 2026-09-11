import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

export const MINIMUM_TRUSTED_PUBLISHING_NPM = "11.5.1";

export function supportsTrustedPublishing(version) {
  const parsed = /^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$/.exec(version.trim());
  if (!parsed) {
    return false;
  }
  const actual = parsed.slice(1).map(Number);
  const minimum = MINIMUM_TRUSTED_PUBLISHING_NPM.split(".").map(Number);
  for (let index = 0; index < minimum.length; index += 1) {
    if (actual[index] !== minimum[index]) {
      return actual[index] > minimum[index];
    }
  }
  return true;
}

export function installedNpmVersion(run = spawnSync) {
  const result = run("npm", ["--version"], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(
      `npm --version failed with exit code ${result.status}: ${result.stderr.trim()}`,
    );
  }
  const version = result.stdout.trim();
  if (!supportsTrustedPublishing(version)) {
    throw new Error(
      `npm ${version || "(unknown)"} does not support trusted publishing; require npm >= ${MINIMUM_TRUSTED_PUBLISHING_NPM}`,
    );
  }
  return version;
}

const invokedModule = process.argv[1]
  ? pathToFileURL(process.argv[1]).href
  : undefined;
if (invokedModule === import.meta.url) {
  console.log(`npm ${installedNpmVersion()} supports trusted publishing`);
}
