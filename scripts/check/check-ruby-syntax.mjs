// Every Ruby source file must compile under the Ruby the adapter's semantics are checked against,
// except listed fixtures that must be rejected.
// tree-sitter-ruby accepts some sources Ruby rejects, so analyzer observations alone cannot show
// that a fixture is valid Ruby. Compiling never runs the code.
import { execFileSync, spawnSync } from "node:child_process";

const minimumMajor = 4;
/** NUL-separated `git ls-files` output for Ruby sources, with the given selection options. */
const listRuby = (...options) =>
  execFileSync("git", ["ls-files", "-z", ...options, "--", "*.rb"], { encoding: "utf8" }).split("\0").filter(Boolean);
// An unstaged deletion stays in the index; other read failures remain failures.
const deleted = new Set(listRuby("--deleted"));
const files = listRuby("--cached", "--others", "--exclude-standard").filter((file) => !deleted.has(file));
// Fixtures that pin the analyzer's result on source Ruby rejects, each with the parser message its
// SyntaxError must contain, so an unrelated error cannot stand in for the one the fixture pins.
const rejected = new Map([
  ["fixtures/ruby-parity/regex-braces/leading-bracket/steps.rb", "empty char-class"],
]);
const stale = [...rejected.keys()].filter((file) => !files.includes(file));
if (stale.length > 0) {
  console.error(`check-ruby-syntax: expected rejections not found: ${stale.join(", ")}`);
  process.exit(1);
}
// The list arrives on stdin as NUL-separated path and expected-message pairs (an empty message:
// must compile), so its size never meets a command-line limit. Any failure is reported per file,
// so one unreadable file does not hide later syntax errors.
const program = `
minimum = ARGV.fetch(0).to_i
abort "check-ruby-syntax: Ruby #{RUBY_VERSION} found; #{minimum}.0 or later is required" if RUBY_VERSION.to_i < minimum
entries = $stdin.binmode.read.split("\\0").each_slice(2).map { |path, message| [message.to_s, path] }
failed = entries.select do |message, path|
  RubyVM::InstructionSequence.compile_file(path)
  warn "#{path}: compiles, but is listed as an expected rejection" unless message.empty?
  !message.empty?
rescue SyntaxError => error
  pinned = !message.empty? && error.message.include?(message)
  warn "#{path}: #{error.class}: #{error.message}" unless pinned
  warn "#{path}: rejected, but without #{message.inspect}" unless pinned || message.empty?
  !pinned
rescue ScriptError, StandardError => error
  warn "#{path}: #{error.class}: #{error.message}"
  true
end
expected = entries.count { |message, _| !message.empty? }
puts "check-ruby-syntax: #{entries.size - failed.size}/#{entries.size} Ruby sources behave as expected under Ruby #{RUBY_VERSION} (#{expected} expected rejection#{"s" unless expected == 1})"
exit(failed.empty? ? 0 : 1)
`;
// RUBYOPT is applied after the command line and could select parse.y, which accepts sources Prism
// rejects, so it is cleared.
const { RUBYOPT: _ignored, ...env } = process.env;
const run = spawnSync("ruby", ["--parser=prism", "-e", program, String(minimumMajor)], {
  env,
  input: files.flatMap((file) => [file, rejected.get(file) ?? ""]).join("\0"),
  stdio: ["pipe", "inherit", "inherit"],
});
if (run.error) {
  console.error(`check-ruby-syntax: cannot run ruby: ${run.error.message}; install Ruby 4.0 or later`);
  process.exit(2);
}
process.exit(run.status ?? 1);
