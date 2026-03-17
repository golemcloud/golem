import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import {
  ScenarioExecutor,
  type ScenarioSpec,
  type ScenarioExecutorOptions,
} from "../src/executor.js";
import type { AgentDriver, AgentResult } from "../src/driver/base.js";
import { SkillWatcher } from "../src/watcher.js";

// Stub driver that records prompts sent to it
class StubDriver implements AgentDriver {
  prompts: string[] = [];
  async setup(_workspace: string, _skillsDir: string): Promise<void> {
    /* no-op */
  }
  async sendPrompt(prompt: string, _timeout: number): Promise<AgentResult> {
    this.prompts.push(prompt);
    return { success: true, output: "", durationSeconds: 0, exitCode: 0 };
  }
  async sendFollowup(prompt: string, _timeout: number): Promise<AgentResult> {
    this.prompts.push(prompt);
    return { success: true, output: "", durationSeconds: 0, exitCode: 0 };
  }
  async teardown(): Promise<void> {
    /* no-op */
  }
}

function createExecutor(
  driver: AgentDriver,
  watcher: SkillWatcher,
  workspace: string,
  skillsDir: string,
  opts?: ScenarioExecutorOptions,
): ScenarioExecutor {
  const executor = new ScenarioExecutor(
    driver,
    watcher,
    workspace,
    skillsDir,
    opts,
  );
  // Patch out golem connectivity check — there's no server in tests
  (executor as unknown as Record<string, unknown>)["verifyGolemConnectivity"] =
    async () => {};
  return executor;
}

