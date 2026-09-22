// The registration is imported under a new name from the CommonJS barrel, so no built-in
// registration name reaches the call site: resolving the barrel's `module.exports` is the only
// evidence these are step definitions. If that resolution regresses, both definitions vanish and
// the duplicate-matcher finding below disappears.
import { Given as registerStep } from "./barrel";

registerStep("cjs barrel registration", async () => firstOperation());
registerStep("cjs barrel registration", async () => secondOperation());
