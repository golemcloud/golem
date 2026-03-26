import { describe, it } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { ScenarioLoader } from "../src/executor.js";

async function writeTempYaml(content: string): Promise<string> {
  const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "loader-test-"));
  const filePath = path.join(tmpDir, "scenario.yaml");
  await fs.writeFile(filePath, content, "utf8");
  return filePath;
}

describe("ScenarioLoader", () => {
  it("loads a valid scenario", async () => {
    const filePath = await writeTempYaml(`
name: "test-scenario"
steps:
  - id: "step-1"
    prompt: "Do something"
    expectedSkills:
      - "some-skill"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.name, "test-scenario");
    assert.equal(spec.steps.length, 1);
    assert.equal(spec.steps[0].id, "step-1");
    assert.deepEqual(spec.steps[0].expectedSkills, ["some-skill"]);
  });

  it("loads a minimal valid scenario", async () => {
    const filePath = await writeTempYaml(`
name: "minimal"
steps:
  - prompt: "hello"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.name, "minimal");
    assert.equal(spec.steps.length, 1);
  });

  it("rejects scenario without name", async () => {
    const filePath = await writeTempYaml(`
steps:
  - prompt: "hello"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("Invalid scenario file"));
        return true;
      },
    );
  });

  it("rejects scenario without steps", async () => {
    const filePath = await writeTempYaml(`
name: "no-steps"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("Invalid scenario file"));
        return true;
      },
    );
  });

  it("rejects scenario with empty steps array", async () => {
    const filePath = await writeTempYaml(`
name: "empty-steps"
steps: []
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("at least one step"));
        return true;
      },
    );
  });

  it("rejects scenario with invalid step field types", async () => {
    const filePath = await writeTempYaml(`
name: "bad-types"
steps:
  - id: 123
    strictSkillMatch: "yes"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("Invalid scenario file"));
        return true;
      },
    );
  });

  // Settings/prerequisites tests

  it("loads scenario with settings", async () => {
    const filePath = await writeTempYaml(`
name: "with-settings"
settings:
  timeout_per_subprompt: 120
  golem_server:
    router_port: 9000
    custom_request_port: 9001
  cleanup: false
steps:
  - prompt: "hello"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.settings?.timeout_per_subprompt, 120);
    assert.equal(spec.settings?.golem_server?.router_port, 9000);
    assert.equal(spec.settings?.golem_server?.custom_request_port, 9001);
    assert.equal(spec.settings?.cleanup, false);
  });

  it("loads scenario with prerequisites", async () => {
    const filePath = await writeTempYaml(`
name: "with-prereqs"
prerequisites:
  env:
    MY_VAR: "test_value"
    OTHER_VAR: "other"
steps:
  - prompt: "hello"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.deepEqual(spec.prerequisites?.env, {
      MY_VAR: "test_value",
      OTHER_VAR: "other",
    });
  });

  it("loads step with timeout and continue_session", async () => {
    const filePath = await writeTempYaml(`
name: "step-options"
steps:
  - id: "step-1"
    prompt: "first"
    timeout: 60
  - id: "step-2"
    prompt: "second"
    continue_session: false
    timeout: 120
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].timeout, 60);
    assert.equal(spec.steps[1].continue_session, false);
    assert.equal(spec.steps[1].timeout, 120);
  });

  // Invoke tests

  it("loads step with invoke and expect", async () => {
    const filePath = await writeTempYaml(`
name: "invoke-test"
steps:
  - id: "invoke-step"
    invoke:
      agent: "my-agent"
      function: "my-func"
      args: '{"key": "value"}'
    expect:
      exit_code: 0
      stdout_contains: "success"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].invoke?.agent, "my-agent");
    assert.equal(spec.steps[0].invoke?.function, "my-func");
    assert.equal(spec.steps[0].expect?.exit_code, 0);
    assert.equal(spec.steps[0].expect?.stdout_contains, "success");
  });

  // Shell/sleep/trigger tests

  it("loads step with sleep", async () => {
    const filePath = await writeTempYaml(`
name: "sleep-test"
steps:
  - id: "wait"
    sleep: 5
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].sleep, 5);
  });

  it("loads step with shell command", async () => {
    const filePath = await writeTempYaml(`
name: "shell-test"
steps:
  - id: "run-cmd"
    shell:
      command: "echo"
      args: ["hello"]
      cwd: "./subdir"
    expect:
      stdout_contains: "hello"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].shell?.command, "echo");
    assert.deepEqual(spec.steps[0].shell?.args, ["hello"]);
    assert.equal(spec.steps[0].shell?.cwd, "./subdir");
  });

  it("loads step with trigger", async () => {
    const filePath = await writeTempYaml(`
name: "trigger-test"
steps:
  - id: "fire"
    trigger:
      agent: "my-agent"
      function: "do-thing"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].trigger?.agent, "my-agent");
    assert.equal(spec.steps[0].trigger?.function, "do-thing");
  });

  // Validation: exactly one action per step

  it("rejects step with two actions (prompt + shell)", async () => {
    const filePath = await writeTempYaml(`
name: "two-actions"
steps:
  - id: "bad"
    prompt: "hello"
    shell:
      command: "echo"
      args: ["hi"]
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("exactly one action"));
        return true;
      },
    );
  });

  it("rejects step with zero actions", async () => {
    const filePath = await writeTempYaml(`
name: "no-action"
steps:
  - id: "empty"
    expectedSkills:
      - "some-skill"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("exactly one action"));
        return true;
      },
    );
  });

  // Create/delete agent tests

  it("loads step with create_agent", async () => {
    const filePath = await writeTempYaml(`
name: "create-agent-test"
steps:
  - id: "create"
    create_agent:
      name: "test-agent"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].create_agent?.name, "test-agent");
  });

  it("loads step with delete_agent", async () => {
    const filePath = await writeTempYaml(`
name: "delete-agent-test"
steps:
  - id: "delete"
    delete_agent:
      name: "test-agent"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].delete_agent?.name, "test-agent");
  });

  // Tests for only_if/skip_if

  it("loads step with only_if condition", async () => {
    const filePath = await writeTempYaml(`
name: "only-if-test"
steps:
  - id: "conditional"
    prompt: "do something"
    only_if:
      agent: "claude-code"
      language: "ts"
      os: "macos"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.deepEqual(spec.steps[0].only_if, {
      agent: "claude-code",
      language: "ts",
      os: "macos",
    });
  });

  it("loads step with skip_if condition", async () => {
    const filePath = await writeTempYaml(`
name: "skip-if-test"
steps:
  - id: "skip-opencode"
    prompt: "do something"
    skip_if:
      agent: "opencode"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.deepEqual(spec.steps[0].skip_if, { agent: "opencode" });
  });

  it("loads step with both only_if and skip_if", async () => {
    const filePath = await writeTempYaml(`
name: "both-conditions"
steps:
  - id: "combo"
    prompt: "do something"
    only_if:
      language: "ts"
    skip_if:
      os: "windows"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.deepEqual(spec.steps[0].only_if, { language: "ts" });
    assert.deepEqual(spec.steps[0].skip_if, { os: "windows" });
  });

  // HTTP step tests

  it("loads step with http action", async () => {
    const filePath = await writeTempYaml(`
name: "http-test"
steps:
  - id: "healthcheck"
    http:
      url: "http://localhost:9881/healthcheck"
      method: "GET"
    expect:
      status: 200
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].http?.url, "http://localhost:9881/healthcheck");
    assert.equal(spec.steps[0].http?.method, "GET");
    assert.equal(spec.steps[0].expect?.status, 200);
  });

  it("loads step with http POST and headers", async () => {
    const filePath = await writeTempYaml(`
name: "http-post-test"
steps:
  - id: "post"
    http:
      url: "http://localhost:8080/api"
      method: "POST"
      body: '{"key":"value"}'
      headers:
        Content-Type: "application/json"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].http?.method, "POST");
    assert.equal(spec.steps[0].http?.body, '{"key":"value"}');
    assert.deepEqual(spec.steps[0].http?.headers, {
      "Content-Type": "application/json",
    });
  });

  // Retry schema tests

  it("loads step with retry config", async () => {
    const filePath = await writeTempYaml(`
name: "retry-test"
steps:
  - id: "flaky"
    prompt: "do something flaky"
    retry:
      attempts: 3
      delay: 2
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].retry?.attempts, 3);
    assert.equal(spec.steps[0].retry?.delay, 2);
  });
});
