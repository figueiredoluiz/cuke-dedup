# Safety and limitations

CukeDedup analyzes repositories that may contain malformed or adversarial input. It does not
execute project configuration, imported modules, package scripts, matchers, or test code. Explicit
resource limits keep failures visible and bound the work performed.

## Compatibility

- The minimum supported Rust version is 1.90.
- The npm launcher is tested on Node.js 20 and 24.
- Source adapters cover JavaScript and TypeScript, including JSX and common module variants.
- Unsupported source languages are rejected rather than guessed.
- Classic Gherkin and Gherkin Markdown are supported, including declared Gherkin dialects.
- Before version 1.0, configuration and machine-report schemas may evolve between minor releases.

## Static-analysis boundaries

- Dynamic framework configuration may require explicit `features` or `definitions` patterns.
- Named handlers declared in the same file resolve to their bodies. Imported or unresolved handler
  references remain usable for matcher and usage rules but are not compared by name.
- Malformed JavaScript and TypeScript fail closed. Unsupported regular-expression constructs warn;
  malformed matcher escapes are operational errors.
- One analysis root is one corpus. Analyze independent monorepo packages separately.
- Regular expressions are matched against feature steps but are never reversed into invented
  overlap witnesses.
- Bounded fuzzy indexes preserve a deterministic sample in highly repetitive vocabularies but do
  not promise every possible pair after a work limit is reached.

When analysis is incomplete, existing findings remain valid but the absence of a finding proves
nothing. Machine reports expose the incomplete or truncated status. Pass `--fail-on-incomplete`
when CI must reject degraded analysis.

## Input limits

| Input | Limit |
| --- | ---: |
| Definition source, feature file, imported registration module, or baseline | 8 MiB each |
| Package, CukeDedup, framework, or static project configuration | 1 MiB each |
| Project-resolution metadata files | 1,024 files / 16 MiB total |
| Project-resolution mappings | 16,384 |
| Workspace discovery entries | 100,000 |
| Imported registration modules | 1,024 modules / 64 MiB total |
| Memoized module path/depth states | 16,384 |

Resolution confines imports and package targets to canonical analysis or package roots and shares
cached work between importing files.

## Matcher and Gherkin limits

- A matcher pattern or compiled program is limited to 1 MiB.
- Each matcher receives a 2 MiB lazy-DFA cache.
- Oversized matcher sets split recursively into bounded sets rather than falling back to a full
  definition-by-step loop.
- Gherkin Markdown conversion collects at most 10,000 ambiguous candidates, performs at most 128
  parser probes, and parses at most 64 MiB of synthesized input.
- Default-English bullets use exact Gherkin keyword filtering and one whole-document probe.
- Dialect-dependent candidates use a whole-document retry only within the cumulative byte budget;
  larger inputs use bounded 32-item restoration batches.

## Analysis limits

| Work | Limit |
| --- | ---: |
| Unique definition comparisons and static overlap pairs | 2,000,000 |
| Structural proposals per handler class | 250,000 |
| Raw overlap witness proposals | Four times the remaining unique-pair budget |
| Retained overlap findings | 10,000 |
| Trigram keys per definition / per run | 4,096 / 250,000 |
| Matcher-blocking posting proposals | 2,000,000 |
| Combined handler events | 10,000,000 |
| Pairwise or overlap suppression work | 100,000,000 units each |
| Unmatched-suppression advisory work | 10,000,000 units |
| One matcher-edit or handler-LCS matrix | 100,000,000 estimated units |
| Aggregate verification and evidence work | 1,000,000,000 units |

Project configuration and CLI flags may lower candidate and structural limits but cannot raise the
hard ceilings. Matcher similarity is evaluated before reserving expensive handler-LCS work.

Fuzzy matcher discovery uses collision-free, fixed-width, case-folded character trigrams with
digit runs normalized. Unsaturated posting lists are fully considered. Lists above 256 definitions
use deterministic lexical neighbors instead of quadratic expansion. Candidates must be capable of
reaching the 50% handler-behavior gate and—unless structurally equivalent—share a canonical call
action.

Reaching a candidate-generation or verification limit preserves evaluated findings, warns, and
sets `analysis.truncated: true` with a nonzero skipped count. Exit status follows finding severity
unless `--fail-on-incomplete` is enabled. Baselines are never updated from incomplete analysis.

## Report safety

Reports bound source snippets and cluster memberships, escape HTML, encode script-closing JSON
characters, and remove bidirectional, invisible-format, and Unicode Tag characters. JSONL content
still originates in the analyzed repository and must be treated as untrusted data rather than
agent instructions.

See [Reports](reports.md) for presentation limits and [Discovery and frameworks](discovery-and-frameworks.md)
for corpus-completeness rules.
