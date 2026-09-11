import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";

const codeqlConfig = ".github/codeql/codeql-config.yml";
const codeqlWorkflow = ".github/workflows/codeql.yml";
const legacyConfig = ".github/codeql-config.yml";
const malformedFixture = "fixtures/corpus/malformed-matcher/steps.js";

test("CodeQL advanced setup excludes the intentionally malformed fixture", () => {
  assert.equal(existsSync(codeqlConfig), true);
  assert.equal(existsSync(codeqlWorkflow), true);
  assert.equal(existsSync(legacyConfig), false);

  const configuration = readFileSync(codeqlConfig, "utf8").replaceAll("\r\n", "\n");
  assert.match(configuration, /^paths-ignore:\n(?:  .*\n)*  - fixtures\/corpus\/malformed-matcher\/steps\.js$/m);

  const workflow = readFileSync(codeqlWorkflow, "utf8").replaceAll("\r\n", "\n");
  assert.match(workflow, /config-file: \.\/\.github\/codeql\/codeql-config\.yml/);
  assert.doesNotMatch(workflow, /github\/codeql-action\/(?:init|analyze)@v\d/);
  assert.equal(existsSync(malformedFixture), true);
});
