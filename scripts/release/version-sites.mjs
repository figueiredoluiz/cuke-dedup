// Validate copyable Action examples wherever they appear in project documentation or workflows.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

const SKIPPED_DIRECTORIES = new Set(["node_modules", "target", "dist", ".git", "corpus"]);
const SEARCHED_EXTENSIONS = [".md", ".markdown", ".yml", ".yaml"];

const ACTION_COORDINATE = "figueiredoluiz/cuke-dedup@";
const ACTION_REFERENCE = /(?<![\w./-])figueiredoluiz\/cuke-dedup@([^\s#'"`,}\]]*)/gi;
// After an escaped newline, the repeated character arm consumes indentation.
const INLINE_TOKEN = /(?:^|[ \t:\[{,])("(?:[^"\\\r\n]|\\(?:\r?\n|[^\r\n]))*"|'(?:[^'\r\n]|'')*')|((?:^|[ \t]+)#.*$)/gm;
const YAML_ESCAPE = /\\(?:\r?\n[ \t]*|x[\da-fA-F]{2}|u[\da-fA-F]{4}|U[\da-fA-F]{8}|[^\r\n])/g;
const MARKDOWN_CODE_SPAN = /(?<!`)(`+)(?!`)(.*?)(?<!`)\1(?!`)/g;
const MARKDOWN_FENCE = /^ {0,3}(`{3,}|~{3,})(.*)$/;
const MARKDOWN_INDENTED_CODE = /^(?: {4}| {0,3}\t)/;
const MARKDOWN_YAML_LINE = /^[ \t]*(?:[-?:>][ \t]+)*(?:[\[{]|(?:[\w.-]+|"[^"]+"|'[^']+')[ \t]*:)/;
const RELEASE_COMMENT = /^[ \t]+#[ \t]*(v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(?=\s|$)/i;
const USES_PREFIX = /(?:^([ ]*(?:-[ \t]+)*)|[,{][ \t]*)(?:uses|"uses"|'uses')[ \t]*:[ \t]*(?:[&!]\S+[ \t]+)*$/;
const BLOCK_HEADER = /:[ \t]*(?:[&!]\S+[ \t]+)*[|>]([1-9][+-]?|[+-][1-9]?)?([ \t]*(?:#.*)?)$/;
const FLOW_START = /^[ ]*(?:-[ \t]+)*(?:[^\[\]{},]+:[ \t]+)?[\[{]/;
const COMMIT_SHA = /^[0-9a-f]{40}$/;

/** Reads the first real inline comment only when it follows the Action reference. */
function inlineComment(line, referenceEnd) {
  for (const match of line.matchAll(INLINE_TOKEN)) {
    const comment = match[2];
    if (comment) {
      const hash = match.index + comment.indexOf("#");
      return hash >= referenceEnd ? comment : undefined;
    }
  }
}

/** Reads a version comment, allowing terminal punctuation when written in prose. */
function releaseTag(comment, prose) {
  const annotation = prose ? comment?.replace(/(?<=[^\s).;])[).;]+(?=\s|$)/, "") : comment;
  return annotation?.match(RELEASE_COMMENT)?.[1];
}

/** Reads an annotation from its own code span or from the text surrounding the spans. */
function actionInlineTag(line, referenceEnd, span, prose) {
  if (span) {
    const start = span.index + span[1].length;
    const tag = releaseTag(inlineComment(span[2], referenceEnd - start), false);
    if (tag) return tag;
  }
  if (prose) {
    line = line.replace(MARKDOWN_CODE_SPAN, (span) => " ".repeat(span.length));
  }
  return releaseTag(inlineComment(line, referenceEnd), prose);
}

/** Projects a YAML escape onto the ASCII characters that can form an Action coordinate. */
function coordinateEscape(escape) {
  const kind = escape[1];
  if (kind === "\r" || kind === "\n") return "";
  if (kind === "/" || kind === "\\") return kind;
  if (escape.length === 2) return "\0";
  const codepoint = Number.parseInt(escape.slice(2), 16);
  return codepoint < 128 ? String.fromCharCode(codepoint) : "\0";
}

/** Gives the literal scanner the result of validating the complete quoted Action value. */
function normalizeQuotedPins(text) {
  return text.replace(INLINE_TOKEN, (token, quoted) => {
    if (!quoted) return token;
    const contents = quoted.slice(1, -1);
    const decoded = quoted[0] === '"' ? contents.replace(YAML_ESCAPE, coordinateEscape) : contents;
    if (!decoded.trimStart().toLowerCase().startsWith(ACTION_COORDINATE)) return token;
    const pin = actionValueSha(decoded);
    return `${token.slice(0, -quoted.length)}"${ACTION_COORDINATE}${pin}"`;
  });
}

/** Returns the pin from a complete Action value, or an empty pin when the value is malformed. */
function actionValueSha(value) {
  if (!value.toLowerCase().startsWith(ACTION_COORDINATE)) return "";
  const sha = value.slice(ACTION_COORDINATE.length);
  return COMMIT_SHA.test(sha) ? sha : "";
}

/** Reads an Action value that starts beneath its key, bounded by the next dedent. */
function actionBody(lines, index) {
  const line = lines[index];
  const header = line.match(BLOCK_HEADER);
  const comment = inlineComment(line, 0);
  const key = header ? line.slice(0, header.index + 1) : line.slice(0, line.length - (comment?.length ?? 0));
  const prefix = key.match(USES_PREFIX)?.[1];
  if (prefix === undefined) return;

  const explicitIndent = Number(header?.[1]?.match(/[1-9]/)?.[0] ?? "0");
  let content;
  let contentIndent;
  let first;
  let multiple = false;
  let end = index + 1;
  for (; end < lines.length; end += 1) {
    const candidate = lines[end];
    const value = candidate.replace(/^ +/, "");
    const indent = candidate.length - value.length;
    const empty = header
      ? value.length === 0 && (contentIndent === undefined || indent <= contentIndent)
      : /^[ \t]*$/.test(value) || value.startsWith("#");
    if (empty) continue;
    if (indent <= prefix.length) break;
    if (first !== undefined) {
      multiple = true;
      continue;
    }
    first = end;
    contentIndent = explicitIndent ? prefix.length + explicitIndent : indent;
    content = indent < contentIndent ? "" : candidate.slice(contentIndent);
  }
  // Clip and keep retain a final line break when the content line has one.
  const keepsFinalBreak = header && !header[1]?.includes("-") && first < lines.length - 1;
  const sha = !multiple && !keepsFinalBreak && first === index + 1 ? actionValueSha(content) : "";
  return { end, multiple, literal: Boolean(header), sha, tag: releaseTag(header?.[2], false) };
}

/** Carries a flow collection across lines using delimiters outside quotes and comments. */
function continuedFlowDepth(line, depth) {
  const code = line.replace(INLINE_TOKEN, (token, quoted) => quoted ? `${token.slice(0, -quoted.length)}x` : "");
  if (depth === 0 && !FLOW_START.test(code)) return 0;
  for (const delimiter of code.matchAll(/[\[\]{}]/g)) {
    const opening = delimiter[0] === "[" || delimiter[0] === "{";
    depth = Math.max(0, depth + (opening ? 1 : -1));
  }
  return depth;
}

/** Reads a plain Action value up to the comment or enclosing flow delimiter. */
function plainActionSha(line, start, continuation, inFlow) {
  const prefix = line.slice(0, start);
  const key = prefix.match(USES_PREFIX);
  if (!key && !((continuation || inFlow) && /^[ \t]*$/.test(prefix))) return;
  const comment = inlineComment(line, start);
  const tail = line.slice(start, line.length - (comment?.length ?? 0));
  const flow = inFlow || (key && key[1] === undefined);
  const value = flow ? tail.split(/[,\]}]/, 1)[0] : tail;
  return actionValueSha(value.replace(/[ \t]+$/, ""));
}

/** Reads a literal Action reference and its version comment from Markdown or YAML. */
function* actionPins(text, markdown) {
  if (markdown) {
    text = text.replace(MARKDOWN_CODE_SPAN, (_, delimiter, code) =>
      `${delimiter}${normalizeQuotedPins(code)}${delimiter}`);
  }
  text = normalizeQuotedPins(text);
  const lines = text.split(/\r?\n/);
  let fence;
  let body;
  let flowDepth = 0;
  for (const [index, line] of lines.entries()) {
    if (body && index >= body.end) body = undefined;
    const marker = markdown && line.match(MARKDOWN_FENCE);
    if (marker) {
      flowDepth = 0;
      const [, run, tail] = marker;
      if (fence) {
        if (run[0] === fence[0] && run.length >= fence.length && !tail.trim()) fence = undefined;
      } else if (run[0] === "~" || !tail.includes("`")) {
        fence = run;
      }
    }
    const markdownText = markdown && !fence && !MARKDOWN_INDENTED_CODE.test(line);
    const prose = markdownText && flowDepth === 0 && !MARKDOWN_YAML_LINE.test(line);
    const spans = markdownText && flowDepth === 0 && !body ? [...line.matchAll(MARKDOWN_CODE_SPAN)] : [];
    for (const match of line.matchAll(ACTION_REFERENCE)) {
      const [reference, rawSha] = match;
      const end = match.index + reference.length;
      const span = spans.find(({ index: start, 0: source }) => match.index >= start && end <= start + source.length);
      if (body?.literal) {
        yield [body.sha, body.tag];
        continue;
      }
      const start = span ? match.index - span.index - span[1].length : match.index;
      const plainSha = !prose || span || body
        ? plainActionSha(span?.[2] ?? line, start, Boolean(body), !span && flowDepth > 0) : undefined;
      const tokenSha = prose && !span && !body ? rawSha.replace(/(?<![).;])[).;]+$/, "") : rawSha;
      const sha = body?.multiple ? "" : plainSha ?? tokenSha;
      const tag = actionInlineTag(line, end, span, prose || Boolean(span));
      yield [sha, tag];
    }
    if (!body && !prose) body = actionBody(lines, index);
    if (!prose && !body?.literal) flowDepth = continuedFlowDepth(line, flowDepth);
  }
}

/** Yields Markdown and YAML files below `directory`, skipping hidden and build directories. */
async function* searchableFiles(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.name.startsWith(".") && entry.name !== ".github") {
      continue;
    }
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      if (!SKIPPED_DIRECTORIES.has(entry.name)) {
        yield* searchableFiles(path);
      }
    } else if (SEARCHED_EXTENSIONS.some((extension) => entry.name.endsWith(extension))) {
      yield path;
    }
  }
}

/**
 * Markdown and YAML files Git tracks or would track under `root`. Outside a repository it falls
 * back to a directory walk; any other Git failure throws.
 */
function projectFiles(root) {
  const result = runGit(root, ["ls-files", "--cached", "--others", "--exclude-standard", "-z"]);
  if (result.status === 0) {
    return result.stdout.split("\0").filter((path) =>
      SEARCHED_EXTENSIONS.some((extension) => path.endsWith(extension))
      && !path.split("/").some((part) => SKIPPED_DIRECTORIES.has(part) || (part.startsWith(".") && part !== ".github")),
    ).map((path) => join(root, path)).filter(existsSync);
  }
  if (result.status === 128 && result.stderr.includes("not a git repository")) return searchableFiles(root);
  throw result.error ?? new Error(`Git version-site discovery failed: ${result.stderr}`);
}

/** Runs a bounded read-only Git query against the project. */
function runGit(root, args) {
  return spawnSync("git", ["-C", root, "--no-replace-objects", ...args], {
    encoding: "utf8", timeout: 10_000, maxBuffer: 16 * 1024 * 1024,
    env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !/^GIT_/i.test(key))), LC_ALL: "C" },
  });
}

