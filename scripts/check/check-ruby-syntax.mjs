// Every Ruby source must compile under the Ruby the adapter's semantics are checked against, except
// listed sources that must be rejected for a stated reason.
// tree-sitter-ruby accepts some sources Ruby rejects, so analyzer observations alone cannot show
// that a source is valid Ruby. `compile_file` also compiles regex literals with Onigmo, which
// `ruby -c` does not. Compiling never runs the code.
//
// Usage:
//   check-ruby-syntax.mjs                    tracked and untracked non-ignored `.rb` files
//   check-ruby-syntax.mjs --captured <dir>   sources the test suite fed the analyzer, recorded
//                                            with the `ruby-source-capture` feature
import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { capturedEntries, judge, parseRejections } from "./lib/ruby-syntax.mjs";

const minimumMajor = 4;
const capturedRejections = path.join(path.dirname(fileURLToPath(import.meta.url)), "ruby-test-source-rejections.json");
// Fixtures that pin the analyzer's result on source Ruby rejects, each with the parser message its
// SyntaxError must contain, so an unrelated error cannot stand in for the one the fixture pins.
const trackedRejections = new Map([
  ["fixtures/ruby-parity/regex-braces/leading-bracket/steps.rb", "empty char-class"],
  ["fixtures/ruby-syntax/regex-literal/steps.rb", "empty range in char class"],
]);

/** NUL-separated `git ls-files` output for Ruby sources, with the given selection options. */
const listRuby = (...options) =>
  // pattern-ok: git must see the caller's repository and index; a hook sets GIT_DIR and GIT_INDEX_FILE
  execFileSync("git", ["ls-files", "-z", ...options, "--", "*.rb"], { encoding: "utf8" }).split("\0").filter(Boolean);

/** Tracked and untracked non-ignored Ruby files with their expectations. */
function trackedEntries() {
  // An unstaged deletion stays in the index; other read failures remain failures.
  const deleted = new Set(listRuby("--deleted"));
  const files = listRuby("--cached", "--others", "--exclude-standard").filter((file) => !deleted.has(file));
  const stale = [...trackedRejections.keys()].filter((file) => !files.includes(file));
  return {
    entries: files.map((file) => ({ path: file, file, expected: trackedRejections.get(file) ?? "" })),
    stale: stale.map((file) => `expected rejection not found: ${file}`),
  };
}

/** Distinct captured sources in `directory`, with expectations from the checked-in list. */
function captured(directory) {
  const rejections = parseRejections(JSON.parse(readFileSync(capturedRejections, "utf8")));
  const files = readdirSync(directory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && entry.name.endsWith(".rb"))
    .map((entry) => ({ path: entry.name, bytes: readFileSync(path.join(directory, entry.name)) }));
  // Tests that read a tracked rejection fixture capture it; its expectation stays in one place.
  const fixtures = new Map(
    [...trackedRejections].map(([file, message]) => [
      createHash("sha256").update(readFileSync(file)).digest("hex"),
      message,
    ]),
  );
  const { entries, stale } = capturedEntries(files, rejections, fixtures);
  return {
    // File names carry a short hash; the label adds the SHA-256 the rejection list is keyed by.
    entries: entries.map((entry) => ({
      ...entry,
      path: `${entry.path} (sha256 ${entry.hash})`,
      file: path.join(directory, entry.path),
    })),
    stale: stale.map((hash) => `listed rejection ${hash} was not captured; update ${path.basename(capturedRejections)}`),
  };
}

// The list arrives on stdin NUL-separated, so its size never meets a command-line limit. Ruby
// reports one JSON line per file and a final summary; every decision is made in JavaScript.
const program = `
require "json"
# Compile warnings are not failures, and deliberately odd test sources produce many of them.
$VERBOSE = nil
minimum = ARGV.fetch(0).to_i
abort "check-ruby-syntax: Ruby #{RUBY_VERSION} found; #{minimum}.0 or later is required" if RUBY_VERSION.to_i < minimum
text = ->(value) { value.to_s.dup.force_encoding(Encoding::UTF_8).scrub }
$stdin.binmode.read.split("\\0").each do |file|
  result = begin
    RubyVM::InstructionSequence.compile_file(file)
    { status: "compiled" }
  rescue SyntaxError => error
    { status: "syntax", message: text.(error.message) }
  rescue ScriptError, StandardError => error
    { status: "error", error: error.class.name, message: text.(error.message) }
  end
  puts JSON.generate({ file: text.(file), **result })
end
puts JSON.generate({ summary: RUBY_VERSION })
`;

const capturedFlag = process.argv.indexOf("--captured");
let selection;
try {
  selection = capturedFlag === -1 ? trackedEntries() : captured(process.argv[capturedFlag + 1] ?? "");
} catch (error) {
  console.error(`check-ruby-syntax: ${error.message}`);
  process.exit(2);
}
// RUBYOPT is applied after the command line and could select parse.y, which accepts sources Prism
// rejects; RUBYLIB could shadow the json library. Both are cleared.
const { RUBYOPT: _rubyopt, RUBYLIB: _rubylib, ...env } = process.env;
const run = spawnSync("ruby", ["--parser=prism", "-e", program, String(minimumMajor)], {
  env,
  input: selection.entries.map((entry) => entry.file).join("\0"),
  encoding: "utf8",
  maxBuffer: 256 * 1024 * 1024,
  stdio: ["pipe", "pipe", "inherit"],
});
if (run.error) {
  console.error(`check-ruby-syntax: cannot run ruby: ${run.error.message}; install Ruby 4.0 or later`);
  process.exit(2);
}
if (run.status !== 0 || run.signal) {
  console.error(`check-ruby-syntax: ruby failed (${run.status ?? run.signal})`);
  process.exit(2);
}
const lines = run.stdout.split("\n").filter(Boolean).map((line) => JSON.parse(line));
const summary = lines.pop();
if (summary?.summary === undefined) {
  console.error("check-ruby-syntax: ruby printed no summary");
  process.exit(2);
}
const byFile = new Map(selection.entries.map((entry) => [entry.file, entry.path]));
const results = lines.map(({ file, ...result }) => ({ path: byFile.get(file) ?? file, ...result }));
const { failures, rejected } = judge(selection.entries, results);
// Stale list entries are not sources, so they count apart from the per-source failures.
const sourceFailures = failures.length;
failures.push(...selection.stale);
for (const failure of failures) console.error(failure);
const total = selection.entries.length;
const stale = selection.stale.length;
console.log(
  `check-ruby-syntax: ${Math.max(0, total - sourceFailures)}/${total} Ruby sources behave as expected under Ruby ${summary.summary} (${rejected} expected rejection${rejected === 1 ? "" : "s"})${stale === 0 ? "" : `; ${stale} stale list entr${stale === 1 ? "y" : "ies"}`}`,
);
process.exit(failures.length === 0 ? 0 : 1);
