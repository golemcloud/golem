import { describe, it } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { ScenarioExecutor } from "../src/executor.js";

type StepLike = {
  expectedSkills?: string[];
  strictSkillMatch?: boolean;
  allowedExtraSkills?: string[];
};
type ExecutorWithPrivate = {
  assertSkillActivation(
    step: StepLike,
    activated: string[],
  ): string | undefined;
  workspace: string;
  findGolemProjectDir(): Promise<string>;
  runLocalCommand(
    command: string,
    args: string[],
    timeoutSeconds: number,
    cwd: string,
    extraEnv?: Record<string, string>,
  ): Promise<{ success: boolean; output: string }>;
  executeVerification(
    stepLabel: string,
    verify: { build?: boolean; deploy?: boolean; expectedFiles?: string[] },
    commandEnv: Record<string, string>,
    currentSuccess: boolean,
    fail: (msg: string) => void,
  ): Promise<void>;
};

function assertSkillActivation(
  step: StepLike,
  activatedSkills: string[],
): string | undefined {
  const executor = Object.create(
    ScenarioExecutor.prototype,
  ) as unknown as ExecutorWithPrivate;
  return executor.assertSkillActivation(step, activatedSkills);
}

describe("assertSkillActivation", () => {
  it("returns undefined when no expected skills", () => {
    const result = assertSkillActivation({}, ["some-skill"]);
    assert.equal(result, undefined);
  });

  it("returns undefined when all expected skills are activated", () => {
    const result = assertSkillActivation(
      { expectedSkills: ["skill-a", "skill-b"] },
      ["skill-a", "skill-b"],
    );
    assert.equal(result, undefined);
  });

  it("returns error when expected skill is missing", () => {
    const result = assertSkillActivation(
      { expectedSkills: ["skill-a", "skill-b"] },
      ["skill-a"],
    );
    assert.ok(result);
    assert.ok(result.includes("SKILL_NOT_ACTIVATED"));
    assert.ok(result.includes("skill-b"));
  });

  it("allows extra skills by default when allowedExtraSkills not set", () => {
    const result = assertSkillActivation({ expectedSkills: ["skill-a"] }, [
      "skill-a",
      "skill-extra",
    ]);
    assert.ok(result);
    assert.ok(result.includes("SKILL_MISMATCH"));
  });

  it("allows specified extra skills", () => {
    const result = assertSkillActivation(
      { expectedSkills: ["skill-a"], allowedExtraSkills: ["skill-extra"] },
      ["skill-a", "skill-extra"],
    );
    assert.equal(result, undefined);
  });

  it("returns error for strict match with extras", () => {
    const result = assertSkillActivation(
      { expectedSkills: ["skill-a"], strictSkillMatch: true },
      ["skill-a", "skill-extra"],
    );
    assert.ok(result);
    assert.ok(result.includes("SKILL_MISMATCH"));
    assert.ok(result.includes("strict"));
  });

  it("passes strict match when exact", () => {
    const result = assertSkillActivation(
      { expectedSkills: ["skill-a"], strictSkillMatch: true },
      ["skill-a"],
    );
    assert.equal(result, undefined);
  });
});

describe("executeVerification", () => {
  it("does not attempt deploy when expected files are missing", async () => {
    const workspace = await fs.mkdtemp(path.join(os.tmpdir(), "verify-files-"));
    try {
      const executor = Object.create(
        ScenarioExecutor.prototype,
      ) as unknown as ExecutorWithPrivate;
      const calls: string[][] = [];
      const failures: string[] = [];

      executor.workspace = workspace;
      executor.findGolemProjectDir = async () => workspace;
      executor.runLocalCommand = async (_command, args) => {
        calls.push(args);
        return { success: true, output: "ok" };
      };

      await executor.executeVerification(
        "verify",
        { expectedFiles: ["missing.txt"], deploy: true },
        {},
        true,
        (msg) => failures.push(msg),
      );

      assert.deepEqual(calls, []);
      assert.equal(failures.length, 1);
      assert.ok(failures[0].includes("EXPECTED_FILE_MISSING"));
    } finally {
      await fs.rm(workspace, { recursive: true, force: true });
    }
  });

  it("does not attempt deploy after a build failure", async () => {
    const workspace = await fs.mkdtemp(path.join(os.tmpdir(), "verify-build-"));
    try {
      const executor = Object.create(
        ScenarioExecutor.prototype,
      ) as unknown as ExecutorWithPrivate;
      const calls: string[][] = [];
      const failures: string[] = [];

      executor.workspace = workspace;
      executor.findGolemProjectDir = async () => workspace;
      executor.runLocalCommand = async (_command, args) => {
        calls.push(args);
        return { success: false, output: "build failed" };
      };

      await executor.executeVerification(
        "verify",
        { build: true, deploy: true },
        {},
        true,
        (msg) => failures.push(msg),
      );

      assert.deepEqual(calls, [["build"]]);
      assert.equal(failures.length, 1);
      assert.ok(failures[0].includes("BUILD_FAILED"));
    } finally {
      await fs.rm(workspace, { recursive: true, force: true });
    }
  });
});
