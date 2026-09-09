import { readFileSync } from "node:fs";

const manifest = JSON.parse(
  readFileSync(new URL("../prebuilt-targets.json", import.meta.url), "utf8"),
);

const EXPECTED_TARGETS = Object.freeze({
  "darwin-arm64": Object.freeze({
    rustTarget: "aarch64-apple-darwin",
    packageName: "cuke-dedup-darwin-arm64",
    packageDirectory: "darwin-arm64",
    platform: "darwin",
    arch: "arm64",
    runner: "macos-15",
    binaryName: "cuke-dedup",
  }),
  "darwin-x64": Object.freeze({
    rustTarget: "x86_64-apple-darwin",
    packageName: "cuke-dedup-darwin-x64",
    packageDirectory: "darwin-x64",
    platform: "darwin",
    arch: "x64",
    runner: "macos-15-intel",
    binaryName: "cuke-dedup",
  }),
  "linux-arm64-gnu": Object.freeze({
    rustTarget: "aarch64-unknown-linux-gnu",
    packageName: "cuke-dedup-linux-arm64-gnu",
    packageDirectory: "linux-arm64-gnu",
    platform: "linux",
    arch: "arm64",
    libc: "gnu",
    runner: "ubuntu-22.04-arm",
    binaryName: "cuke-dedup",
  }),
  "linux-arm64-musl": Object.freeze({
    rustTarget: "aarch64-unknown-linux-musl",
    packageName: "cuke-dedup-linux-arm64-musl",
    packageDirectory: "linux-arm64-musl",
    platform: "linux",
    arch: "arm64",
    libc: "musl",
    runner: "ubuntu-22.04-arm",
    binaryName: "cuke-dedup",
  }),
  "linux-x64-gnu": Object.freeze({
    rustTarget: "x86_64-unknown-linux-gnu",
    packageName: "cuke-dedup-linux-x64-gnu",
    packageDirectory: "linux-x64-gnu",
    platform: "linux",
    arch: "x64",
    libc: "gnu",
    runner: "ubuntu-22.04",
    binaryName: "cuke-dedup",
  }),
  "linux-x64-musl": Object.freeze({
    rustTarget: "x86_64-unknown-linux-musl",
    packageName: "cuke-dedup-linux-x64-musl",
    packageDirectory: "linux-x64-musl",
    platform: "linux",
    arch: "x64",
    libc: "musl",
    runner: "ubuntu-22.04",
    binaryName: "cuke-dedup",
  }),
  "win32-arm64": Object.freeze({
    rustTarget: "aarch64-pc-windows-msvc",
    packageName: "cuke-dedup-windows-arm64-msvc",
    packageDirectory: "win32-arm64",
    platform: "win32",
    arch: "arm64",
    runner: "windows-11-arm",
    binaryName: "cuke-dedup.exe",
  }),
  "win32-x64": Object.freeze({
    rustTarget: "x86_64-pc-windows-msvc",
    packageName: "cuke-dedup-windows-x64-msvc",
    packageDirectory: "win32-x64",
    platform: "win32",
    arch: "x64",
    runner: "windows-latest",
    binaryName: "cuke-dedup.exe",
  }),
});

export function validateTargets(targets) {
  const actualKeys = Object.keys(targets ?? {}).sort();
  const expectedKeys = Object.keys(EXPECTED_TARGETS).sort();
  if (JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)) {
    throw new Error("prebuilt target keys do not match the supported release targets");
  }
  for (const key of expectedKeys) {
    validateTarget(key, targets[key]);
  }
  return targets;
}

export function validateTarget(key, target) {
  const expected = EXPECTED_TARGETS[key];
  if (!expected) {
    throw new Error(`unsupported release target ${key}`);
  }
  const actualKeys = Object.keys(target ?? {}).sort();
  const expectedKeys = Object.keys(expected).sort();
  if (
    JSON.stringify(actualKeys) !== JSON.stringify(expectedKeys)
    || expectedKeys.some((field) => target[field] !== expected[field])
  ) {
    throw new Error(`invalid release target metadata for ${key}`);
  }
  return target;
}

export const TARGETS = Object.freeze(Object.fromEntries(
  Object.entries(validateTargets(manifest.targets)).map(
    ([key, target]) => [key, Object.freeze({ ...target })],
  ),
));

export function targetFor(
  platform,
  arch,
  libc,
  targets = TARGETS,
) {
  const key = platform === "linux" ? `${platform}-${arch}-${libc}` : `${platform}-${arch}`;
  const target = targets[key];
  if (!target) {
    throw new Error(
      `unsupported platform ${key}; supported platforms: ${Object.keys(EXPECTED_TARGETS).join(", ")}`,
    );
  }
  return validateTarget(key, target);
}
