import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { substituteVariables } from "../src/executor.js";

describe("substituteVariables", () => {
  it("substitutes a single variable", () => {
    const result = substituteVariables("Hello {{name}}!", { name: "world" });
    assert.equal(result, "Hello world!");
  });

  it("substitutes multiple variables", () => {
    const result = substituteVariables(
      "{{agent}} running {{language}} in {{workspace}}",
      { agent: "claude-code", language: "ts", workspace: "/tmp/work" },
    );
    assert.equal(result, "claude-code running ts in /tmp/work");
  });

  it("leaves unknown variables as-is", () => {
    const result = substituteVariables("{{known}} and {{unknown}}", {
      known: "yes",
    });
    assert.equal(result, "yes and {{unknown}}");
  });

  it("returns plain strings unchanged", () => {
    const result = substituteVariables("no variables here", { foo: "bar" });
    assert.equal(result, "no variables here");
  });

  it("handles empty variables map", () => {
    const result = substituteVariables("{{a}} {{b}}", {});
    assert.equal(result, "{{a}} {{b}}");
  });

  it("handles empty string", () => {
    const result = substituteVariables("", { a: "b" });
    assert.equal(result, "");
  });

  it("substitutes the same variable multiple times", () => {
    const result = substituteVariables("{{x}} and {{x}}", { x: "val" });
    assert.equal(result, "val and val");
  });
});
