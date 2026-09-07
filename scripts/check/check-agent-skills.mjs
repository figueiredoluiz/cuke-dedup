import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";

const skillsRoot = resolve(process.argv[2] || "skills");
const entries = (await readdir(skillsRoot, { withFileTypes: true }))
  .filter((entry) => entry.isDirectory())
  .sort((left, right) => left.name.localeCompare(right.name));

assert.ok(entries.length > 0, "no Agent Skills found");

for (const entry of entries) {
  const skillPath = join(skillsRoot, entry.name, "SKILL.md");
  const source = (await readFile(skillPath, "utf8")).replaceAll("\r\n", "\n").replaceAll("\r", "\n");
  const match = source.match(/^---\n([\s\S]*?)\n---\n([\s\S]+)$/);
  assert.ok(match, `${skillPath}: expected YAML frontmatter and a non-empty body`);

  const metadata = Object.fromEntries(
    match[1]
      .split("\n")
      .filter(Boolean)
      .map((line) => {
        const separator = line.indexOf(":");
        assert.ok(separator > 0, `${skillPath}: malformed frontmatter line ${line}`);
        return [line.slice(0, separator).trim(), line.slice(separator + 1).trim()];
      }),
  );

  assert.match(metadata.name || "", /^[a-z0-9]+(?:-[a-z0-9]+)*$/, `${skillPath}: invalid name`);
  assert.equal(metadata.name, basename(entry.name), `${skillPath}: name must match its directory`);
  assert.ok(metadata.description, `${skillPath}: description is required`);
  assert.ok(metadata.description.length <= 500, `${skillPath}: description is too long`);
  assert.ok(match[2].trim(), `${skillPath}: instructions are required`);
}

console.log(`Validated ${entries.length} Agent Skill${entries.length === 1 ? "" : "s"}.`);
