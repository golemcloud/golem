import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { describe, test } from "node:test";
import { promisify } from "node:util";
import {
  ampWorkerArgs,
  attemptLimitFor,
  buildWorkerPrompt,
  buildPreviousFailureSection,
  findMissingEffectBranches,
  loadManifest,
  prerequisitesAreComplete,
  reportFailure,
  recordSuccessfulVerification,
  selectNextUnit,
  semanticRemediationFailure,
  validateManifestFiles,
  validateSemanticRemediation,
  validateUnitStatic,
} from "../src/migrate-effect-skills.js";

const harnessRoot = process.cwd();
const repoRoot = path.resolve(harnessRoot, "../../..");
const manifestPath = path.join(repoRoot, "golem-skills", "effect-migration.yaml");
const execFileAsync = promisify(execFile);

function resetMigrationProgress(manifest: Awaited<ReturnType<typeof loadManifest>>): void {
  for (const unit of manifest.units) {
    unit.state = unit.id === "golem-add-agent" ? "passed" : "pending";
    unit.semanticStatus = unit.id === "golem-add-agent" ? "passed" : "pending";
    unit.attempts = [];
    unit.evidence = null;
  }
}

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
    resetMigrationProgress(manifest);
    assert.equal(selectNextUnit(manifest)?.id, "golem-add-npm-package");
  });

  test("selects an execution-passed unit that still has a workaround", async () => {
    const manifest = await loadManifest(manifestPath);
    for (const unit of manifest.units) {
      unit.state = "passed";
      unit.semanticStatus = "passed";
    }
    const workaround = manifest.units.find(({ id }) => id === "golem-make-http-request");
    assert.ok(workaround);
    workaround.semanticStatus = "passed-with-workaround";
    assert.equal(selectNextUnit(manifest)?.id, workaround.id);
  });

  test("gives workaround remediation one bounded budget and does not reselect it when blocked", async () => {
    const manifest = await loadManifest(manifestPath);
    for (const unit of manifest.units) {
      unit.state = "passed";
      unit.semanticStatus = "passed";
    }
    const workaround = manifest.units.find(({ id }) => id === "golem-make-http-request");
    assert.ok(workaround);
    workaround.semanticStatus = "passed-with-workaround";
    const originalAttempts = workaround.attempts.length;
    const attemptLimit = attemptLimitFor(workaround, manifest.policy.maxAttempts);
    assert.equal(attemptLimit, originalAttempts + manifest.policy.maxAttempts);

    workaround.state = "blocked";
    workaround.attempts.push(
      ...Array.from({ length: manifest.policy.maxAttempts }, (_, index) => ({
        number: originalAttempts + index + 1,
        startedAt: new Date(0).toISOString(),
        finishedAt: new Date(1).toISOString(),
        outcome: "blocked" as const,
        reports: [],
        error: "semantic verification failed",
      })),
    );
    assert.equal(attemptLimitFor(workaround, manifest.policy.maxAttempts), attemptLimit);
    assert.equal(selectNextUnit(manifest), undefined);
  });

  test("requires both execution and semantic prerequisite completion", async () => {
    const manifest = await loadManifest(manifestPath);
    resetMigrationProgress(manifest);
    const canary = manifest.units.find(({ id }) => id === "golem-add-agent");
    const config = manifest.units.find(({ id }) => id === "golem-add-config");
    assert.ok(canary);
    assert.ok(config);
    canary.semanticStatus = "passed-with-workaround";
    assert.equal(prerequisitesAreComplete(manifest, config), false);
    canary.semanticStatus = "passed";
    assert.equal(prerequisitesAreComplete(manifest, config), true);
  });

  test("successful remediation updates semantics without discarding history", async () => {
    const manifest = await loadManifest(manifestPath);
    const unit = manifest.units.find(({ id }) => id === "golem-make-http-request");
    assert.ok(unit);
    const priorAttempts = unit.attempts.length;
    const attempt = {
      number: priorAttempts + 1,
      startedAt: new Date(0).toISOString(),
      outcome: "editing" as const,
      reports: ["new-report.json"],
    };
    unit.attempts.push(attempt);
    recordSuccessfulVerification(manifest, unit, attempt);
    assert.equal(unit.state, "passed");
    assert.equal(unit.semanticStatus, "passed");
    assert.equal(unit.attempts.length, priorAttempts + 1);
    assert.deepEqual(unit.evidence?.reports, ["new-report.json"]);
  });

  test("does not select an active unit whose attempt budget is exhausted", async () => {
    const manifest = await loadManifest(manifestPath);
    resetMigrationProgress(manifest);
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

  test("rejects weakening of protected scenario semantic requirements", async () => {
    const temporaryRepo = await fs.mkdtemp(path.join(os.tmpdir(), "effect-semantic-validation-"));
    try {
      const manifest = await loadManifest(manifestPath);
      const unit = manifest.units.find(({ id }) => id === "golem-add-llm");
      assert.ok(unit);
      const skillDir = path.join(
        temporaryRepo,
        "golem-skills",
        "skills",
        "effect",
        unit.targetSkill,
      );
      const scenarioDir = path.join(temporaryRepo, "golem-skills", "tests", "harness", "scenarios");
      await fs.mkdir(skillDir, { recursive: true });
      await fs.mkdir(scenarioDir, { recursive: true });
      await fs.writeFile(
        path.join(skillDir, "SKILL.md"),
        `---\nname: ${unit.targetSkill}\ndescription: test skill\n---\n`,
      );
      await fs.writeFile(
        path.join(scenarioDir, "add-llm.yaml"),
        `name: add-llm\nsemanticRequirements:\n  effect:\n    - Calling any service is sufficient.\nsteps:\n  - prompt: implement it\n    expectedSkills:\n      effect: [${unit.targetSkill}]\n`,
      );
      await assert.rejects(validateUnitStatic(temporaryRepo, unit), /semanticRequirements/);
    } finally {
      await fs.rm(temporaryRepo, { recursive: true, force: true });
    }
  });

  test("allows promotion after a scenario removes its workaround", async () => {
    const manifest = await loadManifest(manifestPath);
    const unit = manifest.units.find(({ id }) => id === "golem-make-http-request");
    assert.ok(unit);
    await validateSemanticRemediation(repoRoot, unit);
  });

  test("requires the restored atomic Effect assertion to match existing behavior", () => {
    const document = {
      steps: [
        {
          id: "verify-event-sequence",
          expect: {
            ts: { body_json: [{ path: "$", equals: ["a", "b", "a", "b", "c", "d"] }] },
            effect: { body_json: [{ path: "$", equals: ["a", "b", "c", "d"] }] },
          },
        },
      ],
    };
    assert.match(semanticRemediationFailure("atomic-block", document) ?? "", /same event sequence/);
  });
});

