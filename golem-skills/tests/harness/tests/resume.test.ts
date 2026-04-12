import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { ScenarioExecutor, type ScenarioSpec, type StepSpec } from "../src/executor.js";

function makePromptStep(id: string, prompt: string): StepSpec {
  return { tag: "prompt", id, prompt };
}
import type { AgentDriver } from "../src/driver/base.js";
import { SkillWatcher } from "../src/watcher.js";

// Minimal mock driver that always succeeds
const mockDriver: AgentDriver = {
  setup: async () => {},
  teardown: async () => {},
  sendPrompt: async () => ({
    success: true,
    output: "ok",
    durationSeconds: 0,
    exitCode: 0,
  }),
  sendFollowup: async () => ({
    success: true,
    output: "ok",
    durationSeconds: 0,
    exitCode: 0,
  }),
  setWorkingDirectory: () => {},
};

// Minimal mock watcher
function createMockWatcher(): SkillWatcher {
  const watcher = new SkillWatcher("/tmp/fake-workspace");
  // Override start/stop to be no-ops
  watcher.start = async () => {};
  watcher.stop = async () => {};
  watcher.markBaseline = () => Date.now();
  watcher.snapshotAtimes = async () => {};
  watcher.getActivatedEventsSince = () => [];
  watcher.getSkillsWithChangedAtime = async () => [];
  return watcher;
}

describe("Resume-from validation", () => {
  it("throws when resumeFromStepId does not exist", async () => {
    const spec: ScenarioSpec = {
      name: "test",
      steps: [makePromptStep("step-1", "hello"), makePromptStep("step-2", "world")],
    };

    const executor = new ScenarioExecutor(
      mockDriver,
      createMockWatcher(),
      "/tmp/fake-workspace",
      "/tmp/bootstrap-skill",
      { resumeFromStepId: "nonexistent-step" },
    );

    await assert.rejects(
      () => executor.execute(spec),
      (err: Error) => {
        assert.ok(err.message.includes("nonexistent-step"));
        assert.ok(err.message.includes("not found"));
        return true;
      },
    );
  });

  it("accepts valid resumeFromStepId", async () => {
    const spec: ScenarioSpec = {
      name: "test",
      steps: [makePromptStep("step-1", "hello"), makePromptStep("step-2", "world")],
    };

    // This should not throw validation error (it will fail at verifyGolemConnectivity
    // but that's expected — we're just testing the validation)
    const executor = new ScenarioExecutor(
      mockDriver,
      createMockWatcher(),
      "/tmp/fake-workspace-resume",
      "/tmp/bootstrap-skill",
      { resumeFromStepId: "step-2" },
    );

    // It will throw because of golem connectivity check, not resume validation
    try {
      await executor.execute(spec);
    } catch (err) {
      // Expected to fail at connectivity check, not at resume validation
      assert.ok(
        !(err as Error).message.includes("not found"),
        "Should not fail at resume validation",
      );
    }
  });
});
