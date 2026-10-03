// Shared discovery for the one class of version site that cannot be enumerated from a manifest.
// Every other site is structural: two Cargo manifests, two Cargo lockfiles, the npm manifests
// named by `npm/prebuilt-targets.json`, the npm lockfile, and two workflow inputs. Action pins in
// prose are free-form, so a new guide can introduce one at any time. `bump-version.mjs` rewrites
// whatever this finds and `check-release-version.mjs` verifies the same set, which keeps a pin in
// a file neither script knew about from going stale unnoticed.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

const SKIPPED_DIRECTORIES = new Set(["node_modules", "target", "dist", ".git", "corpus"]);
const SEARCHED_EXTENSIONS = [".md", ".markdown", ".yml", ".yaml"];

/// Matches a pinned use of this repository's own Action, the form documented for consumers.
export const ACTION_PIN = /figueiredoluiz\/cuke-dedup@v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)/g;

async function* searchableFiles(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.name.startsWith(".") && entry.name !== ".github") {
      continue;
    }
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      if (!SKIPPED_DIRECTORIES.has(entry.name)) {
        yield* searchableFiles(path);
      }
    } else if (SEARCHED_EXTENSIONS.some((extension) => entry.name.endsWith(extension))) {
      yield path;
    }
  }
}

function projectFiles(root) {
  const result = spawnSync("git", ["-C", root, "ls-files", "--cached", "--others", "--exclude-standard", "-z"], {
    encoding: "utf8", timeout: 10_000, maxBuffer: 16 * 1024 * 1024,
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_"))), LC_ALL: "C" },
  });
  if (result.status === 0) {
    return result.stdout.split("\0").filter((path) =>
      SEARCHED_EXTENSIONS.some((extension) => path.endsWith(extension))
      && !path.split("/").some((part) => SKIPPED_DIRECTORIES.has(part) || (part.startsWith(".") && part !== ".github")),
    ).map((path) => join(root, path)).filter(existsSync);
  }
  if (result.status === 128 && result.stderr.includes("not a git repository")) return searchableFiles(root);
  throw result.error ?? new Error(`Git version-site discovery failed: ${result.stderr}`);
}

/// Returns every file carrying at least one Action pin, with the versions it pins, sorted by path
/// so callers report deterministically.
export async function findActionPins(root = ".") {
  const found = [];
  for await (const path of projectFiles(root)) {
    const versions = [...(await readFile(path, "utf8")).matchAll(ACTION_PIN)].map(
      (match) => match[1],
    );
    if (versions.length > 0) {
      found.push({ path, versions });
    }
  }
  found.sort((left, right) => left.path.localeCompare(right.path));
  return found;
}
