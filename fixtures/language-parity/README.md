# Paired language corpus

This initial matrix runs original, semantically paired Cucumber.js/TypeScript and Cucumber-Ruby scenarios against independent expected outcomes. It covers positive and negative cases for all eight rules, plus selected discovery and assertion boundaries. It is not an exhaustive framework or language parity claim and does not replace the larger Ruby census.

Build the released baseline from the exact `baselineCommit` in `manifest.json` in an isolated checkout. Build the current binary from the proposed source tree, then run:

```sh
node scripts/check/check-language-parity.mjs target/debug/cuke-dedup /path/to/pinned-baseline/target/debug/cuke-dedup /tmp/language-parity.json
node --test scripts/check/lib/language-parity.test.mjs
```

The caller is responsible for building the baseline from the pinned commit. The report records the expected commit and SHA-256 hashes of both executable artifacts and the manifest; a filename or `--version` string alone does not prove source provenance. The checker never executes fixture code. It materializes each scenario in an isolated temporary directory and statically scans it with identical feature selection, reporting and strictness options.

The three lanes are released TypeScript, current TypeScript and current Ruby. TypeScript drift compares the first two lanes. Ruby parity compares the latter two. Comparison retains definition and feature counts, completeness, strict exit, truncation and active findings with their rule, severity and logical primary/related locations. Fingerprint IDs, language-specific source spelling, diagnostic wording and numeric confidence scores are intentionally outside this initial comparison. The independent oracle checks each case's stated rule obligation and required non-findings; it does not certify every possible finding on that source.

Classifications are `equivalent`, `ruby-behind`, `typescript-behind`, `shared-limitation`, `both-have-gaps`, `outcome-difference` and `unmeasured`. A passing negative case cannot establish support for that rule's positive cases. In particular, Ruby's disabled parameterization path can satisfy a prohibition without proving better semantic analysis. Two implementations that fail the independent oracle are never classified as successfully equivalent. Execution failures remain unmeasured. Language-specific exclusions require a separate justified disposition; this manifest currently contains paired cases only.

Exit zero requires every paired case to meet its oracle, equal current outcomes, a usable released baseline and no observed TypeScript drift. Known differences deliberately keep this exploratory command failing; do not add it to a mandatory release gate until the desired behavior gaps are resolved or explicitly dispositioned. The JSON report remains available on a completed measurement with gaps. Group cases by behavioral contract when assessing completion; neither the case count nor a raw percentage is an exhaustive parity metric.
