import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { classifyFailure } from "../src/failure-classification.js";

describe("classifyFailure", () => {
  it("classifies SKILL_NOT_ACTIVATED as agent", () => {
    const result = classifyFailure(
      'SKILL_NOT_ACTIVATED: expected "golem-new-project" but activated []',
    );
    assert.equal(result.code, "SKILL_NOT_ACTIVATED");
    assert.equal(result.category, "agent");
    assert.ok(result.guidance.length > 0);
  });

  it("classifies SKILL_MISMATCH as agent", () => {
    const result = classifyFailure("SKILL_MISMATCH: unexpected extra skills [foo]");
    assert.equal(result.code, "SKILL_MISMATCH");
    assert.equal(result.category, "agent");
  });

  it("classifies BUILD_FAILED as build", () => {
    const result = classifyFailure("BUILD_FAILED: exit code 1");
    assert.equal(result.code, "BUILD_FAILED");
    assert.equal(result.category, "build");
  });

  it("classifies DEPLOY_FAILED as deploy", () => {
    const result = classifyFailure("DEPLOY_FAILED: connection refused");
    assert.equal(result.code, "DEPLOY_FAILED");
    assert.equal(result.category, "deploy");
  });

  it("classifies INVOKE_FAILED as deploy", () => {
    const result = classifyFailure("INVOKE_FAILED: function not found");
    assert.equal(result.code, "INVOKE_FAILED");
    assert.equal(result.category, "deploy");
  });

  it("classifies SHELL_FAILED as infra", () => {
    const result = classifyFailure("SHELL_FAILED: command not found");
    assert.equal(result.code, "SHELL_FAILED");
    assert.equal(result.category, "infra");
  });

  it("classifies HTTP_FAILED as network", () => {
    const result = classifyFailure("HTTP_FAILED: 503 Service Unavailable");
    assert.equal(result.code, "HTTP_FAILED");
    assert.equal(result.category, "network");
  });

  it("classifies CREATE_AGENT_FAILED as infra", () => {
    const result = classifyFailure("CREATE_AGENT_FAILED: timeout");
    assert.equal(result.code, "CREATE_AGENT_FAILED");
    assert.equal(result.category, "infra");
  });

  it("classifies DELETE_AGENT_FAILED as infra", () => {
    const result = classifyFailure("DELETE_AGENT_FAILED: not found");
    assert.equal(result.code, "DELETE_AGENT_FAILED");
    assert.equal(result.category, "infra");
  });

  it("classifies ASSERTION_FAILED as assertion", () => {
    const result = classifyFailure(
      "ASSERTION_FAILED (stdout_contains): stdout does not contain expected",
    );
    assert.equal(result.code, "ASSERTION_FAILED");
    assert.equal(result.category, "assertion");
  });

  it("classifies Agent failed as agent", () => {
    const result = classifyFailure("Agent failed: timeout exceeded");
    assert.equal(result.code, "AGENT_FAILED");
    assert.equal(result.category, "agent");
  });

  it("falls back to unknown for unrecognized errors", () => {
    const result = classifyFailure("Something completely unexpected happened");
    assert.equal(result.code, "UNKNOWN");
    assert.equal(result.category, "unknown");
    assert.ok(result.guidance.length > 0);
  });

  it("matches prefix correctly (not substring)", () => {
    // Should match BUILD_FAILED, not something that contains it mid-string
    const result = classifyFailure("BUILD_FAILED: compiler error xyz");
    assert.equal(result.code, "BUILD_FAILED");
    assert.equal(result.category, "build");
  });
});
