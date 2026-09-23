// Pure source parsing for the documentation-parity gate, extracted from check-docs-parity.mjs so
// the struct/flag/key extraction can be unit-tested without the real source tree. The parser must
// never silently return fewer surfaces than exist — a dropped flag is exactly what would let an
// undocumented one pass this gate — so `cliFlags` handles multi-line `#[arg(...)]` blocks and
// `long = "override"`, and the caller floors the counts.
import assert from "node:assert/strict";

/** Returns the brace-matched body of the first `struct <name> { ... }` in `source`. */
export function structBody(source, name) {
  const start = source.indexOf(`struct ${name} {`);
  assert.notEqual(start, -1, `struct ${name} not found`);
  let depth = 0;
  const open = source.indexOf("{", start);
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === "{") depth += 1;
    else if (source[i] === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(open + 1, i);
    }
  }
  throw new Error(`unterminated struct ${name}`);
}

export const kebab = (field) => field.replace(/_/g, "-");
export const camel = (field) => field.replace(/_([a-z])/g, (_, c) => c.toUpperCase());

/**
 * Every `--long` flag declared by clap on the given struct body. `long = "x"` overrides the
 * kebab-cased field name; a bare `long` derives it. The `#[arg(...)]` block is matched across
 * newlines so a multi-line attribute is never skipped.
 * @param {string} structBodyText Body from `structBody(cliSource, "CheckOptions")`.
 * @returns {string[]}
 */
export function cliFlags(structBodyText) {
  const flags = [];
  const pattern =
    /#\[arg\(([\s\S]*?)\)\]\s*(?:(?:#\[[\s\S]*?\]|\/\/\/[^\n]*)\s*)*([a-z][a-z0-9_]*)\s*:/g;
  for (const [, argAttr, field] of structBodyText.matchAll(pattern)) {
    if (!/\blong\b/.test(argAttr)) continue;
    const override = argAttr.match(/long\s*=\s*"([^"]+)"/);
    flags.push(`--${override ? override[1] : kebab(field)}`);
  }
  return flags;
}

/**
 * Every camelCased config key declared on the given struct body (serde `rename_all = "camelCase"`).
 * @param {string} structBodyText Body from `structBody(configSource, "RawConfig")`.
 * @returns {string[]}
 */
export function configKeys(structBodyText) {
  const keys = [];
  for (const raw of structBodyText.split("\n")) {
    const field = raw.trim().match(/^([a-z][a-z0-9_]*)\s*:/);
    if (field) keys.push(camel(field[1]));
  }
  return keys;
}
