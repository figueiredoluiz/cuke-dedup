export function renderPlatformReadme({ name, description }) {
  return `# ${name}

${description}.

This is an optional native binary package for
[CukeDedup](https://www.npmjs.com/package/cuke-dedup), a Rust-powered static
analyzer for duplicate and reusable Cucumber step definitions.

Do not install this package directly. Install the main package instead:

\`\`\`bash
npm install --save-dev cuke-dedup
# or
npm install --global cuke-dedup
\`\`\`

This package contains only the native \`cuke-dedup\` binary for its target
platform, plus package metadata, legal notices, and this README.

Supply-chain notes:

- no runtime dependencies;
- no install scripts or postinstall downloads;
- published from the project GitHub Actions workflow with npm provenance enabled;
- npm registry signatures and provenance can be checked with
  \`npm audit signatures\` from an installed project.
`;
}
