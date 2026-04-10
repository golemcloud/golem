import { describe, it } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { parseStep, ScenarioLoader, type StepSpec } from "../src/executor.js";

async function loadScenarioYaml(content: string) {
  const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "parse-step-test-"));
  const filePath = path.join(tmpDir, "scenario.yaml");
  await fs.writeFile(filePath, content, "utf8");
  return ScenarioLoader.load(filePath);
}

describe("parseStep", () => {
  it("parses a prompt step", () => {
    const result = parseStep({ prompt: "hello world" });
    assert.equal(result.tag, "prompt");
    if (result.tag === "prompt") {
      assert.equal(result.prompt, "hello world");
    }
  });

  it("parses an invoke step", () => {
    const result = parseStep({
      invoke: { agent: "my-agent", method: "do-thing", args: '{"x":1}' },
    });
    assert.equal(result.tag, "invoke");
    if (result.tag === "invoke") {
      assert.equal(result.invoke.agent, "my-agent");
      assert.equal(result.invoke.method, "do-thing");
      assert.equal(result.invoke.args, '{"x":1}');
    }
  });

  it("parses a shell step", () => {
    const result = parseStep({
      shell: { command: "echo", args: ["hi"], cwd: "/tmp" },
    });
    assert.equal(result.tag, "shell");
    if (result.tag === "shell") {
      assert.equal(result.shell.command, "echo");
      assert.deepEqual(result.shell.args, ["hi"]);
      assert.equal(result.shell.cwd, "/tmp");
    }
  });

  it("parses a trigger step", () => {
    const result = parseStep({
      trigger: { agent: "worker", method: "run" },
    });
    assert.equal(result.tag, "trigger");
    if (result.tag === "trigger") {
      assert.equal(result.trigger.agent, "worker");
      assert.equal(result.trigger.method, "run");
    }
  });

  it("parses a create_agent step", () => {
    const result = parseStep({
      create_agent: { name: "new-agent", env: { KEY: "val" } },
    });
    assert.equal(result.tag, "create_agent");
    if (result.tag === "create_agent") {
      assert.equal(result.create_agent.name, "new-agent");
      assert.deepEqual(result.create_agent.env, { KEY: "val" });
    }
  });

  it("parses a delete_agent step", () => {
    const result = parseStep({ delete_agent: { name: "old-agent" } });
    assert.equal(result.tag, "delete_agent");
    if (result.tag === "delete_agent") {
      assert.equal(result.delete_agent.name, "old-agent");
    }
  });

  it("parses a sleep step", () => {
    const result = parseStep({ sleep: 10 });
    assert.equal(result.tag, "sleep");
    if (result.tag === "sleep") {
      assert.equal(result.sleep, 10);
    }
  });

  it("parses an http step", () => {
    const result = parseStep({
      http: {
        url: "http://localhost:8080/api",
        method: "POST",
        body: '{"a":1}',
        headers: { "Content-Type": "application/json" },
      },
    });
    assert.equal(result.tag, "http");
    if (result.tag === "http") {
      assert.equal(result.http.url, "http://localhost:8080/api");
      assert.equal(result.http.method, "POST");
      assert.equal(result.http.body, '{"a":1}');
      assert.deepEqual(result.http.headers, { "Content-Type": "application/json" });
    }
  });

  it("preserves common fields", () => {
    const result = parseStep({
      id: "step-1",
      prompt: "do it",
      expectedSkills: ["skill-a"],
      allowedExtraSkills: ["skill-b"],
      strictSkillMatch: true,
      timeout: 60,
      continue_session: true,
      verify: { build: true, deploy: false },
      only_if: { language: "ts" },
      skip_if: { os: "windows" },
      retry: { attempts: 3, delay: 2 },
    });
    assert.equal(result.id, "step-1");
    assert.deepEqual(result.expectedSkills, ["skill-a"]);
    assert.deepEqual(result.allowedExtraSkills, ["skill-b"]);
    assert.equal(result.strictSkillMatch, true);
    assert.equal(result.timeout, 60);
    assert.equal(result.continue_session, true);
    assert.deepEqual(result.verify, { build: true, deploy: false });
    assert.deepEqual(result.only_if, { language: "ts" });
    assert.deepEqual(result.skip_if, { os: "windows" });
    assert.deepEqual(result.retry, { attempts: 3, delay: 2 });
  });

  it("omits undefined common fields from output", () => {
    const result = parseStep({ prompt: "minimal" });
    assert.equal(result.tag, "prompt");
    assert.equal(Object.prototype.hasOwnProperty.call(result, "id"), false);
    assert.equal(Object.prototype.hasOwnProperty.call(result, "timeout"), false);
    assert.equal(Object.prototype.hasOwnProperty.call(result, "retry"), false);
    assert.equal(Object.prototype.hasOwnProperty.call(result, "only_if"), false);
  });

  it("throws when no action field is present", () => {
    assert.throws(
      () => parseStep({ id: "no-action" } as Parameters<typeof parseStep>[0]),
      (err: Error) => {
        assert.ok(err.message.includes("no action field"));
        return true;
      },
    );
  });

  it("does not carry extra action fields onto the model", () => {
    const result = parseStep({ prompt: "only this" });
    assert.equal(result.tag, "prompt");
    assert.equal(Object.prototype.hasOwnProperty.call(result, "invoke"), false);
    assert.equal(Object.prototype.hasOwnProperty.call(result, "shell"), false);
    assert.equal(Object.prototype.hasOwnProperty.call(result, "sleep"), false);
    assert.equal(Object.prototype.hasOwnProperty.call(result, "http"), false);
  });
});

