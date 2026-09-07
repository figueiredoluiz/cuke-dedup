import assert from "node:assert/strict";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

test("skill validation accepts Windows CRLF line endings", async () => {
  const skillsRoot = await mkdtemp(join(tmpdir(), "cuke-dedup-skills-"));
  const skillRoot = join(skillsRoot, "example-skill");
  await mkdir(skillRoot);
  await writeFile(
    join(skillRoot, "SKILL.md"),
    "---\r\nname: example-skill\r\ndescription: Example skill\r\n---\r\n\r\nRun the example.\r\n",
  );

  const result = spawnSync(
    process.execPath,
    [resolve("scripts/check/check-agent-skills.mjs"), skillsRoot],
    { encoding: "utf8" },
  );

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Validated 1 Agent Skill\./);
});
