// This helper uses its own working directory so each filesystem operation is relative to the directory already verified.
import { lstatSync, statSync, unlinkSync } from "node:fs";
import path from "node:path";

/** Removes the previous regular JSON report relative to a verified working directory. */
function prepareReport(directory) {
  const volume = path.parse(directory).root;
  if (!volume) throw new Error("expected an absolute report directory");
  const components = directory.slice(volume.length).split(path.sep).filter(Boolean);
  process.chdir(volume);
  // The native writer uses the physical system directories behind these Darwin volume aliases.
  if (process.platform === "darwin" && ["etc", "tmp", "var"].includes(components[0])) {
    process.chdir(`/private/${components.shift()}`);
  }
  for (const name of components) {
    if (name === ".") continue;
    const entry = lstatSync(name, { bigint: true, throwIfNoEntry: false });
    if (!entry) return;
    if (!entry.isDirectory() || entry.isSymbolicLink()) throw new Error("report parent must be a real directory");
    process.chdir(name);
    const opened = statSync(".", { bigint: true });
    if (!entry.ino || entry.dev !== opened.dev || entry.ino !== opened.ino) {
      throw new Error("report directory changed while opening it");
    }
  }
  const filename = "cuke-dedup.json";
  const entry = lstatSync(filename, { throwIfNoEntry: false });
  if (!entry) return;
  if (!entry.isFile()) {
    const error = new Error("JSON report destination must be a regular file");
    if (entry.isDirectory()) error.code = "EISDIR";
    throw error;
  }
  unlinkSync(filename);
}

try {
  prepareReport(process.argv[2]);
} catch (error) {
  process.stderr.write(JSON.stringify({ message: error.message, code: error.code }));
  process.exitCode = 2;
}
