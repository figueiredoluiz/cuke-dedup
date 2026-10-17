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
