import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const binary = resolve(process.argv[2] || "target/release/cuke-dedup");
const repeats = Number.parseInt(process.env.CUKE_DEDUP_BENCH_REPEATS || "3", 10);
assert.ok(Number.isInteger(repeats) && repeats > 0, "CUKE_DEDUP_BENCH_REPEATS must be positive");
const timeoutMs = Number.parseInt(process.env.CUKE_DEDUP_BENCH_TIMEOUT_MS || "60000", 10);
assert.ok(
  Number.isInteger(timeoutMs) && timeoutMs > 0,
  "CUKE_DEDUP_BENCH_TIMEOUT_MS must be positive",
);
const smoke = process.env.CUKE_DEDUP_BENCH_SMOKE === "1";
const sizeFilter = process.env.CUKE_DEDUP_BENCH_SIZE === undefined
  ? null
  : Number.parseInt(process.env.CUKE_DEDUP_BENCH_SIZE, 10);
assert.ok(
  sizeFilter === null || (Number.isInteger(sizeFilter) && sizeFilter > 0),
  "CUKE_DEDUP_BENCH_SIZE must be positive",
);

const allProfiles = smoke
  ? [
      { name: "unrelated", sizes: [10], sharedStructure: false },
      { name: "shared-structure", sizes: [10], sharedStructure: true },
      { name: "homogeneous-handler", sizes: [20], sharedHandler: true },
      {
        name: "varied-handler",
        sizes: [50],
        variedHandlers: true,
        requiresCompleteCensus: true,
      },
      { name: "repository-scale", sizes: [20], repositoryShape: true },
      { name: "usage-scale", sizes: [50], usageSteps: 100 },
      { name: "candidate-limit", sizes: [20], sharedStructure: true, candidateLimit: 5 },
    ]
  : [
      { name: "unrelated", sizes: [250, 500, 1000, 2000], sharedStructure: false },
      { name: "shared-structure", sizes: [250, 500], sharedStructure: true },
      { name: "homogeneous-handler", sizes: [1000, 2000], sharedHandler: true },
      {
        name: "varied-handler",
        sizes: [500, 1000, 2000],
        variedHandlers: true,
        requiresCompleteCensus: true,
      },
      { name: "repository-scale", sizes: [2000], repositoryShape: true },
      { name: "usage-scale", sizes: [2000], usageSteps: 2000 },
      { name: "candidate-limit", sizes: [10000], sharedStructure: true, candidateLimit: 1000 },
    ];
const profileFilter = process.env.CUKE_DEDUP_BENCH_PROFILE;
const selectedProfiles = profileFilter
  ? allProfiles.filter((profile) => profile.name === profileFilter)
  : allProfiles;
const profiles = selectedProfiles
  .map((profile) => ({
    ...profile,
    sizes: sizeFilter === null ? profile.sizes : profile.sizes.filter((size) => size === sizeFilter),
  }))
  .filter((profile) => profile.sizes.length > 0);
assert.ok(
  profiles.length > 0,
  `Unknown benchmark profile or size: ${profileFilter || "all"}/${sizeFilter || "all"}`,
);
const temporary = await mkdtemp(join(tmpdir(), "cuke-dedup-benchmark-"));
const results = [];

