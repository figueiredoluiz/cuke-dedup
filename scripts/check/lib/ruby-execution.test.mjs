import assert from "node:assert/strict";
import test from "node:test";
import { globalExcludesPath, globsetRegex, hiddenWhitelistConflict, ignoreFiles, ignoreLayoutConflict, problems, selectedFiles, surplus, undiscovered } from "./ruby-execution.mjs";

const files = ["x.rb", "a/y.rb", "a/b/z.rb", "c/w.rb", "steps/s.rb", "q.rb"];

/** Each row's expectation is the file list the release binary's `--explain-discovery` printed. */
test("globs select what the analyzer's globset discovery selects", () => {
  for (const [pattern, expected] of [
    ["*.rb", files],
    ["**/*.rb", files],
    ["a/*.rb", ["a/y.rb", "a/b/z.rb"]],
    ["a/**/*.rb", ["a/y.rb", "a/b/z.rb"]],
    ["**/s.rb", ["steps/s.rb"]],
    ["steps/*.rb", ["steps/s.rb"]],
    ["?.rb", ["x.rb", "q.rb"]],
    ["[xq].rb", ["x.rb", "q.rb"]],
    ["[!x].rb", ["q.rb"]],
  ]) {
    assert.deepEqual(files.filter((file) => globsetRegex(pattern).test(file)), expected, pattern);
  }
  assert.ok(globsetRegex("{a,c}/w.rb").test("c/w.rb"));
  // Checked with an `exclude` of each pattern in `.cuke-dedup.json`: `a.rb` stays discovered.
  for (const pattern of ["a{,b}.rb", "a{b,}.rb"]) {
    assert.ok(!globsetRegex(pattern).test("a.rb"), pattern);
    assert.ok(globsetRegex(pattern).test("ab.rb"), pattern);
  }
  assert.ok(!globsetRegex("x\\*.rb").test("xy.rb"));
  // `^` negates like `!`.
  assert.deepEqual(files.filter((file) => globsetRegex("[^x].rb").test(file)), ["q.rb"]);
  // globset matches bytes: `?` and a class each take one byte of a two-byte `é`.
  assert.ok(!globsetRegex("?.rb").test("é.rb"));
  assert.ok(globsetRegex("??.rb").test("é.rb"));
  assert.ok(!globsetRegex("[é].rb").test("é.rb"));
  assert.ok(globsetRegex("é.rb").test("é.rb"));
});

/** Syntax outside the tested subset fails instead of selecting a guess. */
test("unsupported glob syntax throws", () => {
  for (const pattern of ["[]].rb", "[!]].rb", "[a\\]].rb", "[[].rb", "{a,{b,c}}.rb", "a{b.rb", "a}.rb", "a\\", "[ab.rb", "{**/,}s.rb", "{a,**}/w.rb", "[^]].rb"]) {
    assert.throws(() => globsetRegex(pattern), /unsupported pattern/, pattern);
  }
});

const walk = { exclude: ["**/node_modules/**"], includeHidden: false };

/** Only an ancestor directory holding a higher-ranked ignore-file name than a descendant fails. */
test("ignore layouts the per-directory mirror cannot reproduce are reported", () => {
  /** The conflict for a project holding `paths`. */
  const conflict = (paths) => ignoreLayoutConflict(ignoreFiles(paths, walk));
  assert.equal(conflict([".ignore", "a/.gitignore", "a/x.rb"]), ".ignore over a/.gitignore");
  assert.equal(conflict([".cuke-dedupignore", "a/b/.ignore"]), ".cuke-dedupignore over a/b/.ignore");
  // Controls: same directory, deeper higher rank, unrelated directories, same name across levels.
  for (const paths of [[".gitignore", ".ignore"], [".gitignore", "a/.ignore"], ["a/.ignore", "b/.gitignore"],
    [".gitignore", "a/.gitignore"], ["ab/.ignore", "a/.gitignore"], ["x.rb"]]) {
    assert.equal(conflict(paths), null, paths.join(" "));
  }
});

/** Ignore files in directories the walk prunes are never read. */
test("ignore files come only from directories discovery enters", () => {
  const paths = [".gitignore", "a/.ignore", ".hidden/.gitignore", "node_modules/m/.gitignore", "x.rb"];
  assert.deepEqual(ignoreFiles(paths, walk).map((file) => file.path), [".gitignore", "a/.ignore"]);
  assert.deepEqual(ignoreFiles(paths, { ...walk, includeHidden: true }).map((file) => file.path),
    [".gitignore", "a/.ignore", ".hidden/.gitignore"]);
  assert.deepEqual(ignoreFiles(["a/.ignore"], walk), [{ path: "a/.ignore", rank: 1, directory: "a/" }]);
});

