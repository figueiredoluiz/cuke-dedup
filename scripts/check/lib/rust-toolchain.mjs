// Rust toolchain pin (`rust-toolchain.toml`) and MSRV (`Cargo.toml` `rust-version`): one reader
// for CI, release and `check:versions`. Python's `tomllib` parses the TOML, so comments, CRLF,
// quoting, dotted keys and table scoping follow the TOML spec instead of a regex.
import { existsSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { parseArgs } from "node:util";

// rustup reads `rust-toolchain` before `rust-toolchain.toml` when both exist.
const LEGACY_TOOLCHAIN_FILE = "rust-toolchain";
const TOOLCHAIN_FILE = "rust-toolchain.toml";
const MANIFEST_FILE = "Cargo.toml";

const EXACT = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
// Cargo accepts two or three numeric components, no operators or pre-release tags.
const RUST_VERSION = /^(0|[1-9]\d*)\.(0|[1-9]\d*)(?:\.(0|[1-9]\d*))?$/;
// One launch reads every file. Floats and dates become [null, type, text]: JSON has no inf/nan or
// dates, and TOML has no null, so no value in the file can produce that array; a float or date pin
// (`channel = 1.100`) stays a non-string and is refused. ValueError covers TOMLDecodeError and
// UnicodeDecodeError (a non-UTF-8 file). No f-strings: Python 2 and 3.5 must still reach the
// version message.
const PARSE_TOML = `import json, sys
try:
    import tomllib
except ImportError:
    sys.exit("Python 3.11 or later is required (no tomllib)")
class Float:
    def __init__(self, text):
        self.text = text
def encode(value):
    return [None, "float", value.text] if isinstance(value, Float) else [None, "date-time", value.isoformat()]
documents = []
for path in sys.argv[1:]:
    try:
        with open(path, "rb") as handle:
            documents.append(tomllib.load(handle, parse_float=Float))
    except (OSError, ValueError) as error:
        sys.exit("%s: %s: %s" % (path, type(error).__name__, error))
json.dump(documents, sys.stdout, default=encode)`;

/** The interpreter that runs PARSE_TOML: `python` on Windows, `python3` elsewhere. */
export const PYTHON = process.platform === "win32" ? "python" : "python3";

/** Parse TOML files with Python's standard library in one launch; throws on any launch, read or parse error. */
export function readTomlFiles(paths, python = PYTHON) {
  const result = spawnSync(python, ["-I", "-c", PARSE_TOML, ...paths], {
    encoding: "utf8",
    // pattern-ok: -I makes Python ignore every PYTHON* variable; version-manager shims (asdf, mise,
    // pyenv) need HOME and their own variables to find the interpreter.
    env: process.env,
    timeout: 30_000,
    maxBuffer: 1024 * 1024,
  });
  if (result.error) throw new Error(`cannot run ${python} to read ${paths.join(", ")}: ${result.error.message}`);
  if (result.status !== 0) {
    // The last stderr line names the cause: a file's TOML or OS error, no tomllib, or the interpreter's own failure.
    const cause = result.stderr.trim().split(/\r?\n/).pop() || "no output on stderr";
    throw new Error(`${python} failed reading ${paths.join(", ")} (exit ${result.status ?? result.signal}): ${cause}`);
  }
  if (!result.stdout) throw new Error(`${python} printed nothing for ${paths.join(", ")}`);
  return JSON.parse(result.stdout);
}

/** A TOML value for a message: floats and dates by type, arrays named, the rest as JSON. */
function describe(value) {
  if (Array.isArray(value) && value[0] === null) return `${value[1]} ${value[2]}`;
  return Array.isArray(value) ? `array ${JSON.stringify(value)}` : JSON.stringify(value);
}

/** `document[key]` when it is a table or absent; an array (`[[key]]`) or a scalar throws. */
function tableOf(document, key, path) {
  const value = document?.[key];
  if (value !== undefined && (value === null || typeof value !== "object" || Array.isArray(value))) {
    throw new Error(`${path} [${key}] must be a table, got ${describe(value)}`);
  }
  return value;
}

/** The pinned channel: `[toolchain] channel`, which must be an exact `x.y.z` release. */
export function pinFrom(toolchain, path = TOOLCHAIN_FILE) {
  const table = tableOf(toolchain, "toolchain", path);
  // rustup refuses `path` beside `channel` and builds nothing from a pin with it.
  if (table?.path !== undefined) throw new Error(`${path} sets [toolchain] path; pin a release with channel only`);
  const channel = table?.channel;
  if (channel === undefined) throw new Error(`${path} has no [toolchain] channel`);
  if (typeof channel !== "string" || !EXACT.test(channel)) {
    throw new Error(`${path} channel must be an exact Rust release such as "1.100.0", got ${describe(channel)}`);
  }
  return channel;
}

/** The declared MSRV: `[package] rust-version`, as written (`x.y` or `x.y.z`). */
export function msrvFrom(manifest, path = MANIFEST_FILE) {
  const version = tableOf(manifest, "package", path)?.["rust-version"];
  if (version === undefined) throw new Error(`${path} has no [package] rust-version`);
  if (typeof version !== "string" || !RUST_VERSION.test(version)) {
    throw new Error(`${path} rust-version must be "x.y" or "x.y.z", got ${describe(version)}`);
  }
  return version;
}

/** The release an MSRV names: `1.100` is `1.100.0`, the floor CI tests. */
export function msrvFloor(msrv) {
  const [, major, minor, patch = "0"] = RUST_VERSION.exec(msrv) ?? [];
  if (major === undefined) throw new Error(`not a rust-version: ${JSON.stringify(msrv)}`);
  return `${major}.${minor}.${patch}`;
}

/** `{pin, msrv, floor}` when the pin is the MSRV's release; throws otherwise. */
export function matchPinToMsrv(pin, msrv, toolchainPath = TOOLCHAIN_FILE, manifestPath = MANIFEST_FILE) {
  const floor = msrvFloor(msrv);
  if (pin !== floor) {
    throw new Error(`${toolchainPath} pins ${pin} but ${manifestPath} rust-version ${msrv} means ${floor}; upgrade both together`);
  }
  return { pin, msrv, floor };
}

/** Read and cross-check both files under `root`: `{pin, msrv, floor}`. */
export function readToolchain(root = ".", readFiles = readTomlFiles) {
  const [legacy, toolchainPath, manifestPath] = [LEGACY_TOOLCHAIN_FILE, TOOLCHAIN_FILE, MANIFEST_FILE].map((file) => join(root, file));
  if (existsSync(legacy)) throw new Error(`${legacy} exists; rustup prefers it over ${TOOLCHAIN_FILE}, so remove it`);
  const [toolchain, manifest] = readFiles([toolchainPath, manifestPath]);
  return matchPinToMsrv(pinFrom(toolchain, toolchainPath), msrvFrom(manifest, manifestPath), toolchainPath, manifestPath);
}

const PRINT = { pin: "pin", msrv: "floor" };
const USAGE = "usage: check-rust-toolchain.mjs [--root DIR] [--print pin|msrv]";

/** Parse `[--root DIR] [--print pin|msrv]`; null on bad usage. */
function parseOptions(args) {
  try {
    const options = { root: { type: "string" }, print: { type: "string" } };
    const { values, tokens } = parseArgs({ args, options, strict: true, tokens: true });
    const names = tokens.filter(({ kind }) => kind === "option").map(({ name }) => name);
    // util.parseArgs keeps the last of a repeated option; a repeat is a contradiction, not an override.
    if (new Set(names).size !== names.length) return null;
    if (values.print !== undefined && !Object.hasOwn(PRINT, values.print)) return null;
    return { root: values.root ?? ".", print: values.print };
  } catch {
    // util.parseArgs throws only ERR_PARSE_ARGS_* for bad usage.
    return null;
  }
}

/** CLI entry for check-rust-toolchain.mjs; returns the exit code: 0 agree, 1 disagree or unreadable, 2 usage. */
export function main(argv) {
  const options = parseOptions(argv);
  if (!options) {
    console.error(USAGE);
    return 2;
  }
  try {
    const toolchain = readToolchain(options.root);
    console.log(options.print ? toolchain[PRINT[options.print]] : `rust toolchain pin ${toolchain.pin} matches MSRV ${toolchain.msrv}`);
    return 0;
  } catch (error) {
    console.error(`rust toolchain: ${error.message}`);
    return 1;
  }
}
