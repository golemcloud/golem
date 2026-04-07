import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { shouldRunStep, type StepCondition } from "../src/executor.js";

function makeConditions(overrides: { only_if?: StepCondition; skip_if?: StepCondition } = {}) {
  return overrides;
}

const ctx = { agent: "claude-code", language: "ts", os: "darwin" };

describe("shouldRunStep", () => {
  it("runs step with no conditions", () => {
    assert.equal(shouldRunStep(makeConditions(), ctx), true);
  });

  it("runs step when only_if matches", () => {
    const step = makeConditions({ only_if: { agent: "claude-code" } });
    assert.equal(shouldRunStep(step, ctx), true);
  });

  it("skips step when only_if does not match", () => {
    const step = makeConditions({ only_if: { agent: "opencode" } });
    assert.equal(shouldRunStep(step, ctx), false);
  });

  it("runs step when only_if language matches", () => {
    const step = makeConditions({ only_if: { language: "ts" } });
    assert.equal(shouldRunStep(step, ctx), true);
  });

  it("skips step when only_if language does not match", () => {
    const step = makeConditions({ only_if: { language: "rust" } });
    assert.equal(shouldRunStep(step, ctx), false);
  });

  it("runs step when only_if os matches (darwin -> macos)", () => {
    const step = makeConditions({ only_if: { os: "macos" } });
    assert.equal(shouldRunStep(step, ctx), true);
  });

  it("skips step when only_if os does not match", () => {
    const step = makeConditions({ only_if: { os: "windows" } });
    assert.equal(shouldRunStep(step, ctx), false);
  });

  it("skips step when skip_if agent matches", () => {
    const step = makeConditions({ skip_if: { agent: "claude-code" } });
    assert.equal(shouldRunStep(step, ctx), false);
  });

  it("runs step when skip_if agent does not match", () => {
    const step = makeConditions({ skip_if: { agent: "opencode" } });
    assert.equal(shouldRunStep(step, ctx), true);
  });

  it("skips step when skip_if language matches", () => {
    const step = makeConditions({ skip_if: { language: "ts" } });
    assert.equal(shouldRunStep(step, ctx), false);
  });

  it("skips step when skip_if os matches", () => {
    const step = makeConditions({ skip_if: { os: "macos" } });
    assert.equal(shouldRunStep(step, ctx), false);
  });

  it("requires ALL only_if conditions to match", () => {
    const step = makeConditions({
      only_if: { agent: "claude-code", language: "rust" },
    });
    assert.equal(shouldRunStep(step, ctx), false);
  });

  it("runs when ALL only_if conditions match", () => {
    const step = makeConditions({
      only_if: { agent: "claude-code", language: "ts" },
    });
    assert.equal(shouldRunStep(step, ctx), true);
  });

  it("only_if is evaluated before skip_if", () => {
    // only_if fails -> step should not run, regardless of skip_if
    const step = makeConditions({
      only_if: { agent: "opencode" },
      skip_if: { language: "rust" },
    });
    assert.equal(shouldRunStep(step, ctx), false);
  });

  it("skip_if evaluated when only_if passes", () => {
    const step = makeConditions({
      only_if: { agent: "claude-code" },
      skip_if: { language: "ts" },
    });
    assert.equal(shouldRunStep(step, ctx), false);
  });

  it("handles win32 platform normalization", () => {
    const winCtx = { agent: "claude-code", language: "ts", os: "win32" };
    const step = makeConditions({ only_if: { os: "windows" } });
    assert.equal(shouldRunStep(step, winCtx), true);
  });

  it("handles linux platform as-is", () => {
    const linuxCtx = { agent: "claude-code", language: "ts", os: "linux" };
    const step = makeConditions({ only_if: { os: "linux" } });
    assert.equal(shouldRunStep(step, linuxCtx), true);
  });
});
