import { describe, it } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { ScenarioLoader } from "../src/executor.js";

async function writeTempYaml(content: string): Promise<string> {
  const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "http-test-"));
  const filePath = path.join(tmpDir, "scenario.yaml");
  await fs.writeFile(filePath, content, "utf8");
  return filePath;
}

describe("HTTP step schema", () => {
  it("loads step with http GET", async () => {
    const filePath = await writeTempYaml(`
name: "http-get-test"
steps:
  - id: "check-health"
    http:
      url: "http://localhost:9881/healthcheck"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].http?.url, "http://localhost:9881/healthcheck");
    assert.equal(spec.steps[0].http?.method, "GET");
  });

  it("loads step with http POST and body", async () => {
    const filePath = await writeTempYaml(`
name: "http-post-test"
steps:
  - id: "post-data"
    http:
      url: "http://localhost:8080/api"
      method: "POST"
      body: '{"key": "value"}'
      headers:
        Content-Type: "application/json"
    expect:
      status: 200
      body_contains: "success"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].http?.method, "POST");
    assert.equal(spec.steps[0].http?.body, '{"key": "value"}');
    assert.deepEqual(spec.steps[0].http?.headers, {
      "Content-Type": "application/json",
    });
    assert.equal(spec.steps[0].expect?.status, 200);
  });

  it("rejects step with both http and prompt", async () => {
    const filePath = await writeTempYaml(`
name: "conflict-test"
steps:
  - id: "bad"
    prompt: "hello"
    http:
      url: "http://localhost:8080"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("exactly one action"));
        return true;
      },
    );
  });

  it("rejects step with both http and shell", async () => {
    const filePath = await writeTempYaml(`
name: "conflict-test-2"
steps:
  - id: "bad"
    shell:
      command: "echo"
      args: ["hi"]
    http:
      url: "http://localhost:8080"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("exactly one action"));
        return true;
      },
    );
  });

  it("rejects http with invalid method", async () => {
    const filePath = await writeTempYaml(`
name: "bad-method"
steps:
  - id: "bad"
    http:
      url: "http://localhost:8080"
      method: "INVALID"
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("Invalid scenario file"));
        return true;
      },
    );
  });

  it("loads http with all methods", async () => {
    for (const method of ["GET", "POST", "PUT", "DELETE", "PATCH"]) {
      const filePath = await writeTempYaml(`
name: "method-${method.toLowerCase()}"
steps:
  - id: "step"
    http:
      url: "http://localhost:8080"
      method: "${method}"
`);
      const spec = await ScenarioLoader.load(filePath);
      assert.equal(spec.steps[0].http?.method, method);
    }
  });
});
