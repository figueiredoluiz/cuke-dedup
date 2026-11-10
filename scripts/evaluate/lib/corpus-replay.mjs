import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

/** Hash the serialized concrete project, including ownership, applicability and invocation metadata. */
function digest(project) {
  return createHash("sha256").update(JSON.stringify(project)).digest("hex");
}

/** Build a portable input artifact. Replay consumes this object without any model or generator. */
export function sealProject(project) {
  return { schemaVersion: 1, project, sha256: digest(project) };
}

/** Restrict replay to portable input files inside a fresh directory, including on case-insensitive filesystems. */
function validateFiles(files) {
  assert.ok(files && typeof files === "object" && !Array.isArray(files), "missing project files");
  const entries = Object.entries(files);
  assert.ok(entries.length > 0 && entries.length <= 64, "project file count outside budget");
  let bytes = 0;
  for (const [name, contents] of entries) {
    assert.ok(name === ".cuke-dedup.json" || /^(?:(?:steps|support)\/)?[a-z][a-z0-9-]{0,63}\.(?:feature|mjs|cjs)$/u.test(name), "unsupported project path");
    assert.ok(!/^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])\./i.test(name.split("/").at(-1)), "reserved filename");
    assert.equal(typeof contents, "string", "project files must contain text");
    bytes += Buffer.byteLength(contents);
    assert.ok(bytes <= 64 * 1024, "project exceeds source byte budget");
    assert.equal(Buffer.from(contents).toString("utf8"), contents, "project text must round-trip through UTF-8");
  }
}

/** Restore exact UTF-8 input bytes. On a partial write remove only the temporary directory allocated here. */
export async function replayProject(bundle) {
  assert.equal(bundle.schemaVersion, 1, "unsupported replay version");
  assert.equal(bundle.sha256, digest(bundle.project), "project digest mismatch");
  validateFiles(bundle.project.files);
  const root = await mkdtemp(join(tmpdir(), "cuke-seed-"));
  try {
    for (const [name, contents] of Object.entries(bundle.project.files)) {
      const path = join(root, name);
      await mkdir(dirname(path), { recursive: true });
      await writeFile(path, contents, { flag: "wx" });
    }
    return root;
  } catch (error) {
    await rm(root, { recursive: true, force: true });
    throw error;
  }
}
