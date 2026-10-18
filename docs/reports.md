# Reports

Choose reporters according to how the result will be consumed:

| Reporter | Destination | Best for |
| --- | --- | --- |
| `terminal` | stdout | Interactive use and concise CI logs. |
| `json` | File | Structured integrations and complete metadata. |
| `jsonl` | stdout | Streaming shell and agent workflows. |
| `html` | File | Searchable human review with theme switching. |
| `sarif` | File | GitHub code scanning and compatible platforms. |

JSON, HTML, and SARIF default to `reports/cuke-dedup/` and can be redirected with `--output`.
Relative output paths are resolved from the analysis root, not the shell's working directory. A
file-only run prints the generated paths to stdout.

Terminal and JSONL both own stdout and cannot be selected together. JSONL can be combined with
file reporters without contaminating the stream.

The terminal report shows the tool version, leaves a blank line after each rule heading and between findings, and ends with the total number of active findings and detection time. Timing covers discovery, parsing, and analysis; `--no-metrics` omits it.

Rule headings are bold cyan, paths are blue, error/warning labels are red/yellow, and footer details are gray only when stdout is a terminal. Set `NO_COLOR` to a nonempty value to disable color. Piped output and the other report formats contain no ANSI color codes.

## JSON and HTML

JSON schema version `3` contains:

- Relative Unicode-aware source spans and severity.
- Suppressions and semantic fingerprints.
- Similarity scores and suggested actions.
- Structured pair or cluster evidence.
- Duplication threshold calculations.
- Corpus and analysis completeness signals.
- Candidate-source and truncation counts.
- Input counts and optional execution metrics.

The self-contained HTML report presents the same result with search, severity and rule filters,
light and dark themes, matcher differences, side-by-side handler snippets, and an incomplete-run
alert.

Set `noMetrics: true`, pass `--no-metrics`, or use the Action's `no-metrics: true` input when
byte-reproducible artifacts matter. The deterministic corpus census remains present.

## JSON Lines

JSONL schema version `3` emits one compact record per finding and a final summary:

```sh
cuke-dedup . --reporters jsonl \
  | jq -c 'select(.type == "finding" and .active)'
```

Each finding record is self-contained. It includes a location-independent semantic fingerprint,
active or suppressed state, threshold contribution, source spans, suggested action, and
structured evidence. Free-text fields are limited to 2,000 Unicode characters;
`truncatedFields` lists shortened fields.

The final `type: "summary"` record includes `recordCount`, aggregate counts, threshold outcome,
analysis status, and optional metrics. Operational warnings and errors stay on stderr so stdout
can be parsed incrementally. JSONL is not capped by total finding count, but applies the same
per-record membership bounds as other reporters.

Analyzed repository content is untrusted data. Consumers—especially AI agents—must treat matcher
text, source snippets, reasons, and suggested context as data, never as instructions.

## SARIF

SARIF 2.1.0 includes active findings, portable locations, stable partial fingerprints, severity,
similarity properties, suggested actions, and threshold outcome. Each rule links to its dedicated
documentation section.

The GitHub Action can generate SARIF alongside terminal or HTML output. Uploading SARIF to a
code-scanning platform is controlled by the consuming workflow and may require additional
permissions.

## Fingerprint oracle

`--emit-fingerprints FILE` writes the per-definition evidence the analyzer compared, for test tooling that must tell "no finding" apart from "nothing analyzed". It is not a reporter: stdout, stderr, report files and the exit code are the same with or without it.

