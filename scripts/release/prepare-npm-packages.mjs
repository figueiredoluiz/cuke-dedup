import { chmod, copyFile, mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";

const artifacts = process.argv[2];
if (!artifacts) {
  throw new Error("usage: node scripts/release/prepare-npm-packages.mjs ARTIFACTS_DIR|--check");
}

const manifest = JSON.parse(
  await readFile(new URL("../../npm/prebuilt-targets.json", import.meta.url), "utf8"),
);

for (const target of artifacts === "--check" ? [] : Object.values(manifest.targets)) {
  const { rustTarget, packageDirectory, binaryName } = target;
  const destinationDirectory = join(
    "npm",
    "platforms",
    packageDirectory,
    "bin",
  );
  const destination = join(destinationDirectory, binaryName);
  await mkdir(destinationDirectory, { recursive: true });
  await copyFile(join(artifacts, `binary-${rustTarget}`, binaryName), destination);
  if (binaryName !== "cuke-dedup.exe") {
    await chmod(destination, 0o755);
  }
}
