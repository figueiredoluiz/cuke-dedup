// Concrete replay deliberately loads neither the seed renderer nor its fixture metadata.
import { constants } from "node:fs";
import { open } from "node:fs/promises";
import { replayProject } from "./lib/corpus-replay.mjs";

/** Read a bounded regular JSON file from a single open handle. */
async function loadBundle(path) {
  const input = await open(path, constants.O_RDONLY | constants.O_NONBLOCK);
  try {
    const stat = await input.stat();
    if (!stat.isFile() || stat.size > 512 * 1024) throw new Error("expected a replay file of at most 512 KiB");
    const buffer = Buffer.alloc(stat.size);
    let offset = 0;
    while (offset < buffer.length) {
      const { bytesRead } = await input.read(buffer, offset, buffer.length - offset, null);
      if (!bytesRead) throw new Error("replay file changed while reading");
      offset += bytesRead;
    }
    if ((await input.stat()).size !== buffer.length) throw new Error("replay file changed while reading");
    return JSON.parse(buffer.toString("utf8"));
  } finally {
    await input.close();
  }
}

const [command, ...args] = process.argv.slice(2);
try {
  if (command === "render") {
    const { renderSeed } = await import("./lib/corpus-seed.mjs");
    console.log(JSON.stringify(renderSeed(args), null, 2));
  } else if (command === "replay" && args.length === 1) {
    console.log(await replayProject(await loadBundle(args[0])));
  } else {
    throw new Error("usage: corpus-seed.mjs render [operations...] | replay <bundle.json>");
  }
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