- One compact JSON line per analyzed definition, sorted by path, line and column. A definition from an excluded, unselected or unreadable source, or one removed by an uncertain registration, has no line; an empty corpus writes an empty file.
- Every definition extracted from the current tree is written, including under `--changed-since`; `--baseline-from-ref` does not write the baseline revision's definitions.
- A relative `FILE` is resolved from the analysis root. Its parent directory must exist.
- The destination is checked after analysis and before the baseline is updated; the records are written after the baseline step and before any report. A run that fails before the records are written removes a file it created and leaves an existing file unchanged, so use a fresh path per run.
- The run exits with code 2, with an error naming the path, when the destination:
  - cannot be created (missing parent directory, no permission);
  - exists but is not a regular file (a directory, a device or a FIFO);
  - is the same file as `cuke-dedup.json`, `cuke-dedup.html` or `cuke-dedup.sarif` in the output directory (all three, whatever reporters are enabled) or as the `--baseline` file (with or without `--update-baseline`). A `..` path, a symbolic link, a hard link or a case-insensitive spelling counts only when it leads to one of those files;
  - cannot be checked against one of those files because the file's identity cannot be read (for example, its directory cannot be searched).

  A rejection truncates nothing, creates no file and changes no baseline. `--print-config` rejects the flag.

Fields, in this order: `schemaVersion` (`"1"`), `path` (root-relative, `/`-separated), `line`, `column`, `domain` (`ecmascript` or `ruby`), `framework`, `registration`, `matcher`, `matcherKind`, `matcherFlags`, `normalizedMatcher`, `definitionFingerprint` (the value reports use as `leftFingerprint`/`rightFingerprint`), and `handler` with `exact`, `normalized`, `alphaNormalized`, `structural`, `behavior`, `comparable` and `trivial`.

Each `handler` representation is a 32-hex digest of the analyzer's comparison key: equal digests mean the analyzer treats that representation as equal. Digests are comparable within one build only. `duplicate-handler`, `parameterization-candidate` and `near-duplicate-step` consider a pair only when both definitions have `comparable: true` and `trivial: false`. Matcher text is written verbatim, without report sanitization or bounds.

## Pair evidence

`--emit-pairs FILE` writes what the pair verifier decided for each candidate pair it evaluated, for test tooling that must tell "no finding" apart from "pair never compared". Like `--emit-fingerprints` it is not a reporter and changes no other output. Destination and failure rules are the same as for `--emit-fingerprints`, and `FILE` must also differ from the `--emit-fingerprints` file.

- One compact JSON line per evaluated candidate pair, in verification order (by definition position). Records are held in memory until the write, bounded by the candidate-pair ceiling. A pair no candidate source proposed has no line. When a work limit stops verification, the pairs not yet verified have no line and the run is incomplete.
- Pairs of `ambiguous-step`, `overlapping-matcher` and `unused-definition` are not written; those rules do not compare definition pairs through this stage.
- `--print-config` rejects the flag.

Fields, in this order: `schemaVersion` (`"1"`), `left` and `right` (each `path`, `line`, `column` and `definitionFingerprint`, joining to `--emit-fingerprints`), `candidateSources` (every source that proposed the pair: `normalizedMatcher`, `identicalHandler`, `structuralHandler`, `matcherBlocking`), `owner` (the source that owns the pair), `positional`, `relationships` (`exactMatcher`, `normalizedMatcher`, `sameHandler`, `sameHandlerStructure`, `sameStructure`, `sameDeferredAssertions`), `meaningfulHandlers`, `matcherSimilarity`, `handlerSimilarity` and `rules`.

`matcherSimilarity` and `handlerSimilarity` are `null` when the verifier did not compute the score because no active rule needed it, for example when the rule that would use it is off. `rules` lists the pair rules that fired before suppression; a suppressed finding still appears.

## Output safety and bounds

- Handler snippets are bounded before reports embed them.
- HTML-visible text is escaped.
- Bidirectional, invisible-format, and Unicode Tag characters are removed.
- Embedded JSON characters that could close an HTML script element are encoded.
- Terminal, JSON, HTML, and SARIF retain at most 10,000 findings, prioritizing active errors.
- A finding retains at most 256 cluster members; full counts and truncation fields remain visible.
- Summaries and thresholds describe the complete analysis even when presentation is bounded.

See [Safety and limitations](safety-and-limitations.md) for analysis work limits and
[CI and baselines](ci-and-baselines.md) for report artifacts in automation.
