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

## JSON and HTML

JSON schema version `2` contains:

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

JSONL schema version `2` emits one compact record per finding and a final summary:

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
