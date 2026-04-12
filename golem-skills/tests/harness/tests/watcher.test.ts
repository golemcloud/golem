import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { SkillWatcher } from "../src/watcher.js";

describe("SkillWatcher", () => {
  describe("pathToSkillName", () => {
    it("extracts skill name from a valid SKILL.md path", () => {
      const watcher = new SkillWatcher("/tmp/skills");
      assert.equal(
        watcher.pathToSkillName("/tmp/workspace/.agents/skills/adding-dependencies/SKILL.md"),
        "adding-dependencies",
      );
    });

    it("extracts skill name from nested path", () => {
      const watcher = new SkillWatcher("/tmp/skills");
      assert.equal(
        watcher.pathToSkillName(
          "/some/other/path/test-app/.agents/skills/golem-new-project/SKILL.md",
        ),
        "golem-new-project",
      );
    });

    it("returns null for non-SKILL.md files", () => {
      const watcher = new SkillWatcher("/tmp/skills");
      assert.equal(watcher.pathToSkillName("/tmp/skills/adding-dependencies/README.md"), null);
    });

    it("returns null for SKILL.md outside agent skill directories", () => {
      const watcher = new SkillWatcher("/tmp/skills");
      assert.equal(watcher.pathToSkillName("/tmp/workspace/docs/SKILL.md"), null);
    });

    it("returns null for empty string", () => {
      const watcher = new SkillWatcher("/tmp/skills");
      assert.equal(watcher.pathToSkillName(""), null);
    });
  });

  describe("baseline and since tracking", () => {
    let watcher: SkillWatcher;

    beforeEach(() => {
      watcher = new SkillWatcher("/tmp/skills");
    });

    it("markBaseline returns 0 initially", () => {
      assert.equal(watcher.markBaseline(), 0);
    });

    it("getActivatedEventsSince returns empty for fresh watcher", () => {
      const baseline = watcher.markBaseline();
      assert.deepEqual(watcher.getActivatedEventsSince(baseline), []);
    });

    it("getActivatedSkills returns empty initially", () => {
      assert.deepEqual(watcher.getActivatedSkills(), []);
    });
  });

  describe("clearActivatedSkills", () => {
    it("resets all state", () => {
      const watcher = new SkillWatcher("/tmp/skills");
      watcher.clearActivatedSkills();
      assert.deepEqual(watcher.getActivatedSkills(), []);
      assert.equal(watcher.markBaseline(), 0);
    });
  });

  describe("snapshotAtimes", () => {
    let tmpDir: string;
    let watcher: SkillWatcher;
    let skillFile: string;

    beforeEach(async () => {
      tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "skill-watcher-"));
      skillFile = path.join(
        tmpDir,
        "test-app",
        ".agents",
        "skills",
        "golem-new-project",
        "SKILL.md",
      );
      await fs.mkdir(path.dirname(skillFile), { recursive: true });
      await fs.writeFile(skillFile, "bootstrap");
      watcher = new SkillWatcher(tmpDir);
    });

    afterEach(async () => {
      await fs.rm(tmpDir, { recursive: true, force: true });
    });

    it("tracks nested skill file access recursively", async () => {
      await watcher.snapshotAtimes();
      await fs.readFile(skillFile, "utf8");

      const changed = await watcher.getSkillsWithChangedAtime();

      assert.deepEqual(changed, [{ skillName: "golem-new-project", path: skillFile }]);
    });
  });
});
