# Changelog

All notable changes to CukeDedup are documented in this file. The project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-09-08

### Changed

- Upgraded `tree-sitter` to 0.27.0 and `taiki-e/install-action` to 2.87.5.

### Fixed

- Added package-specific READMEs to native npm distributions.
- Adopted conventional `windows-*-msvc` names for the Windows npm packages.
- Made npm publication safe to retry after a partial release.

## [0.1.0] - 2026-09-07

Initial public release.

### Added

- Static duplicate, near-duplicate, ambiguous, reusable, and unused Cucumber step analysis.
- JavaScript, JSX, TypeScript, and TSX source adapters and Gherkin Markdown support.
- Terminal, JSON, JSON Lines, HTML, and SARIF reporters.
- Duplication thresholds, semantic baselines, changed-file analysis, suppressions, and ignore files.
- Native Cargo and npm distributions for eight supported targets.
- A checksum-verified GitHub Action and an agent-oriented CukeDedup skill.

[0.1.1]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/figueiredoluiz/cuke-dedup/releases/tag/v0.1.0
