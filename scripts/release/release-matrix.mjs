import { pathToFileURL } from "node:url";

import { TARGETS } from "../../npm/lib/targets.mjs";

export function releaseMatrix(targets = TARGETS) {
  return {
    include: Object.values(targets).map((target) => ({
      runner: target.runner,
      target: target.rustTarget,
      binary: target.binaryName,
      libc: target.libc || "",
      platform: target.platform,
      python: target.platform === "win32" ? "python" : "python3",
    })),
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.stdout.write(JSON.stringify(releaseMatrix()));
}
