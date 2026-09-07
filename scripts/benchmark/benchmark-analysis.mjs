import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const binary = resolve(process.argv[2] || "target/release/cuke-dedup");
const repeats = Number.parseInt(process.env.CUKE_DEDUP_BENCH_REPEATS || "3", 10);
assert.ok(Number.isInteger(repeats) && repeats > 0, "CUKE_DEDUP_BENCH_REPEATS must be positive");
const smoke = process.env.CUKE_DEDUP_BENCH_SMOKE === "1";

const profiles = smoke
  ? [
      { name: "unrelated", sizes: [10], sharedStructure: false },
      { name: "shared-structure", sizes: [10], sharedStructure: true },
      { name: "homogeneous-handler", sizes: [20], sharedHandler: true },
      { name: "repository-scale", sizes: [20], repositoryShape: true },
    ]
  : [
      { name: "unrelated", sizes: [250, 500, 1000, 2000], sharedStructure: false },
      { name: "shared-structure", sizes: [250, 500], sharedStructure: true },
      { name: "homogeneous-handler", sizes: [1000, 2000], sharedHandler: true },
      { name: "repository-scale", sizes: [2000], repositoryShape: true },
    ];
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
        : await writeSingleFileProfile(
            root,
            definitions,
            profile.sharedStructure,
            profile.sharedHandler,
          );

      run(root, output); // Warm filesystem and process-launch paths before recording.
      const samples = [];
      for (let attempt = 0; attempt < repeats; attempt += 1) {
        samples.push(run(root, output));
      }
      const reference = samples[0];
      assert.equal(reference.definitionsAnalyzed, generated.definitions);
      assert.equal(reference.featureStepsAnalyzed, generated.featureSteps);
      assert.equal(reference.definitionFiles, generated.definitionFiles);
      assert.equal(reference.featureFiles, generated.featureFiles);
      assert.equal(reference.findingCount, generated.findingCount);
      for (const sample of samples.slice(1)) {
        assert.equal(sample.definitionsAnalyzed, reference.definitionsAnalyzed);
        assert.equal(sample.featureStepsAnalyzed, reference.featureStepsAnalyzed);
        assert.equal(sample.definitionFiles, reference.definitionFiles);
        assert.equal(sample.featureFiles, reference.featureFiles);
        assert.equal(sample.fileCount, reference.fileCount);
        assert.equal(sample.findingCount, reference.findingCount);
      }
      const wallSamples = samples.map((sample) => sample.wallMs);
      results.push({
        profile: profile.name,
        packageCount: generated.packages,
        definitions,
        featureSteps: reference.featureStepsAnalyzed,
        candidatePairs: generated.candidatePairs,
        definitionFiles: reference.definitionFiles,
        featureFiles: reference.featureFiles,
        fileCount: reference.fileCount,
        findingCount: reference.findingCount,
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
  console.log(JSON.stringify({ schemaVersion: 2, binary, repeats, smoke, results }, null, 2));
} finally {
  await rm(temporary, { recursive: true, force: true });
}

function run(root, output) {
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
  });
  const wallMs = Number(process.hrtime.bigint() - started) / 1_000_000;
  assert.equal(result.status, 0, result.stderr || result.stdout);
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

async function writeSingleFileProfile(root, definitions, sharedStructure, sharedHandler) {
  await writeFile(
    join(root, "steps.ts"),
    source(definitions, sharedStructure, sharedHandler),
  );
  await writeFile(join(root, "suite.feature"), "Feature: Benchmark\n  Scenario: Corpus\n");
  if (sharedHandler) {
    await writeFile(
      join(root, ".cuke-dedup.json"),
      `${JSON.stringify({ threshold: 100, rules: { "near-duplicate-step": "off" } }, null, 2)}\n`,
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
    findingCount: definitions + (sharedStructure || sharedHandler ? definitions - 1 : 0),
  };
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
  };
}
