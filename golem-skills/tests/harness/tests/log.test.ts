import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { stripVTControlCharacters } from "node:util";
import * as log from "../src/log.js";

const LONGEST_TAG_WIDTH = "[claude-code]".length;

function prefix(tag: string): string {
  return `[${tag}]`.padEnd(LONGEST_TAG_WIDTH) + " ";
}

function captureLogs(fn: () => void): string[] {
  const originalLog = console.log;
  const lines: string[] = [];

  console.log = (...args: unknown[]) => {
    lines.push(args.map(String).join(" "));
  };

  try {
    fn();
  } finally {
    console.log = originalLog;
  }

  return lines.map((line) => stripVTControlCharacters(line));
}

describe("step logging", () => {
  it("formats lifecycle events with step prefixes", () => {
    const lines = captureLogs(() => {
      log.stepStart("create-project", 300);
      log.stepSkip("create-project", "condition not met");
      log.stepRetry("create-project", 2, 3, 5);
    });

    assert.equal(lines[0], `${prefix("step")}create-project ▶ start timeout=300s`);
    assert.equal(lines[1], `${prefix("step")}create-project ↷ skipped condition not met`);
    assert.equal(lines[2], `${prefix("step")}create-project ↻ retry 2/3 delay=5s`);
  });

  it("formats common verification and command actions semantically", () => {
    const lines = captureLogs(() => {
      log.stepAction("create-project", "verifying 2 expected files");
      log.stepAction("create-project", "expected file exists: test-app/src/main.ts");
      log.stepAction("create-project", "running golem build in /tmp/test-app");
      log.stepAction("create-project", "running golem deploy in /tmp/test-app");
    });

    assert.equal(lines[0], `${prefix("step")}create-project • verify expected files count=2`);
    assert.equal(lines[1], `${prefix("step")}create-project ✓ file test-app/src/main.ts`);
    assert.equal(lines[2], `${prefix("step")}create-project ▶ golem build cwd=/tmp/test-app`);
    assert.equal(lines[3], `${prefix("step")}create-project ▶ golem deploy cwd=/tmp/test-app`);
  });

  it("logs the full prompt body for initial prompts and followups", () => {
    const lines = captureLogs(() => {
      log.stepPrompt("create-project", "Create a new app\nThen build it", "initial");
      log.stepPrompt("create-project", "Add a health endpoint", "followup");
    });

    assert.equal(lines[0], `${prefix("step")}create-project ▶ prompt`);
    assert.equal(lines[1], `${prefix("step")}create-project │ Create a new app`);
    assert.equal(lines[2], `${prefix("step")}create-project │ Then build it`);
    assert.equal(lines[3], `${prefix("step")}create-project ▶ prompt followup`);
    assert.equal(lines[4], `${prefix("step")}create-project │ Add a health endpoint`);
  });

  it("formats skill detection and activation summaries", () => {
    const lines = captureLogs(() => {
      log.stepSkillDetected(
        "create-project",
        "atime",
        "golem-new-project",
        "/tmp/workspace/.agents/skills/golem-new-project/SKILL.md",
      );
      log.stepActivatedSkills("create-project", ["golem-new-project"]);
      log.stepActivatedSkills("create-project", []);
    });

    assert.equal(
      lines[0],
      `${prefix("step")}create-project ◆ skill golem-new-project detected via atime /tmp/workspace/.agents/skills/golem-new-project/SKILL.md`,
    );
    assert.equal(lines[1], `${prefix("step")}create-project ✓ skills count=1 golem-new-project`);
    assert.equal(lines[2], `${prefix("step")}create-project • skills none activated`);
  });

  it("pads run and driver prefixes to the longest tag width", () => {
    const lines = captureLogs(() => {
      log.info("hello");
      log.driver("amp", "tool output");
      log.driver("claude-code", "tool output");
    });

    assert.equal(lines[0], `${prefix("run")}hello`);
    assert.equal(lines[1], `${prefix("amp")}tool output`);
    assert.equal(lines[2], `${prefix("claude-code")}tool output`);
  });
});

describe("scenario logging", () => {
  it("formats scenario lifecycle lines with a consistent prefix", () => {
    const lines = captureLogs(() => {
      log.scenarioSkip("create-a-new-project");
      log.scenarioPass("create-a-new-project");
      log.scenarioFail("create-a-new-project");
    });

    assert.equal(
      lines[0],
      `${prefix("scenario")}create-a-new-project ↷ skipped skip_if condition met`,
    );
    assert.equal(lines[1], `${prefix("scenario")}create-a-new-project ✓ passed`);
    assert.equal(lines[2], `${prefix("scenario")}create-a-new-project ✗ failed`);
  });
});
