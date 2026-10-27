// Keeps the duplication ratchet's file-level scopes exact.
//
// The ratchet assigns whole files to scopes: files under a `tests` path segment, `tests.rs` files
// and `fuzz/` are tests, every other file under `src/` is production. That is exact only while no
// production file holds a test module, so a test module lives in its own file through
// `#[cfg(test)] mod tests;`. This scan reports every module that breaks that: a test-only one
// written inline (`mod name { … }`), one declared out of line under another name or with `#[path]`
// (its file is not a `tests.rs`), an inner `#![cfg(…)]` that makes the code around it test-only, and
// a `mod tests` that is not test-only (its production code would be measured as tests).
//
// "Test-only" means the `cfg` predicate cannot hold when `test` is off: `cfg(test)` and
// `cfg(all(test, unix))` are test-only, `cfg(not(test))` and `cfg(any(windows, test))` are not.
// Stacked `cfg` attributes on one item combine with `all`, and every assignment of the other
// options is tried, so correlated predicates are decided exactly. Shapes the scan does not model
// are reported, not guessed: `cfg_attr` or a path-qualified `cfg` on a module or inner attribute,
// and a test-only module named by a macro fragment. A module's or inner attribute's `cfg` that
// cannot be parsed throws. The scan reads tokens of the code with comments and literals blanked
// (`rustCode`), so comments, blank lines, multi-line attributes and string contents do not change
// the result. Attributes of items other than modules are not evaluated: test-only helpers
// (`#[cfg(test)] fn …`, `use`, `const`) remain in the production figure.

import { rustCode } from "./rust-docstring-code.mjs";

/**
 * Splits blanked Rust code into identifier and punctuation tokens with their 1-based lines.
 * @param {string} code Output of `rustCode`, so no comment or literal remains.
 * @returns {{ text: string, line: number }[]}
 */
