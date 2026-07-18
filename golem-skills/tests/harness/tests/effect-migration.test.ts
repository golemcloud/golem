import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { describe, test } from "node:test";
import { promisify } from "node:util";
import {
  findMissingEffectBranches,
  loadManifest,
  reportFailure,
  selectNextUnit,
  validateManifestFiles,
  validateUnitStatic,
} from "../src/migrate-effect-skills.js";

const harnessRoot = process.cwd();
const repoRoot = path.resolve(harnessRoot, "../../..");
const manifestPath = path.join(repoRoot, "golem-skills", "effect-migration.yaml");
const execFileAsync = promisify(execFile);

describe("Effect migration manifest", () => {
  test("contains the complete inventory with valid references and scenarios", async () => {
    const manifest = await loadManifest(manifestPath);
    assert.equal(manifest.units.length, 38);
    await validateManifestFiles(repoRoot, manifest);
  });

  test("rejects a language-specific skill missing from the inventory", async () => {
    const manifest = await loadManifest(manifestPath);
    const addAgent = manifest.units.find(({ id }) => id === "golem-add-agent");
    assert.ok(addAgent);
    addAgent.references = ["ts", "rust", "scala"].map(
      (language) => `golem-skills/skills/${language}/golem-add-agent-${language}/SKILL.md`,
    );

    await assert.rejects(
      validateManifestFiles(repoRoot, manifest),
      /golem-add-agent-moonbit\/SKILL\.md/,
    );
  });

  test("rejects a language-specific scenario without a live verification owner", async () => {
    const manifest = await loadManifest(manifestPath);
    const httpEndpoint = manifest.units.find(({ id }) => id === "golem-add-http-endpoint");
    assert.ok(httpEndpoint);
    httpEndpoint.verifyScenarios = httpEndpoint.verifyScenarios.filter(
      (scenario) => scenario !== "configure-api-domain",
    );
    httpEndpoint.relatedScenarios.push("configure-api-domain");

    await assert.rejects(
      validateManifestFiles(repoRoot, manifest),
      /without a live verification owner:[\s\S]*configure-api-domain/,
    );
  });

  test("selects the first ready unit after the passed canary", async () => {
    const manifest = await loadManifest(manifestPath);
    assert.equal(selectNextUnit(manifest)?.id, "golem-add-npm-package");
  });

  test("does not select an active unit whose attempt budget is exhausted", async () => {
    const manifest = await loadManifest(manifestPath);
    const exhausted = manifest.units.find(({ id }) => id === "golem-add-npm-package");
    assert.ok(exhausted);
    exhausted.state = "editing";
    exhausted.attempts = Array.from({ length: manifest.policy.maxAttempts }, (_, index) => ({
      number: index + 1,
      startedAt: new Date(0).toISOString(),
      finishedAt: new Date(1).toISOString(),
      outcome: "failed" as const,
      reports: [],
      error: "verification failed",
    }));

    assert.equal(selectNextUnit(manifest)?.id, "golem-add-config");
  });

  test("the Phase 0 canary passes static validation", async () => {
    const manifest = await loadManifest(manifestPath);
    const canary = manifest.units.find(({ id }) => id === "golem-add-agent");
    assert.ok(canary);
    await validateUnitStatic(repoRoot, canary);
  });
});