/** A negation rule may whitelist a hidden file that selection already pruned. */
test("negation rules with hidden files are reported", () => {
  const config = { includeHidden: false };
  assert.equal(hiddenWhitelistConflict([".gitignore", "a/.h.rb"], ["!.h.rb\n"], config), "negation rule with hidden file a/.h.rb");
  assert.equal(hiddenWhitelistConflict([".gitignore", ".hidden/x.rb"], ["x\n!y\n"], config), "negation rule with hidden file .hidden/x.rb");
  // Controls: hidden files included, no negation, an escaped `\!` literal, only ignore files hidden.
  assert.equal(hiddenWhitelistConflict([".gitignore", "a/.h.rb"], ["!.h.rb\n"], { includeHidden: true }), null);
  assert.equal(hiddenWhitelistConflict([".gitignore", "a/.h.rb"], ["x.rb\n"], config), null);
  assert.equal(hiddenWhitelistConflict([".gitignore", "a/.h.rb"], ["\\!x.rb\n"], config), null);
  assert.equal(hiddenWhitelistConflict([".gitignore", "a/.ignore", "x.rb"], ["!x.rb\n"], config), null);
});

/** Selection follows the effective configuration, as discovery applies it, without discovery. */
test("selected files follow definitions, excludes and hidden-file rules", () => {
  const config = { definitions: ["**/*.rb"], features: ["**/*.feature"], exclude: ["**/node_modules/**", "skip.rb", "gen"], includeHidden: false };
  const paths = ["a.rb", "b/c.rb", "skip.rb", "gen/x.rb", "node_modules/m/i.rb", ".hidden/h.rb", "d/.h.rb", "notes.txt"];
  assert.deepEqual(selectedFiles(paths, config), ["a.rb", "b/c.rb"]);
  assert.deepEqual(selectedFiles(paths, { ...config, includeHidden: true }), [".hidden/h.rb", "a.rb", "b/c.rb", "d/.h.rb"]);
  assert.deepEqual(selectedFiles(["entry.rb", "providers/p.rb"], { ...config, definitions: ["entry.rb"] }), ["entry.rb"]);
  // Ruby is opt-in: a glob that also matches another suffix selects nothing on its own.
  const mixed = ["entry.rb", "other.rb", "steps/s.rb"];
  assert.deepEqual(selectedFiles(mixed, { ...config, definitions: ["**/*", "entry.rb"] }), ["entry.rb"]);
  assert.deepEqual(selectedFiles(mixed, { ...config, definitions: ["steps/*"] }), []);
  assert.deepEqual(selectedFiles(mixed, { ...config, definitions: ["steps/*.rb"] }), ["steps/s.rb"]);
  assert.deepEqual(selectedFiles(mixed, { ...config, definitions: [] }), []);
  // Discovery tests feature globs first.
  assert.deepEqual(selectedFiles(mixed, { ...config, features: ["steps/**"] }), ["entry.rb", "other.rb"]);
});

/** A selected file discovery did not list is a problem; a listed one is not. */
test("undiscovered names selected files discovery dropped", () => {
  assert.deepEqual(undiscovered(["a.rb", "b.rb"], ["a.rb"]), ["selected but not discovered: b.rb"]);
  assert.deepEqual(undiscovered(["a.rb"], ["a.rb", "dep.rb"]), []);
});

/** A `steps.rb` Cucumber-expression registration, with optional overrides. */
const registration = (matcher, line = 1, extra = {}) =>
  ({ path: "steps.rb", line, matcherKind: "cucumber-expression", matcher, matcherFlags: "", ...extra });

/** Identity ignores locations but counts duplicates and keeps kind and flags apart. */
test("surplus compares matcher multisets", () => {
  assert.deepEqual(surplus([registration("a", 1)], [registration("a", 9)]), []);
  assert.deepEqual(surplus([registration("a"), registration("a", 2)], [registration("a")]), ['cucumber-expression "a" at steps.rb:2']);
  assert.deepEqual(surplus([registration("a", 1, { matcherKind: "regular-expression" })], [registration("a")]),
    ['regular-expression "a" at steps.rb:1']);
  assert.deepEqual(surplus([registration("a", 1, { matcherKind: "regular-expression", matcherFlags: "i" })],
    [registration("a", 1, { matcherKind: "regular-expression" })]), ['regular-expression "a"/i at steps.rb:1']);
});

