import { readFile } from "node:fs/promises";

/**
 * Reads a UTF-8 file a run may not have written.
 * @param {string} path File to read.
 * @returns {Promise<string|null>} The contents, or `null` when the file does not exist; any other
 *   failure rethrows.
 */
export async function readOptional(path) {
  return readFile(path, "utf8").catch((error) => {
    if (error.code === "ENOENT") return null;
    throw error;
  });
}