describe("Effect migration verification", () => {
  test("finds nested language maps without an Effect branch", () => {
    assert.deepEqual(
      findMissingEffectBranches({
        prompt: { rust: "rust", ts: "typescript" },
        invoke: { method: { rust: "get_value", ts: "getValue", effect: "getValue" } },
      }),
      ["$root.prompt"],
    );
  });

  test("rejects a failed report even when its result has an empty error message", () => {
    const failure = reportFailure(
      {
        scenario: "failed-with-empty-error",
        matrix: { agent: "amp", language: "effect" },
        status: "fail",
        results: [{ success: false, error: "" }],
      },
      "golem-add-agent-effect",
    );

    assert.notEqual(failure, undefined);
    assert.notEqual(failure, "");
  });

  test("requires explicit target-skill activation evidence in passing reports", () => {
    const baseReport = {
      scenario: "canary",
      matrix: { agent: "amp", language: "effect" },
      status: "pass" as const,
      results: [
        {
          success: true,
          expectedSkills: ["golem-add-agent-effect"],
          activatedSkills: [] as string[],
        },
      ],
    };
    assert.match(
      reportFailure(baseReport, "golem-add-agent-effect") ?? "",
      /no activation evidence/,
    );

    baseReport.results[0].activatedSkills.push("golem-add-agent-effect");
    assert.equal(reportFailure(baseReport, "golem-add-agent-effect"), undefined);
  });

  test("rejects target-skill evidence declared only by a finalizer", async () => {
    const temporaryRepo = await fs.mkdtemp(path.join(os.tmpdir(), "effect-static-validation-"));
    const targetSkill = "golem-add-agent-effect";
    const scenarioName = "finalizer-only-evidence";
    try {
      const skillDir = path.join(temporaryRepo, "golem-skills", "skills", "effect", targetSkill);
      const scenarioDir = path.join(temporaryRepo, "golem-skills", "tests", "harness", "scenarios");
      await fs.mkdir(skillDir, { recursive: true });
      await fs.mkdir(scenarioDir, { recursive: true });
      await fs.writeFile(
        path.join(skillDir, "SKILL.md"),
        `---\nname: ${targetSkill}\ndescription: test skill\n---\n\n# Test\n`,
      );
      await fs.writeFile(
        path.join(scenarioDir, `${scenarioName}.yaml`),
        `name: ${scenarioName}\nsteps:\n  - prompt: implement the Effect agent\nfinally:\n  - prompt: inspect the Effect agent\n    expectedSkills:\n      effect:\n        - ${targetSkill}\n`,
      );

      const manifest = await loadManifest(manifestPath);
      const canary = manifest.units.find(({ id }) => id === "golem-add-agent");
      assert.ok(canary);
      await assert.rejects(
        validateUnitStatic(temporaryRepo, {
          ...canary,
          verifyScenarios: [scenarioName],
          relatedScenarios: [],
        }),
        /No mapped scenario expects/,
      );
    } finally {
      await fs.rm(temporaryRepo, { recursive: true, force: true });
    }
  });

  test("rejects removal of existing non-Effect scenario branches", async () => {
    const temporaryRepo = await fs.mkdtemp(path.join(os.tmpdir(), "effect-branch-preservation-"));
    const targetSkill = "golem-add-agent-effect";
    const scenarioName = "preserve-language-branches";
    try {
      const skillDir = path.join(temporaryRepo, "golem-skills", "skills", "effect", targetSkill);
      const scenarioDir = path.join(temporaryRepo, "golem-skills", "tests", "harness", "scenarios");
      const scenarioPath = path.join(scenarioDir, `${scenarioName}.yaml`);
      await fs.mkdir(skillDir, { recursive: true });
      await fs.mkdir(scenarioDir, { recursive: true });
      await fs.writeFile(
        path.join(skillDir, "SKILL.md"),
        `---\nname: ${targetSkill}\ndescription: test skill\n---\n\n# Test\n`,
      );
      await fs.writeFile(
        scenarioPath,
        `name: ${scenarioName}\nsteps:\n  - prompt:\n      ts: preserve ts\n      rust: preserve rust\n      scala: preserve scala\n      moonbit: preserve moonbit\n    expectedSkills:\n      ts: [golem-add-agent-ts]\n      rust: [golem-add-agent-rust]\n      scala: [golem-add-agent-scala]\n      moonbit: [golem-add-agent-moonbit]\n`,
      );
      await execFileAsync("git", ["init", "-q"], { cwd: temporaryRepo });
      await execFileAsync("git", ["add", "."], { cwd: temporaryRepo });
      await execFileAsync(
        "git",
        [
          "-c",
          "user.name=Bug Finder",
          "-c",
          "user.email=bug-finder@example.invalid",
          "commit",
          "-qm",
          "baseline",
        ],
        { cwd: temporaryRepo },
      );

      await fs.writeFile(
        scenarioPath,
        `name: ${scenarioName}\nsteps:\n  - prompt:\n      effect: implement effect\n    expectedSkills:\n      effect: [${targetSkill}]\n`,
      );

      const manifest = await loadManifest(manifestPath);
      const canary = manifest.units.find(({ id }) => id === "golem-add-agent");
      assert.ok(canary);
      await assert.rejects(
        validateUnitStatic(temporaryRepo, {
          ...canary,
          verifyScenarios: [scenarioName],
          relatedScenarios: [],
        }),
        /non-Effect|language branch|removed/i,
      );
    } finally {
      await fs.rm(temporaryRepo, { recursive: true, force: true });
    }
  });
});
