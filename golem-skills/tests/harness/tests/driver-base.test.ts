import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { BaseAgentDriver, type AgentResult } from "../src/driver/base.js";

class TestDriver extends BaseAgentDriver {
  protected readonly driverName = "test";
  protected readonly skillDirs = [".agents/skills"];

  async sendPrompt(_prompt: string, _timeout: number): Promise<AgentResult> {
    return { success: true, output: "", durationSeconds: 0, exitCode: 0 };
  }

  async sendFollowup(_prompt: string, _timeout: number): Promise<AgentResult> {
    return { success: true, output: "", durationSeconds: 0, exitCode: 0 };
  }

  async teardown(): Promise<void> {
    // no-op
  }
}

describe("BaseAgentDriver bootstrap skill setup", () => {
  let tmpDir: string;
  let workspace: string;
  let bootstrapSkillSourceDir: string;

  beforeEach(async () => {
    tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "driver-base-"));
    workspace = path.join(tmpDir, "workspace");
    bootstrapSkillSourceDir = path.join(tmpDir, "bootstrap");
    await fs.mkdir(workspace, { recursive: true });
    await fs.mkdir(bootstrapSkillSourceDir, { recursive: true });
    await fs.writeFile(path.join(bootstrapSkillSourceDir, "SKILL.md"), "bootstrap");
    await fs.writeFile(path.join(bootstrapSkillSourceDir, "README.md"), "extra");
  });

  afterEach(async () => {
    await fs.rm(tmpDir, { recursive: true, force: true });
  });

  it("copies the bootstrap skill into the agent skill directory", async () => {
    const driver = new TestDriver();

    await driver.setup(workspace, bootstrapSkillSourceDir);

    const skillPath = path.join(workspace, ".agents/skills/golem-new-project/SKILL.md");
    assert.equal(await fs.readFile(skillPath, "utf8"), "bootstrap");

    const extraPath = path.join(workspace, ".agents/skills/golem-new-project/README.md");
    assert.equal(await fs.readFile(extraPath, "utf8"), "extra");
  });
});