try {
  for (const profile of profiles) {
    for (const definitions of profile.sizes) {
      const root = join(temporary, `${profile.name}-${definitions}`);
      const output = join(temporary, "reports", `${profile.name}-${definitions}`);
      await mkdir(root, { recursive: true });
      const generated = profile.repositoryShape
        ? await writeRepositoryScale(root, definitions)
        : profile.usageSteps
          ? await writeUsageScale(root, definitions, profile.usageSteps)
          : profile.variedHandlers
            ? await writeVariedHandlerScale(root, definitions)
            : await writeSingleFileProfile(root, definitions, profile);

      run(root, output, generated.exitCode); // Warm filesystem and process-launch paths before recording.
      const samples = [];
      for (let attempt = 0; attempt < repeats; attempt += 1) {
        samples.push(run(root, output, generated.exitCode));
      }
      const reference = samples[0];
      assert.equal(reference.definitionsAnalyzed, generated.definitions);
      assert.equal(reference.featureStepsAnalyzed, generated.featureSteps);
      assert.equal(reference.definitionFiles, generated.definitionFiles);
      assert.equal(reference.featureFiles, generated.featureFiles);
      if (generated.findingCount !== null) {
        assert.equal(reference.findingCount, generated.findingCount);
      }
      assert.equal(reference.analysisTruncated, generated.analysisTruncated);
      if (profile.requiresCompleteCensus) {
        assert.equal(
          reference.analysisTruncated,
          false,
          `${profile.name}-${definitions}: analysis must complete without truncation`,
        );
        assert.equal(
          reference.skippedCandidateComparisons,
          0,
          `${profile.name}-${definitions}: normal-scale candidates must not be skipped`,
        );
        assert.equal(
          reference.truncatedStructuralClasses,
          0,
          `${profile.name}-${definitions}: structural classes must be complete`,
        );
        assert.ok(
          reference.candidateComparisonsEvaluated > 0,
          `${profile.name}-${definitions}: profile must exercise candidate comparison work`,
        );
      }
      for (const sample of samples.slice(1)) {
        assert.equal(sample.definitionsAnalyzed, reference.definitionsAnalyzed);
        assert.equal(sample.featureStepsAnalyzed, reference.featureStepsAnalyzed);
        assert.equal(sample.definitionFiles, reference.definitionFiles);
        assert.equal(sample.featureFiles, reference.featureFiles);
        assert.equal(sample.fileCount, reference.fileCount);
        assert.equal(sample.findingCount, reference.findingCount);
        assert.equal(sample.analysisTruncated, reference.analysisTruncated);
        assert.equal(sample.candidateComparisonsEvaluated, reference.candidateComparisonsEvaluated);
        assert.equal(sample.skippedCandidateComparisons, reference.skippedCandidateComparisons);
        assert.equal(sample.truncatedStructuralClasses, reference.truncatedStructuralClasses);
        assert.deepEqual(sample.candidateSources, reference.candidateSources);
      }
      const wallSamples = samples.map((sample) => sample.wallMs);
      results.push({
        profile: profile.name,
        packageCount: generated.packages,
        definitions,
        featureSteps: reference.featureStepsAnalyzed,
        candidatePairs: generated.candidatePairs ?? reference.candidateComparisonsEvaluated,
        definitionFiles: reference.definitionFiles,
        featureFiles: reference.featureFiles,
        fileCount: reference.fileCount,
        findingCount: reference.findingCount,
        analysisComplete: !reference.analysisTruncated,
        analysisTruncated: reference.analysisTruncated,
        candidateComparisonsEvaluated: reference.candidateComparisonsEvaluated,
        skippedCandidateComparisons: reference.skippedCandidateComparisons,
        truncatedStructuralClasses: reference.truncatedStructuralClasses,
        candidateSources: reference.candidateSources,
        medianMs: median(wallSamples),
        discoveryMedianMs: median(samples.map((sample) => sample.discoveryMs)),
        parsingMedianMs: median(samples.map((sample) => sample.parsingMs)),
        analysisMedianMs: median(samples.map((sample) => sample.analysisMs)),
        peakMemoryBytes: medianNullable(samples.map((sample) => sample.peakMemoryBytes)),
        samplesMs: wallSamples.map(round),
        samples: samples.map((sample) => ({
          wallMs: round(sample.wallMs),
          discoveryMs: round(sample.discoveryMs),
          parsingMs: round(sample.parsingMs),
          analysisMs: round(sample.analysisMs),
          peakMemoryBytes: sample.peakMemoryBytes,
        })),
      });
    }
  }
  console.log(
    JSON.stringify(
      {
        schemaVersion: 3,
        binary,
        repeats,
        timeoutMs,
        smoke,
        profileFilter: profileFilter || null,
        sizeFilter,
        results,
      },
      null,
      2,
    ),
  );
} finally {
  await rm(temporary, { recursive: true, force: true });
}

