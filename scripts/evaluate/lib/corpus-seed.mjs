import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { posix } from "node:path";
import { sealProject } from "./corpus-replay.mjs";

const manifest = JSON.parse(readFileSync(new URL("../../../fixtures/corpus/manifest.json", import.meta.url), "utf8"));
const seed = manifest.cases.find((item) => item.name === "mixed-esm-and-cjs-registration-syntax");

/** The owned exact-text case uses one shared value and two independent logical definitions. */
function seedModel() {
  return {
    scenario: { name: "Shared registration", value: "a shared module registration", outline: false },
    definitions: [
      { id: "prepare", file: "esm.mjs", binding: "Setup", form: "named", helper: "prepareWorkspace" },
      { id: "restore", file: "common.cjs", binding: "cucumber.Given", form: "namespace", helper: "restoreWorkspace" },
    ],
    provider: "@cucumber/cucumber",
    helperFile: null,
    config: null,
    arguments: [...seed.arguments],
    applicability: "valid",
  };
}

/** Apply one named operation only inside its supported domain; terminal cases cannot be composed further. */
function apply(model, operation) {
  assert.equal(model.applicability, "valid", "cannot transform an invalid or unsupported project");
  const esm = model.definitions[0];
  switch (operation) {
    case "move-module": esm.file = "steps/registration.mjs"; break;
    case "extract-helper": model.helperFile = "support/workspace.mjs"; break;
    case "outline": model.scenario.outline = true; break;
    case "disable-ambiguity": model.config = { rules: { "ambiguous-step": "off" } }; break;
    case "restore-ambiguity":
      assert.ok(model.config, "restore-ambiguity requires disable-ambiguity");
      model.arguments.push("--rule", "ambiguous-step=warning");
      break;
    case "malformed-config":
      model.applicability = "invalid";
      model.arguments.push("--config", ".cuke-dedup.json");
      break;
    case "unknown-framework":
      model.provider = "unsupported-registration-provider";
      model.applicability = "unsupported";
      break;
    default: {
      const value = /^value=([\p{L}\p{N}]+(?: [\p{L}\p{N}]+)*)$/u.exec(operation)?.[1];
      assert.ok(value && Buffer.byteLength(value) <= 128, "unknown operation or unsupported literal value");
      model.scenario.value = value;
    }
  }
}

/** Calculate obligations from logical IDs and declared configuration before placing any source lines. */
function expectations(model) {
  if (model.applicability === "unsupported") return null;
  if (model.applicability === "invalid") return { refusal: "configuration", exit: 2 };
  const members = model.definitions.map(({ id }) => id);
  const groups = [{ rule: "duplicate-matcher", members, severity: "error" }];
  if (!model.config || model.arguments.includes("ambiguous-step=warning")) {
    groups.push({ rule: "ambiguous-step", step: "shared-step", members, severity: model.config ? "warning" : "error" });
  }
  return { groups, definitions: members, steps: ["shared-step"], matcher: model.scenario.value, exit: seed.expectedExit };
}

/** Emit both registrar forms and map the authored owners at their actual insertion points. */
function renderFiles(model) {
  const files = {};
  const locations = {};
  const { name, value, outline } = model.scenario;
  const feature = ["Feature: Mixed module syntax", `  Scenario${outline ? " Outline" : ""}: ${name}`];
  locations["shared-step"] = { path: "example.feature", line: feature.length + 1 };
  feature.push(`    Given ${outline ? "<registration>" : value}`);
  if (outline) feature.push("", "    Examples:", "      | registration |", `      | ${value} |`);
  files["example.feature"] = feature.join("\n") + "\n";
  for (const definition of model.definitions) {
    const { id, file, form, binding, helper } = definition;
    const provider = JSON.stringify(model.provider);
    const lines = [form === "named" ? `import { Given as ${binding} } from ${provider};` : `const cucumber = require(${provider});`];
    if (model.helperFile && form === "named") {
      const relative = posix.relative(posix.dirname(file), model.helperFile);
      const specifier = relative.startsWith(".") ? relative : `./${relative}`;
      lines.push(`import { ${helper} } from "${specifier}";`);
      files[model.helperFile] = `export function ${helper}() { return "ready"; }\n`;
    }
    lines.push("");
    locations[id] = { path: file, line: lines.length + 1 };
    lines.push(`${binding}(${JSON.stringify(value)}, async () => ${helper}());`);
    files[file] = lines.join("\n") + "\n";
  }
  if (model.applicability === "invalid") files[".cuke-dedup.json"] = "{\n";
  else if (model.config) files[".cuke-dedup.json"] = JSON.stringify(model.config) + "\n";
  return { files, locations };
}

/** Render a bounded, ordered recipe. Only the unchanged seed inherits its reviewed verdict. */
export function renderSeed(operations = []) {
  assert.ok(Array.isArray(operations) && operations.length <= 16, "at most 16 transformations");
  assert.equal(new Set(operations).size, operations.length, "duplicate transformation");
  const model = seedModel();
  for (const operation of operations) {
    assert.ok(typeof operation === "string" && operation.length <= 140, "invalid transformation");
    apply(model, operation);
  }
  const { name, origin, language, frameworks, verdict } = seed;
  return sealProject({
    seed: name, operations: [...operations],
    metadata: { origin, language, frameworks: model.applicability === "unsupported" ? ["unknown"] : [...frameworks], verdict: operations.length ? "generated" : verdict },
    applicability: model.applicability, arguments: model.arguments,
    expected: expectations(model), ...renderFiles(model),
  });
}
