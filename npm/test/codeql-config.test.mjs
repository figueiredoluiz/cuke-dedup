import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";

const defaultSetupConfig = ".github/codeql/codeql-configuration.yml";
const legacyConfig = ".github/codeql-config.yml";
const malformedFixture = "fixtures/corpus/malformed-matcher/steps.js";

test("CodeQL default setup excludes the intentionally malformed fixture", () => {
  assert.equal(existsSync(defaultSetupConfig), true);
  assert.equal(existsSync(legacyConfig), false);

  const configuration = readFileSync(defaultSetupConfig, "utf8");
  assert.match(configuration, /^paths-ignore:\n(?:  .*\n)*  - fixtures\/corpus\/malformed-matcher\/steps\.js$/m);
  assert.equal(existsSync(malformedFixture), true);
});