function run(root, output, expectedExitCode) {
  const binaryArguments = [".", "--reporters", "json", "--output", output];
  const memoryFile = join(root, ".cuke-dedup-benchmark-memory");
  let command = binary;
  let commandArguments = binaryArguments;
  let memoryFormat = null;
  if (existsSync("/usr/bin/time") && process.platform === "linux") {
    command = "/usr/bin/time";
    commandArguments = ["-f", "%M", "-o", memoryFile, binary, ...binaryArguments];
    memoryFormat = "linux-kib";
  } else if (existsSync("/usr/bin/time") && process.platform === "darwin") {
    command = "/usr/bin/time";
    commandArguments = ["-l", binary, ...binaryArguments];
    memoryFormat = "darwin-bytes";
  }

  const started = process.hrtime.bigint();
  const result = spawnSync(command, commandArguments, {
    cwd: root,
    encoding: "utf8",
    timeout: timeoutMs,
    maxBuffer: 1024 * 1024,
  });
  const wallMs = Number(process.hrtime.bigint() - started) / 1_000_000;
  assert.equal(
    result.status,
    expectedExitCode,
    result.error?.message || result.stderr || result.stdout,
  );
  const report = JSON.parse(readFileSync(join(output, "cuke-dedup.json"), "utf8"));
  assert.ok(report.metrics, "JSON report is missing execution metrics");
  const peakMemoryBytes = readPeakMemory(memoryFormat, memoryFile, result.stderr);
  return {
    wallMs,
    peakMemoryBytes,
    definitionFiles: report.metrics.definitionFiles,
    featureFiles: report.metrics.featureFiles,
    fileCount: report.metrics.filesDiscovered,
    findingCount: report.summary.findings,
    definitionsAnalyzed: report.summary.definitionsAnalyzed,
    featureStepsAnalyzed: report.summary.featureStepsAnalyzed,
    discoveryMs: report.metrics.discoveryMs,
    parsingMs: report.metrics.parsingMs,
    analysisMs: report.metrics.analysisMs,
    analysisTruncated: report.analysis.truncated,
    candidateComparisonsEvaluated: report.analysis.candidateComparisonsEvaluated,
    skippedCandidateComparisons: report.analysis.skippedCandidateComparisons,
    truncatedStructuralClasses: report.analysis.truncatedStructuralClasses,
    candidateSources: report.analysis.candidateSources,
  };
}

function readPeakMemory(format, memoryFile, stderr) {
  if (format === "linux-kib") {
    const kibibytes = Number.parseInt(readFileSync(memoryFile, "utf8").trim(), 10);
    assert.ok(Number.isFinite(kibibytes), "GNU time did not report peak memory");
    return kibibytes * 1024;
  }
  if (format === "darwin-bytes") {
    const match = stderr.match(/^\s*(\d+)\s+maximum resident set size\s*$/m);
    assert.ok(match, "BSD time did not report peak memory");
    return Number.parseInt(match[1], 10);
  }
  return null;
}

function median(values) {
  const sorted = [...values].sort((left, right) => left - right);
  return round(sorted[Math.floor(sorted.length / 2)]);
}

function medianNullable(values) {
  const measured = values.filter((value) => value !== null);
  return measured.length === 0 ? null : median(measured);
}

function round(value) {
  return Number(value.toFixed(2));
}

function source(count, sharedStructure, sharedHandler) {
  const lines = [];
  for (let index = 0; index < count; index += 1) {
    const handler = sharedHandler
      ? "() => sharedImplementation()"
      : sharedStructure
        ? `() => perform(${index})`
        : `() => action_${index}()`;
    lines.push(`Given('benchmark step ${index}', ${handler});`);
  }
  return `${lines.join("\n")}\n`;
}

