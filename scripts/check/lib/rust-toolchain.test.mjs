// rust-toolchain: pin and MSRV come from real TOML parsing; the pin must be the MSRV's release;
// the CLI prints the values workflows consume.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

import {
  DOC_STATEMENTS,
  PYTHON,
  docMismatches,
  matchPinToMsrv,
  msrvFloor,
  msrvFrom,
  pinFrom,
  readTomlFiles,
  readToolchain,
} from "./rust-toolchain.mjs";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const cli = join(repo, "scripts", "check", "check-rust-toolchain.mjs");
const lib = join(repo, "scripts", "check", "lib", "rust-toolchain.mjs");

// Child node drops NODE_* (NODE_OPTIONS preloads code); Python shims need HOME, `-I` drops PYTHON*.
const childEnv = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("NODE_")));

/** Parse one TOML file through the real reader. */
const readToml = (path, python = PYTHON) => readTomlFiles([path], python)[0];

/** Run `body(dir)` in a fresh temporary directory, removed afterwards. */
function inTemp(body) {
  const dir = mkdtempSync(join(tmpdir(), "rust-toolchain-"));
  try {
    return body(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const docPaths = [...new Set(DOC_STATEMENTS.map(({ path }) => path))];

/** Doc files stating `msrv` x.y (pin x.y.0) in every sentence DOC_STATEMENTS checks. */
function docFixture(msrv) {
  const pin = `${msrv}.0`;
  return {
    "README.md": `Building from source requires Rust ${msrv} or newer:\n\n- Building from source requires Rust ${msrv} or newer.\n`,
    "CONTRIBUTING.md": `| Rust (rustup) | \`${pin}\` pinned in \`rust-toolchain.toml\` | x |\n| Rust MSRV | \`${msrv}\` in \`Cargo.toml\` | x |\n\nrustup toolchain install ${pin} --profile minimal --component clippy\n`,
    "docs/development.md": `- pins the development and release compiler (\`${pin}\`).\n- declares the MSRV (\`${msrv}\`). CI tests the\n  MSRV as \`${pin}\` on Linux, macOS and Windows.\n`,
    "docs/safety-and-limitations.md": `- The minimum supported Rust version is ${msrv}.\n`,
  };
}

/** Write `files` ({relative path: text}) under `dir`. */
function writeFiles(dir, files) {
  for (const [path, text] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), text);
  }
}

/** Parse one TOML text through the real reader. */
const parse = (text) => inTemp((dir) => {
  writeFiles(dir, { "x.toml": text });
  return readToml(join(dir, "x.toml"));
});

/** TOML syntax the regexes this replaces got wrong or could not see: tomllib decides. */
test("readToml follows the TOML spec", () => {
  const rows = [
    ['[toolchain]\nchannel = "1.100.0" # pinned\n', "1.100.0", "trailing comment"],
    ['[toolchain]\r\nchannel = "1.100.0"\r\n', "1.100.0", "CRLF"],
    ["[toolchain]\nchannel = '1.100.0'\n", "1.100.0", "literal string"],
    ['[ toolchain ]\n  channel="1.100.0"\n', "1.100.0", "spaced header and key"],
    ['toolchain.channel = "1.100.0"\n', "1.100.0", "dotted key at the root"],
    ['[toolchain]\n"channel" = "1.100.0"\n', "1.100.0", "quoted key"],
    ['[other]\nchannel = "1.100.0"\n[toolchain]\nprofile = "minimal"\n', undefined, "channel in another table"],
    ['# channel = "1.100.0"\n[toolchain]\n', undefined, "commented-out channel"],
    ['\ufeff[toolchain]\nchannel = "1.100.0"\n', "1.100.0", "leading BOM, which Cargo and rustup accept"],
  ];
  for (const [text, channel, label] of rows) assert.equal(parse(text).toolchain?.channel, channel, label);
  assert.equal(parse('[package]\nrust-version = "1.100" # MSRV\n').package["rust-version"], "1.100");
  assert.deepEqual(parse("[package]\nrust-version.workspace = true\n").package["rust-version"], { workspace: true });
  assert.equal(parse('[workspace.package]\nrust-version = "1.100"\n').package, undefined, "workspace table is not [package]");
  assert.deepEqual(parse("[package.metadata.x]\nlimit = inf\nratio = nan\n").package.metadata.x, { limit: [null, "float", "inf"], ratio: [null, "float", "nan"] }, "inf/nan have no JSON form");
  assert.deepEqual(parse("[toolchain]\nchannel = 1.100\n").toolchain.channel, [null, "float", "1.100"], "a float pin stays a non-string");
  assert.deepEqual(parse("[package]\nrust-version = 1.100\n").package["rust-version"], [null, "float", "1.100"]);
  assert.deepEqual(parse("[toolchain]\nchannel = 2026-11-01\n").toolchain.channel, [null, "date-time", "2026-11-01"], "a date pin stays a non-string");
  assert.deepEqual(parse('[toolchain]\nchannel = { float = "1.100" }\n').toolchain.channel, { float: "1.100" }, "a table is never read as a tag");
});

/** Invalid TOML and a missing interpreter fail with their cause, never as "no channel". */
test("readToml fails closed", () => {
  assert.throws(() => parse('[toolchain]\nchannel = "1.100.0"\nchannel = "1.99.0"\n'), /python3? failed reading .*x\.toml \(exit 1\): .*x\.toml: TOMLDecodeError: /, "duplicate key");
  assert.throws(() => parse('[toolchain\nchannel = "1.100.0"\n'), /python3? failed reading .*x\.toml \(exit 1\): .*x\.toml: TOMLDecodeError: .*line 1/);
  assert.throws(() => inTemp((dir) => {
    writeFileSync(join(dir, "x.toml"), Buffer.from([0xff, 0xfe, 0x5b, 0x00]));
    return readToml(join(dir, "x.toml"));
  }), /\(exit 1\): .*x\.toml: UnicodeDecodeError: /, "non-UTF-8 file named");
  assert.throws(() => readToml(join(repo, "no-such.toml")), /python3? failed reading .*no-such\.toml \(exit 1\): .*no-such\.toml: FileNotFoundError: /);
  assert.throws(() => readToml(join(repo, "Cargo.toml"), join(repo, "no-such-python")), /cannot run .*no-such-python to read .*Cargo\.toml: .*ENOENT/);
  assert.deepEqual(readToml(join(repo, "Cargo.toml")).package.name, "cuke-dedup", "control: the real manifest parses");
});

/** TOML 1.1, which Cargo reads: Python 3.15+ parses it, an older tomllib's error names its dialect; no other error does. */
test("readToml names the TOML dialect", () => inTemp((dir) => {
  writeFiles(dir, { "x.toml": "a = { b = 1,\n}\n", "bad.toml": "a = \n" });
  /** The interpreter's Python 3 minor version, or undefined when it does not run. */
  const minor = (python) => {
    const result = spawnSync(python, ["-I", "-c", "import sys; print(sys.version_info[1])"], { encoding: "utf8", env: childEnv });
    return result.status === 0 ? Number(result.stdout) : undefined;
  };
  const found = [PYTHON, "python3.11", "python3.12", "python3.13", "python3.14", "python3.15"].filter((python) => minor(python) !== undefined);
  assert.ok(found.length > 0, "no interpreter ran");
  /** An error of `kind` without the dialect hint. */
  const noHint = (kind) => (error) => error.message.includes(`${kind}: `) && !error.message.includes("reads TOML 1.0");
  for (const python of found) {
    const version = minor(python);
    assert.throws(() => readToml(join(dir, "missing.toml"), python), noHint("FileNotFoundError"), python);
    if (version >= 15) {
      assert.deepEqual(readToml(join(dir, "x.toml"), python), { a: { b: 1 } }, python);
      assert.throws(() => readToml(join(dir, "bad.toml"), python), noHint("TOMLDecodeError"), python);
    } else {
      assert.throws(() => readToml(join(dir, "x.toml"), python), new RegExp(`TOMLDecodeError: .*\\(Python 3\\.${version} reads TOML 1\\.0; Cargo reads TOML 1\\.1, which needs Python 3\\.15\\)$`), python);
    }
  }
}));

/** One launch reads several files, in order; a bad second file names that file. */
test("readTomlFiles", () => inTemp((dir) => {
  writeFiles(dir, { "a.toml": 'a = "1"\n', "b.toml": "b = 2\n", "bad.toml": "b = \n" });
  assert.deepEqual(readTomlFiles([join(dir, "a.toml"), join(dir, "b.toml")]), [{ a: "1" }, { b: 2 }]);
  assert.throws(() => readTomlFiles([join(dir, "a.toml"), join(dir, "bad.toml")]), /\(exit 1\): .*bad\.toml: TOMLDecodeError: /);
}));

/** Interpreter failures carry their own cause; a shim that needs HOME still runs (POSIX shims). */
test("readToml through interpreter wrappers", { skip: process.platform === "win32" && "POSIX shell shims" }, () => inTemp((dir) => {
  /** Write an executable `/bin/sh` script `name` with `body`; returns its path. */
  const shim = (name, body) => {
    writeFiles(dir, { [name]: `#!/bin/sh\n${body}\n` });
    chmodSync(join(dir, name), 0o755);
    return join(dir, name);
  };
  assert.throws(() => readToml(join(repo, "Cargo.toml"), shim("silent", "exit 9")), /silent failed reading .*Cargo\.toml \(exit 9\): no output on stderr$/);
  assert.throws(() => readToml(join(repo, "Cargo.toml"), shim("quiet", "exit 0")), /quiet printed nothing for .*Cargo\.toml$/);
  assert.throws(() => readToml(join(repo, "Cargo.toml"), shim("stub", "echo 'Python was not found' >&2; exit 109")), /\(exit 109\): Python was not found$/);
  assert.ok(process.env.HOME, "precondition: HOME is set");
  const needsHome = shim("asdf-python", `[ -n "$HOME" ] || { echo "shim: HOME is not set" >&2; exit 3; }\nexec ${PYTHON} "$@"`);
  assert.equal(readToml(join(repo, "Cargo.toml"), needsHome).package.name, "cuke-dedup", "the shim sees HOME");
}));

/** -I: a tomllib.py in the working directory or on PYTHONPATH never replaces the standard parser. */
test("readTomlFiles ignores planted modules", () => inTemp((dir) => {
  writeFiles(dir, { "tomllib.py": 'raise SystemExit("planted tomllib")\n', "json.py": 'raise SystemExit("planted json")\n' });
  const code = "const m = await import(process.argv[1]); console.log(m.readTomlFiles([process.argv[2]])[0].package.name);";
  const result = spawnSync(process.execPath, ["--input-type=module", "-e", code, pathToFileURL(lib).href, join(repo, "Cargo.toml")],
    { cwd: dir, encoding: "utf8", env: { ...childEnv, PYTHONPATH: dir } });
  assert.deepEqual([result.error, result.status, result.stdout], [undefined, 0, "cuke-dedup\n"], result.stderr);
}));

/** The pin is an exact x.y.z release; channel names, host suffixes and floats are refused. */
test("pinFrom", () => {
  assert.deepEqual(["1.100.0", "0.0.0"].map((channel) => pinFrom({ toolchain: { channel } })), ["1.100.0", "0.0.0"]);
  for (const channel of ["1.100", "stable", "nightly-2026-11-01", "1.100.0-aarch64-apple-darwin", "01.100.0", "1.100.0 ", "", 1.1]) {
    const message = `rust-toolchain.toml channel must be an exact Rust release such as "1.100.0", got ${JSON.stringify(channel)}`;
    assert.throws(() => pinFrom({ toolchain: { channel } }), (error) => error.message === message, String(channel));
  }
  assert.throws(() => pinFrom({ toolchain: { channel: [null, "float", "1.100"] } }), /got float 1\.100$/);
  assert.throws(() => pinFrom({ toolchain: { channel: [null, "date-time", "2026-11-01"] } }), /got date-time 2026-11-01$/);
  assert.throws(() => pinFrom({ toolchain: { channel: { float: "1.100" } } }), /got \{"float":"1\.100"\}$/, "a table is not a float");
  for (const [toolchain, got] of [[[{ channel: "1.100.0" }], 'array [{"channel":"1.100.0"}]'], [[], "array []"], ["1.100.0", '"1.100.0"']]) {
    assert.throws(() => pinFrom({ toolchain }), { message: `rust-toolchain.toml [toolchain] must be a table, got ${got}` }, got);
  }
  for (const toolchain of [{ path: "/opt/rust" }, { channel: "1.100.0", path: "/opt/rust" }]) {
    assert.throws(() => pinFrom({ toolchain }), /rust-toolchain\.toml sets \[toolchain\] path; pin a release with channel only/, JSON.stringify(toolchain));
  }
  assert.throws(() => pinFrom({ toolchain: { profile: "minimal" } }), /rust-toolchain\.toml has no \[toolchain\] channel/);
  assert.throws(() => pinFrom({}), /has no \[toolchain\] channel/);
});

/** The MSRV is Cargo's two- or three-part form; inheritance, operators and pre-releases are refused. */
test("msrvFrom and msrvFloor", () => {
  assert.deepEqual(["1.100", "1.100.2"].map((version) => msrvFrom({ package: { "rust-version": version } })), ["1.100", "1.100.2"]);
  for (const version of ["1", "^1.100", "1.100.0-beta.1", "1.100.0.1", "1.0100", { workspace: true }, 1.1]) {
    assert.throws(() => msrvFrom({ package: { "rust-version": version } }), /Cargo\.toml rust-version must be "x\.y" or "x\.y\.z", got /, JSON.stringify(version));
  }
  assert.throws(() => msrvFrom({ package: { name: "x" } }), /Cargo\.toml has no \[package\] rust-version/);
  assert.throws(() => msrvFrom({ package: [{ "rust-version": "1.100" }] }), { message: 'Cargo.toml [package] must be a table, got array [{"rust-version":"1.100"}]' });
  assert.deepEqual(["1.100", "1.100.2", "0.0"].map(msrvFloor), ["1.100.0", "1.100.2", "0.0.0"]);
  assert.throws(() => msrvFloor("stable"), /not a rust-version: "stable"/);
});

/** Equal releases pass; a different minor or patch on either side fails. */
test("matchPinToMsrv", () => {
  const rows = [["1.100.0", "1.100", true], ["1.100.2", "1.100.2", true], ["1.100.0", "1.100.0", true],
    ["1.101.0", "1.100", false], ["1.100.1", "1.100", false], ["1.100.0", "1.100.2", false], ["1.99.0", "1.100", false]];
  for (const [pin, msrv, ok] of rows) {
    if (ok) assert.deepEqual(matchPinToMsrv(pin, msrv), { pin, msrv, floor: pin }, `${pin} ${msrv}`);
    else assert.throws(() => matchPinToMsrv(pin, msrv), new RegExp(`pins ${pin.replaceAll(".", "\\.")} but Cargo\\.toml rust-version ${msrv.replaceAll(".", "\\.")} means ${msrvFloor(msrv).replaceAll(".", "\\.")}`), `${pin} ${msrv}`);
  }
});

/** readToolchain reads both files and refuses a legacy `rust-toolchain` file that would win. */
test("readToolchain", () => inTemp((dir) => {
  writeFiles(dir, { "rust-toolchain.toml": '[toolchain]\nchannel = "1.100.0"\n', "Cargo.toml": '[package]\nname = "x"\nrust-version = "1.100"\n' });
  assert.deepEqual(readToolchain(dir), { pin: "1.100.0", msrv: "1.100", floor: "1.100.0" });
  const launches = [];
  readToolchain(dir, (paths) => (launches.push(paths), readTomlFiles(paths)));
  assert.deepEqual(launches, [[join(dir, "rust-toolchain.toml"), join(dir, "Cargo.toml")]], "one launch for both files");
  writeFiles(dir, { "Cargo.toml": '[package]\nname = "x"\nrust-version = "1.101"\n' });
  assert.throws(() => readToolchain(dir), { message: `${join(dir, "rust-toolchain.toml")} pins 1.100.0 but ${join(dir, "Cargo.toml")} rust-version 1.101 means 1.101.0; upgrade both together` });
  writeFiles(dir, { "rust-toolchain.toml": "[toolchain]\nchannel = 1.100\n" });
  assert.throws(() => readToolchain(dir), { message: `${join(dir, "rust-toolchain.toml")} channel must be an exact Rust release such as "1.100.0", got float 1.100` });
  writeFiles(dir, { "rust-toolchain.toml": '[toolchain]\nchannel = "1.100.0"\n' });
  writeFiles(dir, { "Cargo.toml": '[package]\nname = "x"\nrust-version = "1.100"\n', "rust-toolchain": "1.100.0\n" });
  assert.throws(() => readToolchain(dir), { message: `${join(dir, "rust-toolchain")} exists; rustup prefers it over rust-toolchain.toml, so remove it` });
}));

/** Docs: the checked-in docs agree, also under CRLF or reflowed; a stale value, a reworded sentence or an unreadable file is reported. */
test("docMismatches", () => {
  const toolchain = readToolchain(repo);
  const docs = Object.fromEntries(docPaths.map((path) => [path, readFileSync(join(repo, path), "utf8")]));
  /** Reader over the checked-in docs with per-path replacements; an Error value throws. */
  const read = (overrides = {}) => (path) => {
    const text = overrides[path] ?? docs[path];
    if (text instanceof Error) throw text;
    return text;
  };
  /** Mismatches of `values` over the checked-in docs with `overrides`. */
  const check = (values, overrides) => docMismatches(values, repo, read(overrides));
  assert.deepEqual(check(toolchain), [], "control: the checked-in docs agree");
  const crlf = Object.fromEntries(docPaths.map((path) => [path, docs[path].replace(/\r?\n/g, "\r\n")]));
  assert.deepEqual(check(toolchain, crlf), [], "CRLF checkout");
  const reflowed = Object.fromEntries(docPaths.map((path) => [path, docs[path]
    .replace(/(Building|minimum|compiler|declares|on Linux,) /g, "$1\n  ")]));
  assert.notDeepEqual(reflowed, docs, "the reflow changed the docs");
  assert.deepEqual(check(toolchain, reflowed), [], "reflowed paragraphs");
  const next = { pin: "9.9.0", msrv: "9.9", floor: "9.9.0" };
  const stale = DOC_STATEMENTS.flatMap(({ path, value, count }) =>
    Array(count).fill(`${join(repo, path)}: states ${toolchain[value]}, ${value} is ${next[value]}`));
  assert.deepEqual(check(next), stale, "one report per stated value, in statement order");
  const safety = "docs/safety-and-limitations.md";
  assert.deepEqual(check(toolchain, { [safety]: docs[safety].replace("The minimum supported Rust version is", "MSRV:") }),
    [`${join(repo, safety)}: expected 1 statement(s) matching ${DOC_STATEMENTS.at(-1).pattern}, found 0`]);
  const readme = "README.md";
  assert.deepEqual(check(toolchain, { [readme]: docs[readme].replace(`requires Rust ${toolchain.msrv} or newer`, "requires Rust 1.99 or newer") }),
    [`${join(repo, readme)}: states 1.99, msrv is ${toolchain.msrv}`]);
  const missing = Object.assign(new Error("gone"), { code: "ENOENT" });
  const contributing = "CONTRIBUTING.md";
  assert.deepEqual(check(toolchain, { [readme]: missing, [contributing]: docs[contributing].replace(`\`${toolchain.msrv}\` in`, "`1.99` in") }),
    [`${join(repo, readme)}: cannot read (ENOENT)`, `${join(repo, contributing)}: states 1.99, msrv is ${toolchain.msrv}`], "a missing file does not hide another file's mismatch");
  let reads = 0;
  docMismatches(toolchain, repo, (path) => (reads++, docs[path]));
  assert.equal(reads, docPaths.length, "each file read once");
});

/** CLI: check, print and usage on fixed fixtures (not the repo's live values); exit codes 0, 1 and 2. */
test("cli", () => inTemp((dir) => {
  writeFiles(dir, { "rust-toolchain.toml": '[toolchain]\nchannel = "1.42.0"\n', "Cargo.toml": '[package]\nname = "x"\nrust-version = "1.42"\n', ...docFixture("1.42") });
  /** Run the CLI with `args`. */
  const run = (...args) => spawnSync(process.execPath, [cli, ...args], { encoding: "utf8", env: childEnv });
  /** Run the CLI through a symlink, as a linked or vendored checkout would. */
  const runLinked = (...args) => {
    symlinkSync(cli, join(dir, "linked.mjs"));
    try {
      return spawnSync(process.execPath, [join(dir, "linked.mjs"), ...args], { encoding: "utf8", env: childEnv });
    } finally {
      rmSync(join(dir, "linked.mjs"));
    }
  };
  let result = run("--root", dir);
  assert.deepEqual([result.status, result.stdout], [0, "rust toolchain pin 1.42.0 matches MSRV 1.42; docs agree\n"], result.stderr);
  result = run(`--root=${dir}`, "--print", "pin");
  assert.deepEqual([result.status, result.stdout], [0, "1.42.0\n"]);
  result = run("--print", "msrv", "--root", dir);
  assert.deepEqual([result.status, result.stdout], [0, "1.42.0\n"]);
  writeFiles(dir, { "rust-toolchain.toml": '[toolchain]\nchannel = "1.43.0"\n' });
  const mismatch = `rust toolchain: ${join(dir, "rust-toolchain.toml")} pins 1.43.0 but ${join(dir, "Cargo.toml")} rust-version 1.42 means 1.42.0; upgrade both together\n`;
  for (const args of [[], ["--print", "pin"], ["--print", "msrv"]]) {
    result = run("--root", dir, ...args);
    assert.deepEqual([result.status, result.stdout, result.stderr], [1, "", mismatch], `nothing printed for a workflow to consume: ${args.join(" ")}`);
  }
  if (process.platform !== "win32") {
    result = runLinked("--root", dir);
    assert.deepEqual([result.status, result.stdout, result.stderr], [1, "", mismatch], "a symlinked CLI still checks");
  }
  writeFiles(dir, { "rust-toolchain.toml": '[toolchain]\nchannel = "1.42.0"\n', "docs/safety-and-limitations.md": "- The minimum supported Rust version is 1.41.\n" });
  result = run("--root", dir);
  const staleSafety = `rust toolchain: ${join(dir, "docs/safety-and-limitations.md")}: states 1.41, msrv is 1.42\n`;
  assert.deepEqual([result.status, result.stdout, result.stderr], [1, "", staleSafety], "a stale doc fails the check");
  for (const [print, value] of [["pin", "1.42.0\n"], ["msrv", "1.42.0\n"]]) {
    result = run("--root", dir, "--print", print);
    assert.deepEqual([result.status, result.stdout, result.stderr], [0, value, ""], `--print ${print} checks pin and MSRV only`);
  }
  rmSync(join(dir, "README.md"));
  result = run("--root", dir);
  assert.deepEqual([result.status, result.stdout, result.stderr], [1, "", `rust toolchain: ${join(dir, "README.md")}: cannot read (ENOENT)\n${staleSafety}`],
    "missing file and the other file's mismatch both reported");
  for (const args of [["--root"], ["--root", "--print"], ["--print", "--root"], ["--print", "version"], ["--root", dir, "--root", dir], ["--print", "pin", "--print", "msrv"], ["--bogus", "x"], ["pin"]]) {
    result = run(...args);
    assert.deepEqual([result.status, result.stdout], [2, ""], args.join(" "));
    assert.equal(result.stderr, "usage: check-rust-toolchain.mjs [--root DIR] [--print pin|msrv]\n");
  }
}));
