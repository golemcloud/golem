import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { findGolemAppDir } from "../src/workspace.js";

describe("findGolemAppDir", () => {
  let tmpDir: string;

  beforeEach(async () => {
    tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "workspace-test-"));
  });

  afterEach(async () => {
    await fs.rm(tmpDir, { recursive: true, force: true });
  });

  it("returns workspace root when golem.yaml is in root", async () => {
    await fs.writeFile(path.join(tmpDir, "golem.yaml"), "app: test\n");
    const result = await findGolemAppDir(tmpDir);
    assert.equal(result, tmpDir);
  });

  it("finds golem.yaml in a subdirectory", async () => {
    const appDir = path.join(tmpDir, "my-app");
    await fs.mkdir(appDir);
    await fs.writeFile(path.join(appDir, "golem.yaml"), "app: my-app\n");
    const result = await findGolemAppDir(tmpDir);
    assert.equal(result, appDir);
  });

  it("skips dotfile directories like .claude", async () => {
    await fs.mkdir(path.join(tmpDir, ".claude", "skills"), { recursive: true });
    await fs.writeFile(path.join(tmpDir, ".claude", "golem.yaml"), "");
    const appDir = path.join(tmpDir, "deploy-test");
    await fs.mkdir(appDir);
    await fs.writeFile(path.join(appDir, "golem.yaml"), "app: deploy-test\n");
    const result = await findGolemAppDir(tmpDir);
    assert.equal(result, appDir);
  });

  it("prefers root golem.yaml over subdirectory", async () => {
    await fs.writeFile(path.join(tmpDir, "golem.yaml"), "app: root\n");
    const subDir = path.join(tmpDir, "sub-app");
    await fs.mkdir(subDir);
    await fs.writeFile(path.join(subDir, "golem.yaml"), "app: sub\n");
    const result = await findGolemAppDir(tmpDir);
    assert.equal(result, tmpDir);
  });

  it("falls back to workspace root when no golem.yaml exists", async () => {
    await fs.mkdir(path.join(tmpDir, "empty-dir"));
    const result = await findGolemAppDir(tmpDir);
    assert.equal(result, tmpDir);
  });
});