async function writeSingleFileProfile(root, definitions, profile) {
  const { sharedStructure, sharedHandler, candidateLimit } = profile;
  await writeFile(
    join(root, "steps.ts"),
    source(definitions, sharedStructure, sharedHandler),
  );
  await writeFile(join(root, "suite.feature"), "Feature: Benchmark\n  Scenario: Corpus\n");
  if (sharedHandler || candidateLimit) {
    const rules = {};
    if (sharedHandler) rules["near-duplicate-step"] = "off";
    if (candidateLimit) rules["unused-definition"] = "off";
    await writeFile(
      join(root, ".cuke-dedup.json"),
      `${JSON.stringify({
        threshold: 100,
        ...(candidateLimit ? { maxStructuralClassComparisons: candidateLimit } : {}),
        rules,
      }, null, 2)}\n`,
    );
  }
  return {
    packages: 1,
    definitions,
    featureSteps: 0,
    definitionFiles: 1,
    featureFiles: 1,
    candidatePairs: sharedHandler
      ? definitions - 1
      : sharedStructure
        ? (definitions * (definitions - 1)) / 2
        : 0,
    findingCount: candidateLimit
      ? candidateLimit
      : definitions + (sharedStructure || sharedHandler ? definitions - 1 : 0),
    exitCode: candidateLimit ? 2 : 0,
    analysisTruncated: Boolean(candidateLimit),
  };
}

async function writeVariedHandlerScale(root, definitions) {
  const actors = ["customer", "administrator", "operator", "reviewer", "member"];
  const resources = [
    "account",
    "order",
    "invoice",
    "profile",
    "shipment",
    "subscription",
    "payment",
    "workspace",
  ];
  const states = ["ready", "approved", "archived", "visible", "synchronized"];
  const channels = ["dashboard", "api", "mobile", "batch"];
  const registrations = ["Given", "When", "Then"];
  const lines = [];

  for (let index = 0; index < definitions; index += 1) {
    const actor = actors[index % actors.length];
    const resource = resources[Math.floor(index / actors.length) % resources.length];
    const state = states[
      Math.floor(index / (actors.length * resources.length)) % states.length
    ];
    const channel = channels[
      Math.floor(index / (actors.length * resources.length * states.length)) % channels.length
    ];
    const handlerVariant = Math.floor(index / (actors.length * resources.length)) % 4;
    const workflow = `${actor}${capitalize(resource)}`;
    const matcher = variedMatcher(index, actor, resource, state, channel);
    const handler = variedHandler(index, resource, state, workflow, handlerVariant);
    lines.push(
      `${registrations[index % registrations.length]}(${JSON.stringify(matcher)}, ${handler});`,
    );
  }

  await writeFile(join(root, "steps.ts"), `${lines.join("\n")}\n`);
  await writeFile(
    join(root, "suite.feature"),
    "Feature: Varied handler benchmark\n  Scenario: Census only\n",
  );
  await writeFile(
    join(root, ".cuke-dedup.json"),
    `${JSON.stringify({ threshold: 100, rules: { "unused-definition": "off" } }, null, 2)}\n`,
  );

  return {
    packages: 1,
    definitions,
    featureSteps: 0,
    definitionFiles: 1,
    featureFiles: 1,
    findingCount: null,
    exitCode: 0,
    analysisTruncated: false,
  };
}

function variedMatcher(index, actor, resource, state, channel) {
  switch (index % 4) {
    case 0:
      return `the ${actor} ${resource} workflow ${index} becomes ${state} through ${channel}`;
    case 1:
      return `${actor} completes ${channel} workflow ${index} for the ${state} ${resource}`;
    case 2:
      return `the ${state} ${resource} workflow ${index} is processed for ${actor} by ${channel}`;
    default:
      return `during the ${channel} workflow ${actor} marks ${resource} ${index} as ${state}`;
  }
}

function variedHandler(index, resource, state, workflow, variant) {
  switch (variant) {
    case 0:
      return `async ({ world }) => { const record = await world.${resource}.load(${index}); await ${workflow}Workflow.verify(record); }`;
    case 1:
      return `async function (context) { const record = await context.${resource}.findById(${index}); return ${workflow}Policy.assertState(record); }`;
    case 2:
      return `({ services }) => services.${resource}.transition(${index}, ${JSON.stringify(state)})`;
    default:
      return `async ({ api }) => { await api.${resource}.update(${index}, { state: ${JSON.stringify(state)} }); await ${workflow}Audit.record(${index}); }`;
  }
}

function capitalize(value) {
  return `${value[0].toUpperCase()}${value.slice(1)}`;
}

