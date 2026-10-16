import assert from "node:assert/strict";
import test from "node:test";

import {
  installedNpmVersion,
  MINIMUM_TRUSTED_PUBLISHING_NPM,
  supportsTrustedPublishing,
} from "../../scripts/release/check-npm-trusted-publishing.mjs";

/** Verify npm versions at or above the trusted-publishing minimum pass and older or malformed versions fail. */
test("trusted publishing enforces npm's documented minimum", () => {
  for (const version of ["11.5.1", "11.6.0", "12.0.0", "12.0.2-beta.1"]) {
    assert.equal(supportsTrustedPublishing(version), true, version);
  }
  for (const version of ["11.5.0", "11.4.9", "10.99.99", "invalid", "11.5"]) {
    assert.equal(supportsTrustedPublishing(version), false, version);
  }
  assert.equal(MINIMUM_TRUSTED_PUBLISHING_NPM, "11.5.1");
});

/** Verify installed npm validation returns supported versions and propagates unsupported-version, command, and spawn failures. */
test("installed npm validation fails closed on command and version errors", () => {
  assert.equal(
    installedNpmVersion(() => ({ status: 0, stdout: "11.5.1\n", stderr: "" })),
    "11.5.1",
  );
  assert.throws(
    () => installedNpmVersion(() => ({ status: 0, stdout: "11.5.0\n", stderr: "" })),
    /does not support trusted publishing/,
  );
  assert.throws(
    () => installedNpmVersion(() => ({ status: 9, stdout: "", stderr: "broken\n" })),
    /exit code 9: broken/,
  );
  assert.throws(
    () => installedNpmVersion(() => ({ error: new Error("missing npm") })),
    /missing npm/,
  );
});