describe("multi-step scenario parsing", () => {
  it("parses a build-deploy scenario with mixed action types", async () => {
    const spec = await loadScenarioYaml(`
name: "build-deploy-flow"
settings:
  timeout_per_subprompt: 300
steps:
  - id: "create-project"
    prompt: "Create a new project called my-app"
    expectedSkills: ["golem-new-project"]
    verify:
      deploy: true

  - id: "check-files"
    shell:
      command: "ls"
      args: ["my-app/golem.yaml"]
    expect:
      exit_code: 0

  - id: "wait-for-deploy"
    sleep: 5

  - id: "healthcheck"
    http:
      url: "http://my-app.localhost:9006/health"
      method: "GET"
    expect:
      status: 200
    retry:
      attempts: 3
      delay: 2
`);
    assert.equal(spec.name, "build-deploy-flow");
    assert.equal(spec.steps.length, 4);

    const [create, check, wait, health] = spec.steps;

    assert.equal(create.tag, "prompt");
    if (create.tag === "prompt") {
      assert.equal(create.prompt, "Create a new project called my-app");
    }
    assert.deepEqual(create.expectedSkills, ["golem-new-project"]);
    assert.deepEqual(create.verify, { deploy: true });

    assert.equal(check.tag, "shell");
    if (check.tag === "shell") {
      assert.equal(check.shell.command, "ls");
      assert.deepEqual(check.shell.args, ["my-app/golem.yaml"]);
    }
    assert.equal(check.expect?.exit_code, 0);

    assert.equal(wait.tag, "sleep");
    if (wait.tag === "sleep") {
      assert.equal(wait.sleep, 5);
    }

    assert.equal(health.tag, "http");
    if (health.tag === "http") {
      assert.equal(health.http.url, "http://my-app.localhost:9006/health");
      assert.equal(health.http.method, "GET");
    }
    assert.equal(health.expect?.status, 200);
    assert.deepEqual(health.retry, { attempts: 3, delay: 2 });
  });

  it("parses agent lifecycle scenario (create, invoke, trigger, delete)", async () => {
    const spec = await loadScenarioYaml(`
name: "agent-lifecycle"
steps:
  - id: "spawn"
    create_agent:
      name: "worker-agent"
      env:
        MODE: "test"
      config:
        log_level: "debug"

  - id: "call-sync"
    invoke:
      agent: "worker-agent"
      method: "process-item"
      args: '{"item_id": 42}'
    expect:
      exit_code: 0
      stdout_contains: "processed"

  - id: "fire-async"
    trigger:
      agent: "worker-agent"
      method: "background-cleanup"

  - id: "settle"
    sleep: 2

  - id: "teardown"
    delete_agent:
      name: "worker-agent"
`);
    assert.equal(spec.steps.length, 5);

    const [spawn, call, fire, settle, teardown] = spec.steps;

    assert.equal(spawn.tag, "create_agent");
    if (spawn.tag === "create_agent") {
      assert.equal(spawn.create_agent.name, "worker-agent");
      assert.deepEqual(spawn.create_agent.env, { MODE: "test" });
      assert.deepEqual(spawn.create_agent.config, { log_level: "debug" });
    }

    assert.equal(call.tag, "invoke");
    if (call.tag === "invoke") {
      assert.equal(call.invoke.agent, "worker-agent");
      assert.equal(call.invoke.method, "process-item");
      assert.equal(call.invoke.args, '{"item_id": 42}');
    }
    assert.equal(call.expect?.stdout_contains, "processed");

    assert.equal(fire.tag, "trigger");
    if (fire.tag === "trigger") {
      assert.equal(fire.trigger.agent, "worker-agent");
      assert.equal(fire.trigger.method, "background-cleanup");
    }

    assert.equal(settle.tag, "sleep");
    if (settle.tag === "sleep") assert.equal(settle.sleep, 2);

    assert.equal(teardown.tag, "delete_agent");
    if (teardown.tag === "delete_agent") {
      assert.equal(teardown.delete_agent.name, "worker-agent");
    }
  });

  it("parses scenario with conditional steps and skill matching", async () => {
    const spec = await loadScenarioYaml(`
name: "conditional-flow"
skip_if:
  os: "windows"
steps:
  - id: "ts-only-step"
    prompt: "Generate TypeScript bindings"
    only_if:
      language: "ts"
    expectedSkills: ["golem-codegen"]
    strictSkillMatch: true

  - id: "skip-on-opencode"
    shell:
      command: "golem"
      args: ["build"]
    skip_if:
      agent: "opencode"
    timeout: 120

  - id: "multi-condition"
    prompt: "Run integration tests"
    only_if:
      language: "ts"
      os: "linux"
    skip_if:
      agent: "opencode"
    continue_session: true
    allowedExtraSkills: ["golem-test-runner"]
`);
    assert.equal(spec.name, "conditional-flow");
    assert.deepEqual(spec.skip_if, { os: "windows" });
    assert.equal(spec.steps.length, 3);

    const [tsOnly, skipOpen, multi] = spec.steps;

    assert.deepEqual(tsOnly.only_if, { language: "ts" });
    assert.equal(tsOnly.strictSkillMatch, true);
    assert.deepEqual(tsOnly.expectedSkills, ["golem-codegen"]);

    assert.deepEqual(skipOpen.skip_if, { agent: "opencode" });
    assert.equal(skipOpen.timeout, 120);

    assert.deepEqual(multi.only_if, { language: "ts", os: "linux" });
    assert.deepEqual(multi.skip_if, { agent: "opencode" });
    assert.equal(multi.continue_session, true);
    assert.deepEqual(multi.allowedExtraSkills, ["golem-test-runner"]);
  });

  it("parses HTTP-heavy integration test scenario", async () => {
    const spec = await loadScenarioYaml(`
name: "api-integration-test"
settings:
  golem_server:
    custom_request_port: 9006
    router_port: 9000
steps:
  - id: "setup"
    prompt: "Create and deploy a REST API app"
    verify:
      build: true
      deploy: true

  - id: "create-resource"
    http:
      url: "http://api.localhost:9006/items"
      method: "POST"
      body: '{"name": "widget", "count": 10}'
      headers:
        Content-Type: "application/json"
        X-Request-Id: "test-001"
    expect:
      status: 200
      body_contains: "widget"
    retry:
      attempts: 3
      delay: 5

  - id: "read-resource"
    http:
      url: "http://api.localhost:9006/items/default"
      method: "GET"
    expect:
      status: 200
      body_matches: "widget"

  - id: "update-resource"
    http:
      url: "http://api.localhost:9006/items/default"
      method: "PUT"
      body: '{"name": "widget", "count": 20}'
      headers:
        Content-Type: "application/json"
    expect:
      status: 200

  - id: "delete-resource"
    http:
      url: "http://api.localhost:9006/items/default"
      method: "DELETE"
    expect:
      status: 200

  - id: "verify-deleted"
    shell:
      command: "bash"
      args: ["-c", "curl -s -o /dev/null -w '%{http_code}' http://api.localhost:9006/items/default"]
    expect:
      stdout_contains: "404"
`);
    assert.equal(spec.steps.length, 6);
    assert.deepEqual(spec.settings?.golem_server, {
      custom_request_port: 9006,
      router_port: 9000,
    });

    const tags = spec.steps.map((s) => s.tag);
    assert.deepEqual(tags, ["prompt", "http", "http", "http", "http", "shell"]);

    // Verify CRUD http methods
    const httpSteps = spec.steps.filter((s): s is Extract<StepSpec, { tag: "http" }> => s.tag === "http");
    const methods = httpSteps.map((s) => s.http.method);
    assert.deepEqual(methods, ["POST", "GET", "PUT", "DELETE"]);

    // Verify headers on POST
    assert.deepEqual(httpSteps[0].http.headers, {
      "Content-Type": "application/json",
      "X-Request-Id": "test-001",
    });
  });

  it("parses scenario with all step types interleaved", async () => {
    const spec = await loadScenarioYaml(`
name: "kitchen-sink"
prerequisites:
  env:
    DB_URL: "postgres://localhost/test"
settings:
  timeout_per_subprompt: 600
  cleanup: true
steps:
  - id: "init"
    prompt: "Initialize the project"
  - id: "spawn-worker"
    create_agent:
      name: "bg-worker"
  - id: "build"
    shell:
      command: "golem"
      args: ["build"]
    expect:
      exit_code: 0
  - id: "warmup"
    sleep: 3
  - id: "kick-off"
    trigger:
      agent: "bg-worker"
      method: "start"
  - id: "call-worker"
    invoke:
      agent: "bg-worker"
      method: "status"
    expect:
      stdout_contains: "running"
  - id: "check-api"
    http:
      url: "http://localhost:8080/status"
    expect:
      status: 200
  - id: "cleanup-worker"
    delete_agent:
      name: "bg-worker"
`);
    assert.equal(spec.name, "kitchen-sink");
    assert.deepEqual(spec.prerequisites, { env: { DB_URL: "postgres://localhost/test" } });
    assert.equal(spec.settings?.cleanup, true);
    assert.equal(spec.steps.length, 8);

    const tags = spec.steps.map((s) => s.tag);
    assert.deepEqual(tags, [
      "prompt", "create_agent", "shell", "sleep",
      "trigger", "invoke", "http", "delete_agent",
    ]);

    // Each step has the correct id
    const ids = spec.steps.map((s) => s.id);
    assert.deepEqual(ids, [
      "init", "spawn-worker", "build", "warmup",
      "kick-off", "call-worker", "check-api", "cleanup-worker",
    ]);
  });

  it("no action fields leak across steps in a multi-step scenario", async () => {
    const spec = await loadScenarioYaml(`
name: "isolation-check"
steps:
  - id: "a"
    prompt: "hello"
  - id: "b"
    sleep: 1
  - id: "c"
    http:
      url: "http://localhost"
`);
    for (const step of spec.steps) {
      const actionKeys = ["prompt", "invoke", "invoke_json", "shell", "trigger", "create_agent", "delete_agent", "sleep", "http"] as const;
      const present = actionKeys.filter((k) => Object.prototype.hasOwnProperty.call(step, k));
      assert.equal(present.length, 1, `Step "${step.id}" should have exactly one action field, got: ${present.join(", ")}`);
      assert.equal(present[0], step.tag, `Step "${step.id}" action field should match tag`);
    }
  });
});