/** Each kind of disagreement is reported; agreement reports nothing. */
test("problems name every disagreement", () => {
  const ruby = { registrations: [registration("a")], loadErrors: [], replaced: [] };
  assert.deepEqual(problems({ registrations: [registration("a")] }, ruby), []);
  assert.deepEqual(problems({ registrations: [registration("b")] }, {
    registrations: [registration("a")],
    loadErrors: [{ path: "steps.rb", line: 2, error: "RuntimeError", message: "stop" }],
    replaced: ["Then"],
  }), [
    'extracted but not registered by Ruby: cucumber-expression "b" at steps.rb:1',
    'registered by Ruby but not extracted: cucumber-expression "a" at steps.rb:1',
    "steps.rb:2 fails to load: RuntimeError: stop",
    "step keyword replaced at load time: Then",
  ]);
});

/** Resolves against an in-memory file system; `null` home means none. */
const excludes = (env, filesystem, home = "/h") =>
  globalExcludesPath({ env, home: home ?? undefined, read: (path) => (path in filesystem ? Buffer.from(filesystem[path]) : undefined) });

/** Each row follows `ignore` 0.4.34 `gitconfig_excludes_path`, including where git differs. */
test("global excludes resolve as the analyzer's ignore crate resolves them", () => {
  /** A config that sets `core.excludesFile` to `path`. */
  const key = (path) => `[core]\n\texcludesFile = ${path}\n`;
  const home = { "/h/.gitconfig": key("/home-x") };
  for (const [name, env, filesystem, expected] of [
    ["global env wins", { GIT_CONFIG_GLOBAL: "/g" }, { ...home, "/g": key("/g-x") }, "/g-x"],
    ["global env without the key falls through to home", { GIT_CONFIG_GLOBAL: "/g" }, { ...home, "/g": "[core]\n" }, "/home-x"],
    ["empty global env counts as unset", { GIT_CONFIG_GLOBAL: "" }, home, "/home-x"],
    ["home beats XDG", {}, { ...home, "/h/.config/git/config": key("/xdg-x") }, "/home-x"],
    ["XDG_CONFIG_HOME locates the XDG config", { XDG_CONFIG_HOME: "/x" }, { "/x/git/config": key("/xdg-x") }, "/xdg-x"],
    ["system is last", { GIT_CONFIG_SYSTEM: "/s" }, { "/s": key("/s-x") }, "/s-x"],
    ["empty system env reads /etc/gitconfig", { GIT_CONFIG_SYSTEM: "" }, { "/etc/gitconfig": key("/etc-x") }, "/etc-x"],
    ["default under XDG_CONFIG_HOME", { XDG_CONFIG_HOME: "/x" }, {}, "/x/git/ignore"],
    ["default under home", { XDG_CONFIG_HOME: "" }, {}, "/h/.config/git/ignore"],
    ["every tilde expands", {}, { "/h/.gitconfig": key("~/a~b") }, "/h/a/hb"],
    ["quotes are dropped", {}, { "/h/.gitconfig": '[core]\n  excludesfile = "/q"\n' }, "/q"],
    ["first match in a file wins", {}, { "/h/.gitconfig": `${key("/one")}${key("/two")}` }, "/one"],
    ["a value with a space falls through", {}, { "/h/.gitconfig": key("/a b"), "/etc/gitconfig": key("/etc-x") }, "/etc-x"],
    ["a lone carriage return is not a line break", {}, { "/h/.gitconfig": "x\rexcludesfile = /a\n" }, "/h/.config/git/ignore"],
    ["CRLF line endings match", {}, { "/h/.gitconfig": "[core]\r\n\texcludesFile = /c\r\n" }, "/c"],
    ["a byte order mark in the value is kept", {}, { "/h/.gitconfig": "excludesfile = \uFEFF/b\n" }, "\uFEFF/b"],
    ["invalid UTF-8 falls through", {}, { "/h/.gitconfig": Buffer.concat([Buffer.from("excludesfile = /"), Buffer.from([0xff])]), "/etc/gitconfig": key("/etc-x") }, "/etc-x"],
  ]) {
    assert.equal(excludes(env, filesystem), expected, name);
  }
  assert.equal(excludes({}, {}, null), undefined, "no home and no XDG directory");
  assert.equal(excludes({}, { "/$&/.gitconfig": key("~/i") }, "/$&"), "/$&/i", "the home directory is inserted literally");
});
