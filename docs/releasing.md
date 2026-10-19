# Releasing

## Version sites

The version appears in both Cargo manifests, both Cargo lockfiles, the root and eight platform npm
manifests, the npm lockfile, the `action.yml` and `release.yml` version inputs, and the Action pin
in `README.md` and `docs/ci-and-baselines.md`. Never edit them by hand:

```bash
npm run bump:version -- 0.3.0
```

The script requires Git, updates every site and both lockfiles atomically, rejects unexpected file
shapes, restores changes if a lockfile refresh fails, and preserves line endings.

Then confirm and commit:

```bash
npm run check:versions
git diff
```

`check:versions` also runs in the development gate and in CI, where `EXPECTED_VERSION` and
`RELEASE_TAG` pin the agreed version to the dispatched release and its tag.

Release notes live in [GitHub Releases](https://github.com/figueiredoluiz/cuke-dedup/releases).

## Comparison with the previous release

`npm run release:compare` runs a previous release binary and a new one on every committed corpus case
(the behavior corpus and the recall corpus) and prints a markdown table for the release notes:

```bash
npm run release:compare -- --previous PATH --candidate target/release/cuke-dedup \
  --previous-label v0.13.0 --candidate-label v0.14.0
```

- Rows are findings per rule, total findings, definitions and feature steps analyzed, and the median
  analysis time per case. Counts are exact. Time is the report's own `analysisMs`, the median of
  `--repeats` runs (default 3), and is advisory: shared runners are noisy. Each run is killed after
  `--timeout-ms` (default 120000) and counts as failed.
- A case counts only when both releases produce a report with stable counts across repeats. Cases left
  out are listed under the table with the reason (the first ten, then a count; `--json` has all), and
  cases whose exit status changed are named. The command
  exits 1, with the first reasons on stderr and every reason in the `--json` output, when no case could
  be compared.
- `--markdown FILE` and `--json FILE` write the outputs; the table goes to stdout only when neither
  is given. The
  command takes two binaries and does not download or build anything. A different rule set between
  releases appears as a rule with `0` on one side.

## Prerequisites

- Cargo, npm, the Git tag and the GitHub release use one version.
- npm publication uses OIDC trusted publishing and never reads a long-lived registry token. Every
  package in the npm matrix must authorize the `figueiredoluiz/cuke-dedup` repository,
  `.github/workflows/release.yml` and the `npm` environment on npmjs.com, with direct
  `npm publish` allowed. The workflow rejects npm clients older than the trusted-publishing minimum.

## Experimental Ruby releases

An experimental Ruby release needs the ordinary project and packaging checks and the Ruby
regression gate. The Ruby completion census tracks work toward broader support; pending evidence
or design entries do not block an experimental release. Keep known limitations documented and
assess serious regressions and security exposure before publication.

The project maintainer has final responsibility for scope, compatibility and release decisions.