describe("Effect migration verification", () => {
  test("includes first-class scenario semantic requirements in the worker prompt", async () => {
    const manifest = await loadManifest(manifestPath);
    const unit = manifest.units.find(({ id }) => id === "golem-add-llm");
    assert.ok(unit);
    const prompt = await buildWorkerPrompt(repoRoot, manifest, unit);
    assert.match(prompt, /acceptance criteria and may not be weakened/);
    assert.match(prompt, /Effect AI LanguageModel/);
    assert.match(prompt, /direct fetch and stubbed responses are not substitutes/);
  });

  test("continues the migration thread with live verification evidence", () => {
    const thread = "https://ampcode.com/threads/T-00000000-0000-0000-0000-000000000001";
    const args = ampWorkerArgs("high", "repair the failed scenario", thread);
    assert.deepEqual(args.slice(0, 3), ["threads", "continue", thread]);
    assert.ok(args.includes("--execute"));
    assert.ok(args.includes("--stream-json"));

    const feedback = buildPreviousFailureSection(
      "atomic-block: INVOKE_FAILED",
      ["results/effect-migration/atomic-block/report.json"],
      ["workspaces/run-id/atomic-block/effect"],
    );
    assert.match(feedback, /Previous live verification failed — repair it/);
    assert.match(feedback, /results\/effect-migration\/atomic-block\/report\.json/);
    assert.match(feedback, /workspaces\/run-id\/atomic-block\/effect/);
    assert.match(feedback, /Fix the assigned Effect skill and\/or its[\s\S]*listed scenarios/);
    assert.match(feedback, /atomic-block: INVOKE_FAILED/);
  });

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
