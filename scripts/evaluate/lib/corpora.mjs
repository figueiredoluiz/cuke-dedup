// Loads every labelled corpus as flat lists of cases, so a report can count them side by side. The
// recall and behavior corpora and the paired language-parity corpus list their cases; the Ruby parity
// corpus lists group oracles across its manifest and its group manifests.

import { readFile } from "node:fs/promises";
import { join } from "node:path";

/** A parsed JSON file under the fixtures directory. */
async function load(fixtures, path) {
  return JSON.parse(await readFile(join(fixtures, path), "utf8"));
}

/**
 * Every corpus's labelled cases.
 * @param {string} [fixtures] The fixtures directory.
 * @returns {Promise<{name: string, items: object[]}[]>} Corpora in a fixed order: recall, behavior,
 *   language-parity, ruby-parity. Each Ruby group carries its name beside its oracle.
 */
export async function loadCorpora(fixtures = "fixtures") {
  const rubyManifest = await load(fixtures, "ruby-parity/manifest.json");
  const groups = Object.entries(rubyManifest.groupOracles).map(([name, group]) => ({ name, ...group }));
  for (const file of rubyManifest.groupManifests ?? []) {
    const extension = await load(fixtures, `ruby-parity/${file.split("/").pop()}`);
    groups.push(...Object.entries(extension.groupOracles).map(([name, group]) => ({ name, ...group })));
  }
  return [
    { name: "recall", items: (await load(fixtures, "recall/manifest.json")).cases },
    { name: "behavior", items: (await load(fixtures, "corpus/manifest.json")).cases },
    { name: "language-parity", items: (await load(fixtures, "language-parity/manifest.json")).cases },
    { name: "ruby-parity", items: groups },
  ];
}