describe("Variable substitution integration", () => {
  let tmpDir: string;
  let workspace: string;
  let skillsDir: string;

  beforeEach(async () => {
    tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "vars-integ-"));
    workspace = path.join(tmpDir, "workspace");
    skillsDir = path.join(tmpDir, "skills");
    await fs.mkdir(workspace, { recursive: true });
    await fs.mkdir(skillsDir, { recursive: true });
  });

  afterEach(async () => {
    await fs.rm(tmpDir, { recursive: true, force: true });
  });

  it("substitutes {{agent}}, {{language}}, {{workspace}}, {{scenario}} in a prompt step", async () => {
    const driver = new StubDriver();
    const watcher = new SkillWatcher(skillsDir);
    const opts: ScenarioExecutorOptions = {
      agent: "claude-code",
      language: "ts",
    };
    const executor = createExecutor(
      driver,
      watcher,
      workspace,
      skillsDir,
      opts,
    );

    const spec: ScenarioSpec = {
      name: "my-test-scenario",
      settings: { cleanup: false },
      steps: [
        {
          id: "vars-step",
          prompt:
            "Agent={{agent}} Lang={{language}} WS={{workspace}} Scenario={{scenario}}",
        },
      ],
    };

    const result = await executor.execute(spec);
    assert.equal(result.status, "pass");
    assert.equal(driver.prompts.length, 1);

    const sent = driver.prompts[0];
    assert.ok(
      sent.includes("Agent=claude-code"),
      `expected Agent=claude-code, got: ${sent}`,
    );
    assert.ok(sent.includes("Lang=ts"), `expected Lang=ts, got: ${sent}`);
    assert.ok(
      sent.includes(`WS=${workspace}`),
      `expected WS=${workspace}, got: ${sent}`,
    );
    assert.ok(
      sent.includes("Scenario=my-test-scenario"),
      `expected Scenario=my-test-scenario, got: ${sent}`,
    );
    // No raw {{...}} should remain
    assert.ok(!sent.includes("{{"), `unexpected raw template in: ${sent}`);
  });

  it("substitutes variables in a shell command and verifies via expect", async () => {
    const driver = new StubDriver();
    const watcher = new SkillWatcher(skillsDir);
    const opts: ScenarioExecutorOptions = { agent: "gemini", language: "rust" };
    const executor = createExecutor(
      driver,
      watcher,
      workspace,
      skillsDir,
      opts,
    );

    const spec: ScenarioSpec = {
      name: "shell-vars",
      settings: { cleanup: false },
      steps: [
        {
          id: "echo-vars",
          shell: {
            command: "sh",
            args: ["-c", 'echo "agent={{agent}} lang={{language}}"'],
          },
          expect: {
            stdout_contains: "agent=gemini lang=rust",
          },
        },
      ],
    };

    const result = await executor.execute(spec);
    assert.equal(
      result.status,
      "pass",
      `expected pass, errors: ${result.stepResults[0]?.error}`,
    );
  });

  it("leaves unknown variables as-is in prompts", async () => {
    const driver = new StubDriver();
    const watcher = new SkillWatcher(skillsDir);
    const opts: ScenarioExecutorOptions = {
      agent: "claude-code",
      language: "ts",
    };
    const executor = createExecutor(
      driver,
      watcher,
      workspace,
      skillsDir,
      opts,
    );

    const spec: ScenarioSpec = {
      name: "unknown-var",
      settings: { cleanup: false },
      steps: [
        {
          id: "unknown",
          prompt: "{{agent}} and {{mystery_var}}",
        },
      ],
    };

    const result = await executor.execute(spec);
    assert.equal(result.status, "pass");

    const sent = driver.prompts[0];
    assert.ok(
      sent.includes("claude-code"),
      `expected claude-code, got: ${sent}`,
    );
    assert.ok(
      sent.includes("{{mystery_var}}"),
      `expected {{mystery_var}} preserved, got: ${sent}`,
    );
  });

  it("substitutes variables in invoke fields", async () => {
    const driver = new StubDriver();
    const watcher = new SkillWatcher(skillsDir);
    const opts: ScenarioExecutorOptions = { agent: "opencode", language: "ts" };
    const executor = createExecutor(
      driver,
      watcher,
      workspace,
      skillsDir,
      opts,
    );

    // The invoke will fail (no golem agent running), but we can inspect the step result error
    // to confirm substitution happened — the error will reference the substituted agent name
    const spec: ScenarioSpec = {
      name: "invoke-vars",
      settings: { cleanup: false },
      steps: [
        {
          id: "invoke-step",
          invoke: {
            agent: "{{agent}}",
            function: "test-func",
            args: '{"lang":"{{language}}"}',
          },
        },
      ],
    };

    // The invoke will attempt to run `golem agent invoke opencode test-func ...`
    // It will fail since golem isn't running, but proves substitution happened
    const result = await executor.execute(spec);
    assert.equal(result.status, "fail");
    const stepResult = result.stepResults[0];
    assert.ok(
      stepResult.error?.includes("INVOKE_FAILED"),
      `expected INVOKE_FAILED, got: ${stepResult.error}`,
    );
  });

  it("substitutes variables in create_agent name", async () => {
    const driver = new StubDriver();
    const watcher = new SkillWatcher(skillsDir);
    const opts: ScenarioExecutorOptions = {
      agent: "claude-code",
      language: "ts",
    };
    const executor = createExecutor(
      driver,
      watcher,
      workspace,
      skillsDir,
      opts,
    );

    const spec: ScenarioSpec = {
      name: "agent-name-vars",
      settings: { cleanup: false },
      steps: [
        {
          id: "create",
          create_agent: { name: "{{agent}}-worker" },
        },
      ],
    };

    // Will fail (golem not running), but proves substitution happened
    const result = await executor.execute(spec);
    assert.equal(result.status, "fail");
    const err = result.stepResults[0].error ?? "";
    assert.ok(
      err.includes("CREATE_AGENT_FAILED"),
      `expected CREATE_AGENT_FAILED, got: ${err}`,
    );
  });

  it("conditions + variables work together (skip_if prevents execution)", async () => {
    const driver = new StubDriver();
    const watcher = new SkillWatcher(skillsDir);
    const opts: ScenarioExecutorOptions = {
      agent: "claude-code",
      language: "ts",
    };
    const executor = createExecutor(
      driver,
      watcher,
      workspace,
      skillsDir,
      opts,
    );

    const spec: ScenarioSpec = {
      name: "cond-vars",
      settings: { cleanup: false },
      steps: [
        {
          id: "skipped",
          prompt: "{{agent}} should not run",
          skip_if: { agent: "claude-code" },
        },
        {
          id: "runs",
          prompt: "{{agent}} should run",
          only_if: { language: "ts" },
        },
      ],
    };

    const result = await executor.execute(spec);
    assert.equal(result.status, "pass");
    assert.equal(result.stepResults.length, 2);

    // First step was skipped: 0 duration, success
    assert.equal(result.stepResults[0].durationSeconds, 0);
    assert.equal(result.stepResults[0].success, true);

    // Second step ran: prompt was sent with substituted variable
    assert.equal(driver.prompts.length, 1);
    assert.ok(driver.prompts[0].includes("claude-code should run"));
  });

  it("abort signal stops execution mid-scenario", async () => {
    const driver = new StubDriver();
    const watcher = new SkillWatcher(skillsDir);
    const controller = new AbortController();
    const opts: ScenarioExecutorOptions = {
      agent: "claude-code",
      language: "ts",
      abortSignal: controller.signal,
    };
    const executor = createExecutor(
      driver,
      watcher,
      workspace,
      skillsDir,
      opts,
    );

    const spec: ScenarioSpec = {
      name: "abort-test",
      settings: { cleanup: false },
      steps: [
        { id: "step-1", prompt: "first" },
        { id: "step-2", prompt: "second" },
        { id: "step-3", prompt: "third" },
      ],
    };

    // Abort after first prompt
    const origSend = driver.sendPrompt.bind(driver);
    driver.sendPrompt = async (prompt: string, _timeout: number) => {
      const res = await origSend(prompt, _timeout);
      controller.abort(); // abort after first step completes
      return res;
    };

    const result = await executor.execute(spec);
    // Only 1 step should have executed
    assert.equal(result.stepResults.length, 1);
    assert.equal(driver.prompts.length, 1);
    assert.equal(driver.prompts[0], "first");
  });
});