function tokensOf(code) {
  const tokens = [];
  let line = 1;
  for (const match of code.matchAll(/\n|r#[\p{XID_Start}_][\p{XID_Continue}]*|[\p{XID_Continue}]+|\S/gu)) {
    if (match[0] === "\n") line += 1;
    else tokens.push({ text: match[0].startsWith("r#") ? match[0].slice(2) : match[0], line });
  }
  return tokens;
}

/** Returns the index after the group whose opening bracket is token `open`. */
function groupEnd(tokens, open) {
  const pairs = { "(": ")", "[": "]", "{": "}" };
  const stack = [];
  for (let at = open; at < tokens.length; at += 1) {
    const { text } = tokens[at];
    if (pairs[text]) stack.push(pairs[text]);
    else if (text === stack.at(-1)) {
      stack.pop();
      if (stack.length === 0) return at + 1;
    }
  }
  throw new Error(`line ${tokens[open].line}: unclosed \`${tokens[open].text}\``);
}

/** Most distinct options one item's `cfg` attributes may name; each doubles the work. */
const MAX_CFG_OPTIONS = 16;

/**
 * Parses a `cfg` predicate into a tree.
 * @param {string[]} pieces Tokens between `cfg(` and its `)`. A string value is blanked, so
 *   `key = "value"` arrives as `key =`; each such pair becomes an option of its own, because the
 *   blanked values cannot be compared.
 * @param {() => string} freshName Names each `key =` option.
 * @returns {{ op: "option", name: string } | { op: "all" | "any" | "not", parts: object[] }}
 * @throws {Error} When the predicate is not built from options, `key = value`, `all`, `any`, `not`.
 */
function parseCfg(pieces, freshName) {
  let at = 0;
  /** Throws for a shape outside the cfg grammar. */
  const reject = () => {
    throw new Error(`unparsable cfg predicate \`${pieces.join(" ")}\``);
  };
  /** Parses one predicate at `at`. */
  const predicate = () => {
    const word = pieces[at];
    if (word === undefined || !/^[\p{XID_Start}_]/u.test(word)) reject();
    at += 1;
    if (pieces[at] === "=") {
      at += 1;
      return { op: "option", name: freshName() };
    }
    if (pieces[at] !== "(") return { op: "option", name: word };
    if (!["all", "any", "not"].includes(word)) reject();
    at += 1;
    const parts = [];
    while (pieces[at] !== ")") {
      parts.push(predicate());
      if (pieces[at] === ",") at += 1;
      else if (pieces[at] !== ")") reject();
    }
    at += 1;
    if (word === "not" && parts.length !== 1) reject();
    return { op: word, parts };
  };
  const tree = predicate();
  if (pieces[at] === ",") at += 1;
  if (at !== pieces.length) reject();
  return tree;
}

/**
 * Decides what `cfg` predicates that all apply to one item can be when `test` is off.
 *
 * Every assignment of the other options is tried, so correlated predicates such as
 * `any(test, unix)` with `not(unix)` are decided exactly.
 * @param {object[]} trees Parsed predicates; the item compiles when all of them hold.
 * @returns {"T" | "F" | "U"} True under every assignment, false under every one, or either.
 */
function valueWithoutTest(trees) {
  const names = new Set();
  /** Collects option names other than `test`. */
  const collect = (tree) => (tree.op === "option"
    ? tree.name !== "test" && names.add(tree.name)
    : tree.parts.forEach(collect));
  trees.forEach(collect);
  if (names.size > MAX_CFG_OPTIONS) throw new Error(`cfg attributes name more than ${MAX_CFG_OPTIONS} options`);
  const options = [...names];
  /** Evaluates a tree under one assignment. */
  const holds = (tree, assignment) => {
    if (tree.op === "option") return tree.name === "test" ? false : assignment.has(tree.name);
    if (tree.op === "not") return !holds(tree.parts[0], assignment);
    return tree.op === "all"
      ? tree.parts.every((part) => holds(part, assignment))
      : tree.parts.some((part) => holds(part, assignment));
  };
  const seen = new Set();
  for (let mask = 0; mask < 2 ** options.length; mask += 1) {
    const assignment = new Set(options.filter((_, bit) => mask & (1 << bit)));
    seen.add(trees.every((tree) => holds(tree, assignment)));
  }
  return seen.size === 2 ? "U" : seen.has(true) ? "T" : "F";
}

/**
 * Evaluates one `cfg` predicate with `test` off.
 * @param {string[]} pieces Tokens between `cfg(` and its `)`, string values blanked.
 * @returns {"T" | "F" | "U"} True under every assignment of the other options, false under every
 *   one, or either.
 * @throws {Error} When the predicate cannot be parsed.
 */
export function cfgValueWithoutTest(pieces) {
  let count = 0;
  return valueWithoutTest([parseCfg(pieces, () => `=${count++}`)]);
}

/**
 * Reads one attribute's path and arguments.
 * @param {{ text: string, line: number }[]} tokens Tokens of the file.
 * @param {number} open Index of the attribute's `[`.
 * @param {number} end Index after its `]`.
 * @returns {{ line: number, name: string, qualified: boolean, args: string[] | undefined }} The
 *   last path segment, whether the path has more than one, and the tokens inside `(…)`, if any.
 */
function attributeOf(tokens, open, end) {
  let at = open + 1;
  let qualified = tokens[at]?.text === ":";
  if (qualified) at += 2;
  let name = tokens[at]?.text;
  at += 1;
  while (tokens[at]?.text === ":" && tokens[at + 1]?.text === ":") {
    qualified = true;
    name = tokens[at + 2]?.text;
    at += 3;
  }
  const args = tokens[at]?.text === "(" ? tokens.slice(at + 1, end - 2).map((token) => token.text) : undefined;
  return { line: tokens[open].line, name, qualified, args };
}

/**
 * Lists the test-only modules that a file-level scope split would count as production.
 * @param {string} source Rust source text.
 * @returns {{ line: number, problem: string }[]} One entry per offending module or inner attribute,
 *   in source order, at the line of its `mod` keyword or inner attribute.
 * @throws {Error} When a module's or inner attribute's `cfg` cannot be parsed, or a bracket is
 *   unclosed.
 */
export function misplacedTestModules(source) {
  const tokens = tokensOf(rustCode(source));
  const found = [];
  let depth = 0;
  let at = 0;
  while (at < tokens.length) {
    const { text } = tokens[at];
    if (text === "mod" && tokens[at + 1]?.text === "tests") {
      // Reached only without a test-only `cfg`: that case skips past `mod` below.
      found.push({ line: tokens[at].line, problem: "module `tests` is not test-only, yet its tests.rs is measured as test code" });
    }
    if (text !== "#") {
      if (text === "{") depth += 1;
      else if (text === "}") depth -= 1;
      at += 1;
      continue;
    }
    // A run of attributes on one item: `#[…]` groups, or one inner `#![…]`. A `#` that opens
    // neither (a macro pattern such as `#$x`) is skipped.
    const first = at;
    const start = tokens[at].line;
    const inner = tokens[at + 1]?.text === "!";
    const attributes = [];
    while (tokens[at]?.text === "#" && (tokens[at + 1]?.text === "[" || tokens[at + 1]?.text === "!")) {
      const open = tokens[at + 1].text === "!" ? at + 2 : at + 1;
      if (tokens[open]?.text !== "[") break;
      const end = groupEnd(tokens, open);
      attributes.push(attributeOf(tokens, open, end));
      at = end;
      if (inner) break;
    }
    if (at === first) {
      at += 1;
      continue;
    }
    let item = at;
    if (!inner) {
      if (tokens[item]?.text === "pub") {
        item += 1;
        if (tokens[item]?.text === "(") item = groupEnd(tokens, item);
      }
      // Attributes of other items are not evaluated: their test-only code stays in the production figure.
      if (tokens[item]?.text !== "mod") continue;
    }
    const line = inner ? start : tokens[item].line;
    // `cfg_attr` can add `cfg` or `path`, and a qualified path names `cfg` under another spelling;
    // neither is evaluated, so the module is reported rather than guessed.
    const hidden = attributes.find(({ name, qualified }) => name === "cfg_attr" || (qualified && name === "cfg"));
    let testOnly = false;
    if (!hidden) {
      const trees = [];
      let count = 0;
      for (const cfg of attributes.filter(({ name }) => name === "cfg")) {
        try {
          if (cfg.args === undefined) throw new Error("`cfg` without a predicate");
          trees.push(parseCfg(cfg.args, () => `=${count++}`));
        } catch (error) {
          throw new Error(`line ${cfg.line}: ${error.message}`);
        }
      }
      try {
        testOnly = valueWithoutTest(trees) === "F";
      } catch (error) {
        throw new Error(`line ${start}: ${error.message}`);
      }
    }
    if (inner) {
      if (hidden) found.push({ line, problem: `an inner \`${hidden.name}\` attribute is not modelled; it may make the code around it test-only` });
      else if (testOnly) found.push({ line, problem: depth === 0 ? "an inner test-only `cfg` makes the whole production file test code" : "an inner test-only `cfg` makes the enclosing item test code" });
      continue;
    }
    const name = tokens[item + 1]?.text;
    const body = tokens[item + 2]?.text;
    if (!hidden && !testOnly) continue;
    if (hidden) {
      found.push({ line, problem: `\`${hidden.name}\` on module \`${name}\` is not modelled; use a plain \`#[cfg(test)] mod tests;\`` });
    } else if (!/^[\p{XID_Start}_]/u.test(name ?? "")) {
      found.push({ line, problem: "a test-only module whose name comes from a macro is not modelled; declare it as `mod tests;` outside the macro" });
    } else if (body === "{") {
      found.push({ line, problem: `inline test module \`${name}\`` });
    } else if (name !== "tests" || attributes.some(({ name }) => name === "path")) {
      found.push({ line, problem: `out-of-line test module \`${name}\` does not load a tests.rs; declare it as \`mod tests;\` without #[path]` });
    }
    // A reported module's body is not scanned again: its nested modules are part of the same report.
    at = body === "{" ? groupEnd(tokens, item + 2) : item + 2;
  }
  return found;
}
