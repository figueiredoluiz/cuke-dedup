// Rules for check-ruby-execution.mjs: which files Cucumber would load, computed from the effective
// configuration as discovery applies it, and how the registrations Ruby made are judged against
// the definitions the analyzer extracted.

/** The UTF-8 bytes of `text`, one per string unit, as globset's byte-mode regex sees them. */
const bytes = (text) => Buffer.from(text, "utf8").toString("latin1");

/**
 * Compiles a glob with `globset`'s default semantics, which the analyzer's discovery uses. globset
 * matches bytes, so `?` and a class each match one byte of the UTF-8 path. `*` and `**` cross `/`,
 * a whole `**` segment also matches no directory, `[...]`, `[!...]` and `[^...]` are classes,
 * `{a,b}` alternates (empty alternatives dropped) and `\` escapes. Syntax outside that tested
 * subset throws instead of guessing: a class that starts with `]` or holds `\` or `[`, nested or
 * unbalanced braces, `**` inside braces, and a trailing `\`.
 * @param {string} glob A configured glob.
 * @returns {{test: (path: string) => boolean}} A matcher for whole root-relative paths.
 */
export function globsetRegex(glob) {
  const pattern = bytes(glob);
  // Each open brace is a frame of finished alternatives plus the one being read.
  const frames = [{ alternatives: [], current: "" }];
  /** Appends regex source to the alternative being read. */
  const emit = (text) => {
    frames.at(-1).current += text;
  };
  /** Escapes one pattern character for a regex. */
  const literal = (ch) => ch.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
  /** Rejects syntax whose globset meaning this matcher does not model. */
  const unsupported = (reason) => {
    throw new Error(`unsupported pattern \`${glob}\`: ${reason}`);
  };
  for (let index = 0; index < pattern.length; index += 1) {
    const ch = pattern[index];
    if (ch === "*" && pattern[index + 1] === "*" && frames.length > 1) unsupported("`**` inside braces");
    if (ch === "*" && pattern[index + 1] === "*" && (index === 0 || pattern[index - 1] === "/") && pattern[index + 2] === "/") {
      // A whole `**` segment matches any directory prefix, including none.
      emit("(?:.*/)?");
      index += 2;
    } else if (ch === "*") {
      while (pattern[index + 1] === "*") index += 1;
      emit(".*");
    } else if (ch === "?") emit(".");
    else if (ch === "{") {
      if (frames.length > 1) unsupported("nested braces");
      frames.push({ alternatives: [], current: "" });
    }
    else if (ch === "," && frames.length > 1) {
      const frame = frames.at(-1);
      frame.alternatives.push(frame.current);
      frame.current = "";
    } else if (ch === "}" && frames.length > 1) {
      const frame = frames.pop();
      // globset discards empty alternatives: `a{,b}` matches `ab` only.
      const alternatives = [...frame.alternatives, frame.current].filter((alternative) => alternative !== "");
      emit(`(?:${alternatives.join("|")})`);
    } else if (ch === "}") unsupported("unbalanced brace");
    else if (ch === "[") {
      const negated = pattern[index + 1] === "!" || pattern[index + 1] === "^";
      const start = negated ? index + 2 : index + 1;
      const end = pattern.indexOf("]", start);
      const body = pattern.slice(start, end);
      if (end < 0 || body === "" || /[\\[]/.test(body)) unsupported("character class");
      emit(`[${negated ? "^" : ""}${body}]`);
      index = end;
    } else if (ch === "\\") {
      index += 1;
      if (index >= pattern.length) unsupported("trailing backslash");
      emit(literal(pattern[index]));
    } else emit(literal(ch));
  }
  if (frames.length > 1) unsupported("unbalanced brace");
  const regex = new RegExp(`^${frames[0].current}$`);
  return { test: (path) => regex.test(bytes(path)) };
}

/**
 * Whether discovery's walk prunes a root-relative path: an `exclude` glob matches it or an
 * ancestor, in its own or trailing-`/` form, or a segment is hidden and `includeHidden` is off.
 * @param {{exclude: string[], includeHidden: boolean}} config From `--print-config`.
 * @returns {(path: string) => boolean} The predicate.
 */
function pruned(config) {
  const excludes = config.exclude.map(globsetRegex);
  return (path) => {
    const parts = path.split("/");
    return (!config.includeHidden && parts.some((part) => part.startsWith(".")))
      || parts.some((_, index) => {
        const ancestor = parts.slice(0, index + 1).join("/");
        return excludes.some((glob) => glob.test(ancestor) || glob.test(`${ancestor}/`));
      });
  };
}

/**
 * The Ruby files an effective configuration selects, computed without the analyzer's discovery so
 * that a file discovery drops is still loaded: the walk does not prune it, no `features` glob
 * claims it (discovery tests features first), and a `definitions` glob matches. Ruby is opt-in, so
 * a glob that also matches the path with another suffix (`**`, `steps/*`) does not select it.
 * Files reached only through `require` are left for Ruby to load.
 * @param {string[]} paths Every root-relative file in the project, `/`-separated.
 * @param {{definitions: string[], features: string[], exclude: string[], includeHidden: boolean}} config From `--print-config`.
 * @returns {string[]} The selected `.rb` files, sorted.
 */
export function selectedFiles(paths, config) {
  const definitions = config.definitions.map(globsetRegex);
  const features = config.features.map(globsetRegex);
  const walkPrunes = pruned(config);
  return paths
    .filter((path) => {
      // Ruby is opt-in: only a matching glob that also depends on the `.rb` suffix selects a file.
      const probe = path.replace(/\.rb$/, ".cuke-dedup-suffix-probe");
      return /(^|\/)[^/]+\.rb$/.test(path) && definitions.some((glob) => glob.test(path) && !glob.test(probe));
    })
    .filter((path) => !walkPrunes(path) && !features.some((glob) => glob.test(path)))
    .sort();
}

// Discovery's ignore-file names, lowest precedence first.
const IGNORE_FILES = [".gitignore", ".ignore", ".cuke-dedupignore"];

/**
 * The ignore files discovery reads: those in directories its walk enters. The file's own hidden
 * name does not prune it; the walk reads ignore files by name.
 * @param {string[]} paths Every root-relative file in the project, `/`-separated.
 * @param {{exclude: string[], includeHidden: boolean}} config From `--print-config`.
 * @returns {{path: string, rank: number, directory: string}[]} Each file with its precedence rank
 *   and its `/`-terminated directory (`""` for the root).
 */
export function ignoreFiles(paths, config) {
  const walkPrunes = pruned(config);
  return paths.flatMap((path) => {
    const name = path.split("/").at(-1);
    const rank = IGNORE_FILES.indexOf(name);
    const directory = path.slice(0, -name.length);
    return rank < 0 || (directory !== "" && walkPrunes(directory.slice(0, -1))) ? [] : [{ path, rank, directory }];
  });
}

/**
 * The first ignore-file pair whose precedence a per-directory `.gitignore` mirror cannot reproduce,
 * or `null`. The `ignore` crate ranks file names across all levels (custom over `.ignore` over
 * `.gitignore`), while git lets the deeper directory win, so the two disagree only when an ancestor
 * directory holds a higher-ranked name than a descendant directory.
 * @param {{path: string, rank: number, directory: string}[]} files From {@link ignoreFiles}.
 * @returns {string | null} The conflicting pair, ancestor first.
 */
export function ignoreLayoutConflict(files) {
  for (const outer of files) {
    const inner = files.find((file) => file.directory !== outer.directory && file.directory.startsWith(outer.directory) && outer.rank > file.rank);
    if (inner) return `${outer.path} over ${inner.path}`;
  }
  return null;
}

/**
 * Why selection cannot be computed when `includeHidden` is off and an ignore file has a negation
 * rule while the project holds a hidden file, or `null`. The `ignore` crate lets a whitelist match
 * override the hidden filter; selection prunes hidden paths before ignore rules apply.
 * @param {string[]} paths Every root-relative file in the project, `/`-separated.
 * @param {string[]} rules The text of each ignore file discovery reads.
 * @param {{includeHidden: boolean}} config From `--print-config`.
 * @returns {string | null} The reason, naming one hidden file.
 */
export function hiddenWhitelistConflict(paths, rules, config) {
  const negation = rules.some((text) => text.split("\n").some((line) => line.startsWith("!")));
  const hidden = paths.find((path) => !IGNORE_FILES.includes(path.split("/").at(-1)) && path.split("/").some((part) => part.startsWith(".")));
  return !config.includeHidden && negation && hidden ? `negation rule with hidden file ${hidden}` : null;
}

/**
 * Selected files discovery did not list, as problems.
 * @param {string[]} files The selected files Ruby loads.
 * @param {string[]} listed The files discovery listed.
 * @returns {string[]} One problem per missing file.
 */
export function undiscovered(files, listed) {
  return files.filter((file) => !listed.includes(file)).map((file) => `selected but not discovered: ${file}`);
}

/** Matcher identity: kind, text and flags. */
const identity = (item) => `${item.matcherKind} ${JSON.stringify(item.matcher)}${item.matcherFlags ? `/${item.matcherFlags}` : ""}`;

/**
 * Registrations in `left` without a counterpart in `right`, as display strings. Identity is the
 * matcher kind, text and flags: locations differ legitimately, because Cucumber records a
 * handler's own location while the analyzer reports the registration call.
 * @param {object[]} left Registrations with `path`, `line`, `matcherKind`, `matcher`, `matcherFlags`.
 * @param {object[]} right The registrations to match them against.
 * @returns {string[]} One entry per unmatched registration.
 */
export function surplus(left, right) {
  const counts = new Map();
  for (const item of right) counts.set(identity(item), (counts.get(identity(item)) ?? 0) + 1);
  return left.filter((item) => {
    const remaining = counts.get(identity(item)) ?? 0;
    counts.set(identity(item), remaining - 1);
    return remaining <= 0;
  }).map((item) => `${identity(item)} at ${item.path}:${item.line}`);
}

/**
 * Every difference between what Ruby did and what the analyzer claims for a complete suite.
 * @param {{registrations: object[]}} analyzer The analyzer's extracted definitions.
 * @param {{registrations: object[], loadErrors: object[], replaced: string[]}} ruby The harness result.
 * @returns {string[]} Problems; empty when the two agree.
 */
export function problems(analyzer, ruby) {
  return [
    ...surplus(analyzer.registrations, ruby.registrations).map((item) => `extracted but not registered by Ruby: ${item}`),
    ...surplus(ruby.registrations, analyzer.registrations).map((item) => `registered by Ruby but not extracted: ${item}`),
    ...ruby.loadErrors.map((error) => `${error.path}:${error.line ?? "?"} fails to load: ${error.error}: ${error.message}`),
    ...ruby.replaced.map((keyword) => `step keyword replaced at load time: ${keyword}`),
  ];
}

/** ASCII whitespace, as the crate's non-Unicode regex `\s` reads it. */
const SPACE = "[\\t\\n\\v\\f\\r ]";
// Rust's multi-line `^` and `$` anchor only at `\n`; JavaScript's also anchor at `\r`.
const EXCLUDES = new RegExp(`(?<![^\\n])${SPACE}*excludesfile${SPACE}*=${SPACE}*"?${SPACE}*([^\\t\\n\\v\\f\\r ]+?)${SPACE}*"?${SPACE}*(?![^\\n])`, "i");

/**
 * The global excludes file the analyzer applies, resolved as the pinned `ignore` crate's
 * `gitconfig_excludes_path` does, which differs from git: the first config with an
 * `excludesfile` line wins in the order `GIT_CONFIG_GLOBAL`, `~/.gitconfig`, the XDG config,
 * then `GIT_CONFIG_SYSTEM` or `/etc/gitconfig`, so a global config without the key falls
 * through. The value is the first match, unquoted, with every `~` replaced by the home directory;
 * a value with whitespace or invalid UTF-8 does not count. The fallback is `git/ignore` under
 * the XDG config directory. Empty variables count as unset.
 * @param {{env: Record<string, string | undefined>, home: string | undefined, read: (path: string) => Buffer | undefined}} context
 * @returns {string | undefined} The path, which may not exist.
 */
export function globalExcludesPath({ env, home, read }) {
  /** The variable `name`, with an empty value read as unset. */
  const set = (name) => env[name] || undefined;
  const xdg = set("XDG_CONFIG_HOME") ?? (home === undefined ? undefined : `${home}/.config`);
  const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  /** The `excludesfile` value in the config at `path`, if it is readable and has one. */
  const parse = (path) => {
    const match = path === undefined ? null : read(path)?.toString("latin1").match(EXCLUDES);
    if (!match) return undefined;
    try {
      const value = decoder.decode(Buffer.from(match[1], "latin1"));
      return home === undefined ? value : value.replaceAll("~", () => home);
    } catch {
      return undefined;
    }
  };
  const sources = [set("GIT_CONFIG_GLOBAL"), home && `${home}/.gitconfig`, xdg && `${xdg}/git/config`, set("GIT_CONFIG_SYSTEM") ?? "/etc/gitconfig"];
  for (const source of sources) {
    const value = parse(source || undefined);
    if (value !== undefined) return value;
  }
  return xdg && `${xdg}/git/ignore`;
}
