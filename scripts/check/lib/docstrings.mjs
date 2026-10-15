import { execFileSync } from "node:child_process";
import { createRequire } from "node:module";
import { rustCode } from "./rust-docstring-code.mjs";
const require = createRequire(import.meta.url);

/** Parse Git's three-context-line hunks, retaining deletion boundaries. */
export function touchedLines(diff) {
  const touched = new Map();
  let current = null;
  let inHunk = false;
  for (const line of diff.split("\n")) {
    if (line.startsWith("diff --git ")) { current = null; inHunk = false; }
    if (!inHunk && line.startsWith("+++ ")) {
      const target = line.slice(4);
      current = target === "/dev/null" ? null : target.replace(/^b\//, "");
      if (current && (current.startsWith('"') || current.startsWith("/") || current.split("/").includes(".."))) {
        throw new Error("Unsupported Git diff path");
      }
      if (current) touched.set(current, []);
    } else if (current && line.startsWith("@@ ")) {
      const match = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@/.exec(line);
      if (!match) throw new Error("Malformed Git diff hunk");
      inHunk = true;
      const start = Number(match[1]);
      const count = Number(match[2] ?? 1);
      touched.get(current).push([Math.max(1, start), Math.max(1, start + Math.max(count, 1) - 1)]);
    }
  }
  return touched;
}

const RUST_FN = /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+)?fn\s+([A-Za-z_]\w*)/;
/** Select the supported source language. */
function language(path) {
  if (/\.rs$/.test(path)) return "rust";
  if (/\.[cm]?[jt]sx?$/.test(path)) return "js";
  if (/\.py$/.test(path)) return "python";
  return null;
}

/** Locate a Rust function's body end in source whose comments and literals are masked. */
function rustEnd(lines, start) {
  let depth = 0, opened = false, previous = "";
  const groups = [];
  const closers = { "(": ")", "[": "]", "{": "}" };
  for (let i = start; i < lines.length; i++) {
    for (const ch of lines[i]) {
      if (opened) {
        if (ch === "{") depth++;
        if (ch === "}" && --depth === 0) return i + 1;
      } else if (ch === "{" && !groups.length && previous !== "!") {
        opened = true;
        depth = 1;
      } else if (ch === ";" && !groups.length) return i + 1;
      else if (closers[ch]) groups.push(closers[ch]);
      else if (ch === "<" && !groups.includes("}")) groups.push(">");
      else if (ch === groups.at(-1) && !(ch === ">" && previous === "-")) groups.pop();
      if (!/\s/.test(ch)) previous = ch;
    }
  }
  throw new Error("Cannot determine Rust function body range");
}

