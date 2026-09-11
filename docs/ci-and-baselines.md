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

  - uses: figueiredoluiz/cuke-dedup@v0.2.0
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

`--fail-on-new` defaults to zero when no count is supplied. It and `--update-baseline` both require
`--baseline` and cannot be combined. The Action exposes the same behavior through `baseline` and
`fail-on-new` inputs.

Baselines store sorted semantic fingerprints with multiplicity counts. Renames and unrelated line
changes do not make a finding new, while an additional occurrence beyond the recorded count does.
Version `1` baselines from CukeDedup 0.1 can be replaced by running the version `2` tool with
`--update-baseline`; comparison rejects mismatched schemas.

Updates reject `--changed-since` and are skipped after incomplete analysis or an operational error,
preventing a partial scan from erasing accepted findings.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The threshold passed and no independent error policy failed. |
| `1` | The threshold, an error-level rule, or the new-finding allowance failed. |
| `2` | Discovery, parsing, configuration, or another operational failure. |

Warnings alone do not produce exit code `1`. See [Rules](rules.md) for threshold behavior and
[Reports](reports.md) for generated artifacts.
