import { describe, it } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { ScenarioLoader } from "../src/executor.js";

async function writeTempYaml(content: string): Promise<string> {
  const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "retry-test-"));
  const filePath = path.join(tmpDir, "scenario.yaml");
  await fs.writeFile(filePath, content, "utf8");
  return filePath;
}

describe("Retry schema", () => {
  it("loads step with retry config", async () => {
    const filePath = await writeTempYaml(`
name: "retry-test"
steps:
  - id: "flaky-step"
    prompt: "do something flaky"
    retry:
      attempts: 3
      delay: 2
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].retry?.attempts, 3);
    assert.equal(spec.steps[0].retry?.delay, 2);
  });

  it("loads step without retry (undefined)", async () => {
    const filePath = await writeTempYaml(`
name: "no-retry"
steps:
  - id: "stable"
    prompt: "do something"
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].retry, undefined);
  });

  it("rejects retry with zero attempts", async () => {
    const filePath = await writeTempYaml(`
name: "bad-retry"
steps:
  - id: "bad"
    prompt: "do something"
    retry:
      attempts: 0
      delay: 1
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("Invalid scenario file"));
        return true;
      },
    );
  });

  it("rejects retry with negative delay", async () => {
    const filePath = await writeTempYaml(`
name: "bad-delay"
steps:
  - id: "bad"
    prompt: "do something"
    retry:
      attempts: 2
      delay: -1
`);
    await assert.rejects(
      () => ScenarioLoader.load(filePath),
      (err: Error) => {
        assert.ok(err.message.includes("Invalid scenario file"));
        return true;
      },
    );
  });

  it("allows retry with fractional delay", async () => {
    const filePath = await writeTempYaml(`
name: "fractional-delay"
steps:
  - id: "step"
    prompt: "do something"
    retry:
      attempts: 2
      delay: 0.5
`);
    const spec = await ScenarioLoader.load(filePath);
    assert.equal(spec.steps[0].retry?.delay, 0.5);
  });
});