/** Enumerate JS/TS callable ranges from the pinned parser, including test-runner callbacks. */
function javascriptFunctions(path, source) {
  let ts;
  try { ts = require("../tools/node_modules/typescript"); }
  catch { throw new Error("Install check tools with npm ci --prefix scripts/check/tools"); }
  const kind = /\.[cm]?tsx?$/.test(path) ? (path.endsWith("tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS)
    : (path.endsWith("jsx") ? ts.ScriptKind.JSX : ts.ScriptKind.JS);
  const file = ts.createSourceFile(path, source, ts.ScriptTarget.Latest, true, kind);
  if (file.parseDiagnostics.length) throw new Error(`Cannot parse ${path} for docstring validation`);
  const result = [];
  /** Retain a callable's own end and the statement start that owns its documentation. */
  function add(name, node, owner = node) {
    const offset = owner.getStart(file);
    const line = file.getLineAndCharacterOfPosition(offset).line;
    result.push({ name, line: line + 1, end: file.getLineAndCharacterOfPosition(node.end).line + 1,
      documented: documented(source.split("\n"), line, "js", source.slice(0, offset)).documented });
  }
  /** Walk syntax nodes without promoting a callback argument into a named result variable. */
  function visit(node) {
    if (ts.isFunctionDeclaration(node) && node.name) add(node.name.text, node);
    if (ts.isVariableDeclaration(node) && node.initializer && ts.isIdentifier(node.name)) {
      let initializer = node.initializer;
      while (ts.isParenthesizedExpression(initializer) || ts.isAsExpression(initializer)
        || ts.isTypeAssertionExpression(initializer) || ts.isSatisfiesExpression(initializer)) initializer = initializer.expression;
      if (ts.isArrowFunction(initializer) || ts.isFunctionExpression(initializer)) {
        const owner = ts.isVariableDeclarationList(node.parent) ? node.parent.parent : node;
        add(node.name.text, initializer, owner);
      }
    }
    if (ts.isCallExpression(node)) {
      let callee = node.expression;
      while (ts.isPropertyAccessExpression(callee)) callee = callee.expression;
      if (ts.isIdentifier(callee) && ["test", "it", "describe"].includes(callee.text)
        && node.arguments.some(arg => ts.isArrowFunction(arg) || ts.isFunctionExpression(arg))) {
        add(node.arguments[0]?.getText(file) ?? callee.text, node);
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(file);
  return result.sort((a, b) => a.line - b.line);
}

/** Check the documentation immediately preceding a declaration. */
function documented(lines, start, lang, before) {
  let i = start - 1;
  while (i >= 0 && (/^\s*(#\[|#!\[|@)/.test(lines[i]) || lines[i].trim() === "")) i--;
  if (i < 0 && before === undefined) return { documented: false, test: false };
  const attrs = lines.slice(i + 1, start).join("\n");
  const test = /#\[(test|tokio::test|rstest|cfg\(test\))/.test(attrs);
  const prefix = (before ?? lines.slice(0, i + 1).join("\n")).trimEnd();
  const block = /\/\*\*(?!\*)((?:(?!\*\/)[\s\S])*)\*\/$/.exec(prefix.slice(prefix.lastIndexOf("/**")));
  const hasBlock = Boolean(block && block[1].replace(/[\s*]/g, ""));
  const hasLine = lang === "rust" && /^\s*\/\/\/(?!\/)\s*\S/.test(lines[i] ?? "");
  return { documented: hasBlock || hasLine, test };
}

/** Enumerate Python function bodies and their literal docstrings using the standard parser. */
function pythonFunctions(source) {
  const script = `import ast, json, sys
module = ast.parse(sys.stdin.read())
print(json.dumps([{"name": n.name, "line": min([n.lineno] + [d.lineno for d in n.decorator_list]), "end": n.end_lineno, "documented": bool(ast.get_docstring(n))} for n in ast.walk(module) if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))]))`;
  return JSON.parse(execFileSync(process.platform === "win32" ? "python" : "python3", ["-c", script], {
    input: source, encoding: "utf8", timeout: 30_000, maxBuffer: 4 * 1024 * 1024, stdio: ["pipe", "pipe", "pipe"],
  }));
}

/** Measure touched functions using the repository's conservative local documentation rules. */
export function coverage(diff, readSource) {
  const functions = [];
  for (const [path, ranges] of touchedLines(diff)) {
    const lang = language(path);
    if (!lang) continue;
    const source = readSource(path);
    const lines = source.split("\n");
    const candidates = [];
    if (lang === "python") candidates.push(...pythonFunctions(source));
    else if (lang === "js") candidates.push(...javascriptFunctions(path, source));
    else {
      const code = rustCode(source).split("\n");
      for (let i = 0; i < code.length; i++) {
        const declared = RUST_FN.exec(code[i]);
        if (declared) candidates.push({ name: declared[1], line: i + 1, end: rustEnd(code, i),
          documented: documented(lines, i, lang).documented });
      }
    }
    for (const fn of candidates) {
      if (ranges.some(([a, b]) => a <= fn.end && b >= fn.line)) functions.push({ path, ...fn });
    }
  }
  const covered = functions.filter(fn => fn.documented).length;
  return { functions, covered, passed: covered === functions.length };
}
