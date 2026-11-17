import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { childEnv, spawnExited } from "./spawn-exited.mjs";

const options = { encoding: "utf8", env: childEnv() };
const node = process.execPath;
// Children that overflow or time out stay alive afterwards, so the parent always kills them.
const block = "setTimeout(() => {}, 30_000)";

/** `"command" ["args"] failed:` as the message starts. */
const head = (command, args) => `${JSON.stringify(command)} ${JSON.stringify(args)} failed: `;

/** Validator: exact message; `cause` present with `code` only when a spawn error is expected. */
const fails = (message, code) => (error) => {
  assert.equal(error.message, message);
  assert.equal(Object.hasOwn(error, "cause"), code !== undefined, "cause only with a spawn error");
  assert.equal(error.cause?.code, code);
  return true;
};

/** An exit is returned whatever its status; a timeout, overflow, signal or launch failure throws naming command, args, outcome and output. */
test("spawnExited", () => {
  const failed = spawnExited(node, ["-e", "process.stderr.write('why'); process.exit(3)"], options);
  assert.deepEqual([failed.status, failed.signal, failed.error, failed.stderr], [3, null, undefined, "why"], "a failing exit is a result");
  assert.throws(() => spawnExited(node, ["-e", block], { ...options, timeout: 2_000 }),
    fails(`${head(node, ["-e", block])}error ETIMEDOUT, signal SIGKILL, status null`, "ETIMEDOUT"));
  const stubborn = `process.on('SIGTERM', () => {}); ${block}`;
  const started = Date.now();
  assert.throws(() => spawnExited(node, ["-e", stubborn], { ...options, timeout: 2_000 }),
    fails(`${head(node, ["-e", stubborn])}error ETIMEDOUT, signal SIGKILL, status null`, "ETIMEDOUT"));
  assert.ok(Date.now() - started < 20_000, "a child ignoring SIGTERM is still killed at the timeout");
  const overflow = `require('node:fs').writeSync(1, 'x'.repeat(4096)); ${block}`;
  assert.throws(() => spawnExited(node, ["-e", overflow], { ...options, maxBuffer: 1024 }), (error) => {
    const prefix = head(node, ["-e", overflow]);
    assert.ok(error.message.startsWith(prefix), error.message);
    assert.match(error.message.slice(prefix.length), /^error ENOBUFS, signal SIGKILL, status null; stdout: (\.\.\.)?x+$/);
    assert.equal(error.cause.code, "ENOBUFS");
    return true;
  });
  const missing = join(tmpdir(), "spawn exited no such command");
  assert.throws(() => spawnExited(missing, ["a b", "c"], options), fails(`${head(missing, ["a b", "c"])}error ENOENT, signal null, status null`, "ENOENT"));
  if (process.platform !== "win32") {
    // A self-sent signal ends a Windows child with an exit code, not a signal. Synchronous writes land before the kill.
    const killed = "const fs = require('node:fs'); fs.writeSync(1, '\\u{1F600}'.repeat(2500) + 'END\\n'); fs.writeSync(2, 'bye\\n'); process.kill(process.pid, 'SIGKILL')";
    assert.throws(() => spawnExited(node, ["-e", killed], options),
      fails(`${head(node, ["-e", killed])}error none, signal SIGKILL, status null; stdout: ...${"\u{1F600}".repeat(1997)}END; stderr: bye`, undefined));
  }
});

/** childEnv drops NODE_* in any case and keeps the rest. */
test("childEnv", () => {
  const names = ["NODE_SPAWN_EXITED_PROBE", "node_spawn_exited_probe", "SPAWN_EXITED_NODE_PROBE"];
  const saved = names.map((name) => [name, process.env[name]]);
  try {
    for (const name of names) process.env[name] = "1";
    const env = childEnv();
    // On Windows the first two names are one variable; either way neither survives.
    assert.deepEqual(names.map((name) => Object.hasOwn(env, name)), [false, false, true]);
    assert.ok(Object.keys(env).some((name) => name.toUpperCase() === "PATH"), "PATH kept");
  } finally {
    for (const [name, value] of saved) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
});
