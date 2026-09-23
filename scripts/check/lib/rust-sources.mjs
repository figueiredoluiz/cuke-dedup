// Pure normalization of `git ls-files` output for the duplication scope partition, extracted from
// check-duplication.mjs so it can be unit-tested without a repository. The shipped bug this guards
// (PR #43): during an unresolved merge a conflicted path appears once per stage — three times — so
// without de-duplication the scope-overlap assertion trips on an overlap that does not exist; and a
// path deleted without staging the deletion is still in the index, so it must be filtered by what
// is actually on disk rather than failing later with ENOENT.

/**
 * Splits `git ls-files` stdout into a de-duplicated list of paths, dropping blank lines.
 * @param {string} stdout Raw `git ls-files` output.
 * @returns {string[]} Unique, non-empty paths in first-seen order.
 */
export function parseGitSources(stdout) {
  return [...new Set(stdout.split("\n").filter(Boolean))];
}

/**
 * Keeps only the paths that still exist on disk. `exists` is injected so the filter is testable
 * without touching the filesystem.
 * @param {string[]} paths
 * @param {(path: string) => boolean} exists
 * @returns {string[]}
 */
export function filterExisting(paths, exists) {
  return paths.filter((path) => exists(path));
}
