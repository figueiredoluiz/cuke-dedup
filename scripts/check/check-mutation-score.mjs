import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { pathToFileURL } from "node:url";

export function checkMutationScore(report, minimumScore) {
  assert.ok(Number.isFinite(minimumScore) && minimumScore >= 0 && minimumScore <= 100);
  for (const field of ["caught", "missed", "timeout", "unviable", "success", "total_mutants"]) {
    assert.ok(Number.isInteger(report[field]) && report[field] >= 0, `${field} must be a non-negative integer`);
  }
  assert.equal(
    report.caught + report.missed + report.timeout + report.unviable + report.success,
    report.total_mutants,
    "mutation outcome counts do not match total_mutants",
  );
  assert.ok(
    report.outcomes?.some(
      (outcome) => outcome.scenario === "Baseline" && outcome.summary === "Success",
    ),
    "unmutated baseline did not complete successfully",
  );
  assert.equal(
    report.success,
    0,
    "some mutants were not fully tested; run cargo-mutants without --check",
  );
  const scored = report.caught + report.missed + report.timeout;
  assert.ok(scored > 0, "mutation run did not produce any viable mutants");
  const score = (report.caught / scored) * 100;
  assert.equal(report.timeout, 0, "timed-out mutants are not acceptable");
  assert.ok(
    score >= minimumScore,
    `mutation score ${score.toFixed(1)}% is below ${minimumScore.toFixed(1)}%`,
  );
  return score;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const path = process.argv[2] || "mutants.out/outcomes.json";
  const minimumScore = Number(process.argv[3] || "90");
  const report = JSON.parse(await readFile(path, "utf8"));
  const score = checkMutationScore(report, minimumScore);
  console.log(
    `Mutation score: ${score.toFixed(1)}% (${report.caught}/${report.caught + report.missed + report.timeout} viable mutants caught; ${report.unviable} unviable).`,
  );
}
