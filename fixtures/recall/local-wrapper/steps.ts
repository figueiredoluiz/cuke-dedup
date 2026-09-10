import { Given } from "@cucumber/cucumber";

function step(text: string, handler: () => Promise<void>) {
  Given(text, handler);
}

step("wrapped registration", async () => firstOperation());
step("wrapped registration", async () => secondOperation());
