# CI and baselines

CukeDedup can gate a repository through its CLI or GitHub Action. Start with warnings or a
baseline when introducing it to an established suite, then tighten policy deliberately.

## GitHub Action

The Action downloads the native binary for the runner and verifies its checksum, provenance
bundle, build identity, and archive contents before execution.

```yaml
permissions:
  contents: read

steps:
  - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7
    with:
      fetch-depth: 0
      persist-credentials: false

  - uses: figueiredoluiz/cuke-dedup@v0.2.1
    id: cuke-dedup
    with:
      path: .
      threshold: 5
      config: .cuke-dedup.json
      exclude: |
        generated/**
        fixtures/vendor/**
      reporters: terminal,json,html,sarif
      changed-since: ${{ github.event.pull_request.base.sha }}
      no-metrics: true

  - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7
    if: always()
    with:
      name: cuke-dedup-reports
      path: reports/cuke-dedup/
```

The Action exposes:

- `exit-code`
- `duplicate-rate`
- `duplicate-definitions`
- `total-definitions`
- `json-report`
- `html-report`
- `sarif-report`

It adds JSON internally when necessary so these outputs remain available. Omitted threshold,
reporter, and output inputs preserve project configuration; explicit Action inputs override it.
Inputs are passed directly to the native executable as an argument array.

Set `reporters: jsonl` for an agent-oriented stream in the step log. The Action still creates its
internal JSON report. Set `fail-on-incomplete: true` when truncated comparisons or an incomplete
corpus must fail with exit code `2`.

Downloads require the archive, adjacent SHA-256 checksum, provenance bundle, and exact expected
archive structure. Verification uses the GitHub CLI installed on GitHub-hosted runners and needs
no repository secret or expanded permission. A self-hosted runner must provide `gh` on `PATH`.

For security-sensitive workflows, pin CukeDedup to the release tag's full commit SHA. Use
`continue-on-error` only when a later step deliberately evaluates the `exit-code` output.

## Changed-file analysis

Report only findings that involve files changed from a Git revision while still comparing those
files against the complete corpus:

```sh
cuke-dedup . --changed-since origin/main
```

Paths remain correct from a repository subdirectory, and untracked files are included. Summary
definition counts and the duplication-threshold denominator describe the complete corpus.

An empty changed-file set produces a warning. Read, extraction, and feature-parse failures remain
fatal even for unchanged files because they can affect findings involving changed definitions.

## Semantic baselines

Create or replace a baseline from a complete analysis:

```sh
cuke-dedup . \
  --baseline .cuke-dedup-baseline.json \
  --update-baseline
```

Then allow only a configured number of findings beyond it:

```sh
cuke-dedup . --baseline .cuke-dedup-baseline.json --fail-on-new
cuke-dedup . --baseline .cuke-dedup-baseline.json --fail-on-new 3
```

`--fail-on-new` defaults to zero when no count is supplied and requires `--baseline` or
`--baseline-from-ref`. `--update-baseline` requires the file form and cannot be combined with
`--fail-on-new`. The Action exposes `baseline`, `baseline-from-ref`, and `fail-on-new` inputs.

Baselines store sorted semantic fingerprints with multiplicity counts. Renames and unrelated line
changes do not make a finding new, while an additional occurrence beyond the recorded count does.
Version `1` baselines from CukeDedup 0.1 can be replaced by running the version `2` tool with
`--update-baseline`; comparison rejects mismatched schemas.

Updates reject `--changed-since` and are skipped after incomplete analysis or an operational error,
preventing a partial scan from erasing accepted findings.

### Compare against a Git revision (next release)

Instead of committing a baseline file, compare with a revision that is already fetched locally:

```sh
cuke-dedup . --baseline-from-ref origin/main --fail-on-new
```

The corresponding Action inputs are:

```yaml
with:
  baseline-from-ref: ${{ github.event.pull_request.base.sha }}
  fail-on-new: "0"
```

This requires an Action/binary version containing this feature; v0.2.1 does not include it.
Use checkout with `fetch-depth: 0`, or explicitly fetch the desired revision before analysis.
The tool does not fetch revisions, initialize submodules, install dependencies, or run project code.

The base is scanned in a temporary independent Git checkout using the **current effective
CukeDedup configuration**, including rule overrides, patterns and suppressions. Historical
project module metadata and ignore files remain historical inputs. The same repository-relative
analysis directory must exist in both trees. Missing refs/directories, unsupported submodules,
base extraction errors, unresolved registrations, zero extraction from discovered sources,
definitions without a feature corpus, and truncated base comparisons fail with exit code `2`;
none become an empty accepted baseline. A genuinely empty historical suite is allowed.
Hooks and configured checkout filters are disabled, so Git LFS content is not hydrated.
The full repository snapshot is limited to 512 MiB of tracked content and 100,000 files;
larger repositories can use a committed baseline instead. Git submodules anywhere in that
snapshot are rejected because their content is not materialized.

Matching findings are suppressed using the existing semantic fingerprints and multiplicity
rules. Current modified/untracked files are analyzed normally. `--baseline-from-ref` conflicts
with `--baseline` and `--update-baseline`; it writes no baseline file or base reports and leaves
the current index, branch and worktree registration untouched. Temporary files are removed after
success or failure. This costs a second scan, not a changed-files-only optimization.

The new-finding allowance is an additional gate: it does not disable the duplication threshold
or independent error rules. Use `--fail-on-incomplete` if incomplete current analysis must also
fail instead of retaining the normal partial-report behavior.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The threshold passed and no independent error policy failed. |
| `1` | The threshold, an error-level rule, or the new-finding allowance failed. |
| `2` | Discovery, parsing, configuration, or another operational failure. |

Warnings alone do not produce exit code `1`. See [Rules](rules.md) for threshold behavior and
[Reports](reports.md) for generated artifacts.
