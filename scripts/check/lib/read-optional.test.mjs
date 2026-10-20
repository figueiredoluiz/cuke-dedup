import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readOptional } from "./read-optional.mjs";

/** Checks a present file is read, a missing one is null and any other failure is not swallowed. */
test("readOptional returns null only for a missing file", async () => {
  const directory = await mkdtemp(join(tmpdir(), "read-optional-"));
  try {
    await writeFile(join(directory, "present"), "text");
    assert.equal(await readOptional(join(directory, "present")), "text");
    assert.equal(await readOptional(join(directory, "absent")), null);
    // A directory is not a missing file: reading it fails with another code.
    await assert.rejects(readOptional(directory), /EISDIR|EPERM|EACCES/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