async function writeRepositoryScale(root, definitions) {
  const layout = definitions === 20
    ? { packages: 2, definitionFiles: 2, featureFiles: 2, stepsPerFeature: 5 }
    : { packages: 20, definitionFiles: 20, featureFiles: 10, stepsPerFeature: 25 };
  const definitionsPerFile = 5;
  assert.equal(
    layout.packages * layout.definitionFiles * definitionsPerFile,
    definitions,
    "repository-scale definition layout drifted",
  );
  await writeFile(join(root, ".cuke-dedup.json"), `${JSON.stringify({ threshold: 100 }, null, 2)}\n`);

  const extensions = ["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx"];
  const definitionsPerPackage = layout.definitionFiles * definitionsPerFile;
  const writes = [];
  for (let packageIndex = 0; packageIndex < layout.packages; packageIndex += 1) {
    const packageRoot = join(root, "packages", `package-${packageIndex}`);
    const definitionRoot = join(packageRoot, "test", "steps");
    const featureRoot = join(packageRoot, "features");
    await mkdir(definitionRoot, { recursive: true });
    await mkdir(featureRoot, { recursive: true });

    for (let fileIndex = 0; fileIndex < layout.definitionFiles; fileIndex += 1) {
      const lines = [];
      for (let offset = 0; offset < definitionsPerFile; offset += 1) {
        const localIndex = fileIndex * definitionsPerFile + offset;
        const globalIndex = packageIndex * definitionsPerPackage + localIndex;
        const matcher = localIndex === 0
          ? "the shared cross package maintenance runs"
          : `package ${packageIndex} operation ${localIndex} runs`;
        lines.push(`Given('${matcher}', () => action_${globalIndex}());`);
      }
      const extension = extensions[(packageIndex + fileIndex) % extensions.length];
      writes.push(writeFile(join(definitionRoot, `steps-${fileIndex}.${extension}`), `${lines.join("\n")}\n`));
    }

    for (let fileIndex = 0; fileIndex < layout.featureFiles; fileIndex += 1) {
      const lines = [
        `Feature: Package ${packageIndex} feature ${fileIndex}`,
        `  Scenario: Package ${packageIndex} scenario ${fileIndex}`,
      ];
      for (let stepIndex = 0; stepIndex < layout.stepsPerFeature; stepIndex += 1) {
        const localIndex = 1 + ((fileIndex * layout.stepsPerFeature + stepIndex) % (definitionsPerPackage - 1));
        lines.push(`    Given package ${packageIndex} operation ${localIndex} runs`);
      }
      writes.push(writeFile(join(featureRoot, `suite-${fileIndex}.feature`), `${lines.join("\n")}\n`));
    }
  }
  await Promise.all(writes);

  const featureFiles = layout.packages * layout.featureFiles;
  return {
    packages: layout.packages,
    definitions,
    featureSteps: featureFiles * layout.stepsPerFeature,
    definitionFiles: layout.packages * layout.definitionFiles,
    featureFiles,
    candidatePairs: (layout.packages * (layout.packages - 1)) / 2,
    findingCount: layout.packages + (layout.packages - 1),
    exitCode: 0,
    analysisTruncated: false,
  };
}

async function writeUsageScale(root, definitions, featureSteps) {
  assert.ok(featureSteps >= definitions, "usage-scale must exercise every definition");
  await writeFile(
    join(root, "steps.ts"),
    Array.from(
      { length: definitions },
      (_, index) => `Given('usage step ${index}', () => action_${index}());`,
    ).join("\n") + "\n",
  );
  const featureLines = ["Feature: Production-scale usage", "  Scenario: Exercise definitions"];
  for (let index = 0; index < featureSteps; index += 1) {
    featureLines.push(`    Given usage step ${index % definitions}`);
  }
  await writeFile(join(root, "usage.feature"), `${featureLines.join("\n")}\n`);
  await writeFile(
    join(root, ".cuke-dedup.json"),
    `${JSON.stringify({ threshold: 100 }, null, 2)}\n`,
  );
  return {
    packages: 1,
    definitions,
    featureSteps,
    definitionFiles: 1,
    featureFiles: 1,
    candidatePairs: 0,
    findingCount: 0,
    exitCode: 0,
    analysisTruncated: false,
  };
}
