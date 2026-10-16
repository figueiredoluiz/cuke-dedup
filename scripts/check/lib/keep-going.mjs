// Independent checks run to completion, so one run reports every failing check rather than the
// first. Under `check:equivalence` a stop would also leave the remaining analyzer invocations
// uncompared.

/**
 * Runs the check for case `name`, appending `{ name, error }` to `failures` instead of stopping the
 * run. The name is recorded because a bare assertion does not say which case it came from.
 */
export async function attempt(failures, name, check) {
  try {
    await check();
  } catch (error) {
    failures.push({ name, error });
  }
}

/** Writes every failure, its case name first, to stderr and sets exit code 1 when there is one; true when none. */
export function reportFailures(failures, label) {
  for (const { name, error } of failures) {
    console.error(`${name}:`);
    console.error(error);
  }
  if (failures.length === 0) return true;
  console.error(`${label}: ${failures.length} failing checks`);
  process.exitCode = 1;
  return false;
}
