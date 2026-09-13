// Shared discovery for the one class of version site that cannot be enumerated from a manifest.
// Every other site is structural: two Cargo manifests, two Cargo lockfiles, the npm manifests
// named by `npm/prebuilt-targets.json`, the npm lockfile, and two workflow inputs. Action pins in
// prose are free-form, so a new guide can introduce one at any time. `bump-version.mjs` rewrites
// whatever this finds and `check-release-version.mjs` verifies the same set, which keeps a pin in
// a file neither script knew about from going stale unnoticed.
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

/// Returns every file carrying at least one Action pin, with the versions it pins, sorted by path
/// so callers report deterministically.
export async function findActionPins(root = ".") {
  const found = [];
  for await (const path of searchableFiles(root)) {
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
