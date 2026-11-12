// Every Ruby source file must compile under the Ruby the adapter's semantics are checked against.
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
// The list arrives on stdin, NUL-separated, so its size never meets a command-line limit. Any
// failure is reported per file, so one unreadable file does not hide later syntax errors.
const program = `
minimum = ARGV.fetch(0).to_i
abort "check-ruby-syntax: Ruby #{RUBY_VERSION} found; #{minimum}.0 or later is required" if RUBY_VERSION.to_i < minimum
paths = $stdin.binmode.read.split("\\0")
failed = paths.select do |path|
  RubyVM::InstructionSequence.compile_file(path)
  false
rescue ScriptError, StandardError => error
  warn "#{path}: #{error.class}: #{error.message}"
  true
end
puts "check-ruby-syntax: #{paths.size - failed.size}/#{paths.size} Ruby sources compile under Ruby #{RUBY_VERSION}"
exit(failed.empty? ? 0 : 1)
`;
// RUBYOPT is applied after the command line and could select parse.y, which accepts sources Prism
// rejects, so it is cleared.
const { RUBYOPT: _ignored, ...env } = process.env;
const run = spawnSync("ruby", ["--parser=prism", "-e", program, String(minimumMajor)], {
  env,
  input: files.join("\0"),
  stdio: ["pipe", "inherit", "inherit"],
});
if (run.error) {
  console.error(`check-ruby-syntax: cannot run ruby: ${run.error.message}; install Ruby 4.0 or later`);
  process.exit(2);
}
process.exit(run.status ?? 1);
