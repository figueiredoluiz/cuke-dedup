// spawnSync for tests and checks that need the child's exit status: a run that did not complete
// (launch failure, timeout, output overflow, signal) throws its cause instead of returning
// `status: null`, which an exit-code assertion would report as `null !== 0`. A timeout or overflow
// kills with SIGKILL: spawnSync waits for the child to exit, and SIGTERM can be ignored.
import { spawnSync } from "node:child_process";

// Output kept in a failure message: enough to show how far a hung child got.
const TAIL = 2000;

/** The process environment without NODE_* variables (any case: Windows names are case-insensitive), which change how a child node starts. */
export function childEnv() {
  return Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.toUpperCase().startsWith("NODE_")));
}

/** `; label: text` with the last TAIL code points of captured output (no split surrogate pair), or "" when there is none. */
function tail(label, output) {
  const text = output ? String(output).trimEnd() : "";
  if (!text) return "";
  const chars = Array.from(text);
  return `; ${label}: ${chars.length > TAIL ? `...${chars.slice(-TAIL).join("")}` : text}`;
}

/**
 * Run `command` with `args` and spawnSync `options`; return the result of a child that exited,
 * whatever its status. A spawn error or a signal throws, with the spawn error as `cause` when
 * there is one; spawnSync errors are system errors, so `code` is always set. `options.killSignal`
 * defaults to SIGKILL.
 */
export function spawnExited(command, args, options) {
  // pattern-ok: env is the caller's option; every caller passes childEnv().
  const result = spawnSync(command, args, { killSignal: "SIGKILL", ...options });
  if (result.error || result.signal) {
    const outcome = `error ${result.error?.code ?? "none"}, signal ${result.signal}, status ${result.status}`;
    // pattern-ok: output is read only to describe a run already known to have failed.
    const message = `${JSON.stringify(command)} ${JSON.stringify(args)} failed: ${outcome}${tail("stdout", result.stdout)}${tail("stderr", result.stderr)}`;
    throw new Error(message, result.error ? { cause: result.error } : undefined);
  }
  return result;
}
