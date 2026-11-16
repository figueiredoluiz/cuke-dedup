// Validate copyable Action examples wherever they appear in project documentation or workflows.
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

const SKIPPED_DIRECTORIES = new Set(["node_modules", "target", "dist", ".git", "corpus"]);
const SEARCHED_EXTENSIONS = [".md", ".markdown", ".yml", ".yaml"];

const ACTION_REFERENCE = /(?<![\w./-])figueiredoluiz\/cuke-dedup@([^\s#'"`,}\]]*)/gi;
const INLINE_TOKEN = /(?:^|[ \t:\[{,])(?:"(?:[^"\\]|\\.)*"|'(?:[^']|'')*')|([ \t]+#.*$)/g;
const MARKDOWN_CODE_SPAN = /(?<!`)(`+)(?!`)(.*?)(?<!`)\1(?!`)/g;
const RELEASE_COMMENT = /^[ \t]+#[ \t]*(v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)(?=\s|$)/i;
const BLOCK_HEADER = /:[ \t]*(?:[&!]\S+[ \t]+)*[|>](?:[1-9][+-]?|[+-][1-9]?)?([ \t]*(?:#.*)?)$/;
const COMMIT_SHA = /^[0-9a-f]{40}$/;

/** Reads the first real inline comment only when it follows the Action reference. */
function inlineComment(line, referenceEnd) {
  for (const match of line.matchAll(INLINE_TOKEN)) {
    if (match[1]) {
      const hash = match.index + match[1].indexOf("#");
      return hash >= referenceEnd ? match[1] : undefined;
    }
  }
}

/** Looks for an Action annotation within its Markdown snippet and on the surrounding line. */
function actionInlineComment(line, referenceStart, referenceEnd, markdown) {
  if (markdown) {
    for (const span of line.matchAll(MARKDOWN_CODE_SPAN)) {
      const start = span.index + span[1].length;
      const code = span[2];
      if (referenceStart >= start && referenceEnd <= start + code.length) {
        const comment = inlineComment(code, referenceEnd - start);
        if (comment && RELEASE_COMMENT.test(comment)) return comment;
        break;
      }
    }
    line = line.replace(MARKDOWN_CODE_SPAN, (span) => " ".repeat(span.length));
  }
  return inlineComment(line, referenceEnd);
}

/** Reads a literal Action reference and its version comment from Markdown or YAML. */
function* actionPins(text, markdown) {
  const lines = text.split(/\r?\n/);
  for (const [index, line] of lines.entries()) {
    for (const match of line.matchAll(ACTION_REFERENCE)) {
      const [reference, sha] = match;
      const prefix = line.slice(0, match.index);
      const block = /^ +$/.test(prefix) ? lines[index - 1]?.match(BLOCK_HEADER) : null;
      const comment = block?.[1] ?? actionInlineComment(line, match.index, match.index + reference.length, markdown);
      yield [sha, comment?.match(RELEASE_COMMENT)?.[1]];
    }
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
export async function checkActionPins(root = ".") {
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
    }
  }
}