/** Requires full commit pins and consistent release annotations for copyable Action examples. */
export async function checkActionPins(root = ".", { requireTags = false } = {}) {
  const commitsByTag = new Map();
  for await (const path of projectFiles(root)) {
    const markdown = /\.(?:md|markdown)$/.test(path);
    for (const [sha, tag] of actionPins(await readFile(path, "utf8"), markdown)) {
      if (!COMMIT_SHA.test(sha) || !tag) {
        throw new Error(`${path}: use a full commit SHA and a release version comment for the Action`);
      }
      if (commitsByTag.has(tag) && commitsByTag.get(tag) !== sha) {
        throw new Error(`${path}: conflicting Action commit pins for ${tag}`);
      }
      commitsByTag.set(tag, sha);
    }
  }
  if (commitsByTag.size === 0) throw new Error("no documented Action version pins were found");
  for (const [tag, sha] of commitsByTag) {
    const result = runGit(root, ["rev-parse", "--quiet", "--verify", `refs/tags/${tag}^{commit}`]);
    if (result.status === 0) {
      if (result.stdout.trim() !== sha) throw new Error(`Action commit pin does not match ${tag}`);
    } else if (result.status !== 1 && !result.stderr?.includes("not a git repository")) {
      throw result.error ?? new Error(`could not resolve Action pin tag ${tag}: ${result.stderr}`);
    } else if (requireTags) {
      throw new Error(`Action pin tag ${tag} is required for version validation`);
    }
  }
}
