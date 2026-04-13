import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { evaluate, type AssertionContext, type ExpectSpec } from "../src/assertions.js";

function makeContext(overrides: Partial<AssertionContext> = {}): AssertionContext {
  return {
    stdout: "",
    stderr: "",
    exitCode: 0,
    ...overrides,
  };
}

describe("Assertion Engine", () => {
  describe("exit_code", () => {
    it("passes when exit code matches", () => {
      const results = evaluate(makeContext({ exitCode: 0 }), { exit_code: 0 });
      assert.equal(results.length, 1);
      assert.equal(results[0].passed, true);
    });

    it("fails when exit code does not match", () => {
      const results = evaluate(makeContext({ exitCode: 1 }), { exit_code: 0 });
      assert.equal(results[0].passed, false);
      assert.ok(results[0].message.includes("expected exit code 0"));
    });
  });

  describe("stdout_contains", () => {
    it("passes when stdout contains the string", () => {
      const results = evaluate(makeContext({ stdout: "hello world" }), {
        stdout_contains: "world",
      });
      assert.equal(results[0].passed, true);
    });

    it("fails when stdout does not contain the string", () => {
      const results = evaluate(makeContext({ stdout: "hello" }), {
        stdout_contains: "world",
      });
      assert.equal(results[0].passed, false);
    });
  });

  describe("stdout_not_contains", () => {
    it("passes when stdout does not contain the string", () => {
      const results = evaluate(makeContext({ stdout: "hello" }), {
        stdout_not_contains: "world",
      });
      assert.equal(results[0].passed, true);
    });

    it("fails when stdout contains the string", () => {
      const results = evaluate(makeContext({ stdout: "hello world" }), {
        stdout_not_contains: "world",
      });
      assert.equal(results[0].passed, false);
    });
  });

  describe("stdout_matches", () => {
    it("passes when stdout matches regex", () => {
      const results = evaluate(makeContext({ stdout: "version 1.2.3" }), {
        stdout_matches: "version \\d+\\.\\d+\\.\\d+",
      });
      assert.equal(results[0].passed, true);
    });

    it("fails when stdout does not match regex", () => {
      const results = evaluate(makeContext({ stdout: "no version" }), {
        stdout_matches: "version \\d+\\.\\d+\\.\\d+",
      });
      assert.equal(results[0].passed, false);
    });
  });

  describe("status", () => {
    it("passes when status matches", () => {
      const results = evaluate(makeContext({ status: 200 }), { status: 200 });
      assert.equal(results[0].passed, true);
    });

    it("fails when status does not match", () => {
      const results = evaluate(makeContext({ status: 404 }), { status: 200 });
      assert.equal(results[0].passed, false);
    });
  });

  describe("body_contains", () => {
    it("passes when body contains the string", () => {
      const results = evaluate(makeContext({ body: '{"key":"value"}' }), {
        body_contains: "key",
      });
      assert.equal(results[0].passed, true);
    });

    it("fails when body does not contain the string", () => {
      const results = evaluate(makeContext({ body: '{"key":"value"}' }), {
        body_contains: "missing",
      });
      assert.equal(results[0].passed, false);
      assert.ok(results[0].message.includes('received "{\\"key\\":\\"value\\"}"'));
    });

    it("handles missing body gracefully", () => {
      const results = evaluate(makeContext(), { body_contains: "test" });
      assert.equal(results[0].passed, false);
    });
  });

  describe("body_matches", () => {
    it("passes when body matches regex", () => {
      const results = evaluate(makeContext({ body: "status: ok" }), {
        body_matches: "status:\\s+ok",
      });
      assert.equal(results[0].passed, true);
    });

    it("fails when body does not match regex", () => {
      const results = evaluate(makeContext({ body: "status: error" }), {
        body_matches: "status:\\s+ok",
      });
      assert.equal(results[0].passed, false);
      assert.ok(results[0].message.includes('received "status: error"'));
    });
  });

  describe("result_json", () => {
    it("passes when json path equals expected value", () => {
      const results = evaluate(makeContext({ resultJson: { name: "test", version: 1 } }), {
        result_json: [{ path: "$.name", equals: "test" }],
      });
      assert.equal(results[0].passed, true);
    });

    it("fails when json path does not equal expected value", () => {
      const results = evaluate(makeContext({ resultJson: { name: "other" } }), {
        result_json: [{ path: "$.name", equals: "test" }],
      });
      assert.equal(results[0].passed, false);
      assert.ok(results[0].message.includes('result_json={"name":"other"}'));
    });

    it("passes when json path value contains string", () => {
      const results = evaluate(makeContext({ resultJson: { message: "hello world" } }), {
        result_json: [{ path: "$.message", contains: "world" }],
      });
      assert.equal(results[0].passed, true);
    });

    it("fails when json path value does not contain string", () => {
      const results = evaluate(makeContext({ resultJson: { message: "hello" } }), {
        result_json: [{ path: "$.message", contains: "world" }],
      });
      assert.equal(results[0].passed, false);
      assert.ok(results[0].message.includes('result_json={"message":"hello"}'));
    });

    it("handles missing path gracefully", () => {
      const results = evaluate(makeContext({ resultJson: {} }), {
        result_json: [{ path: "$.missing", equals: "test" }],
      });
      assert.equal(results[0].passed, false);
    });

    it("handles missing parsed json gracefully", () => {
      const results = evaluate(makeContext({ resultJson: undefined }), {
        result_json: [{ path: "$", equals: 1 }],
      });
      assert.equal(results[0].passed, false);
      assert.ok(results[0].message.includes("expected 1"));
    });
  });

  describe("multiple assertions", () => {
    it("evaluates all assertions", () => {
      const expect: ExpectSpec = {
        exit_code: 0,
        stdout_contains: "success",
        stdout_not_contains: "error",
      };
      const results = evaluate(makeContext({ exitCode: 0, stdout: "operation success" }), expect);
      assert.equal(results.length, 3);
      assert.ok(results.every((r) => r.passed));
    });

    it("reports mixed pass/fail", () => {
      const results = evaluate(makeContext({ exitCode: 1, stdout: "success" }), {
        exit_code: 0,
        stdout_contains: "success",
      });
      assert.equal(results.length, 2);
      assert.equal(results[0].passed, false); // exit_code
      assert.equal(results[1].passed, true); // stdout_contains
    });
  });

  describe("empty expect", () => {
    it("returns no results for empty expect", () => {
      const results = evaluate(makeContext(), {});
      assert.equal(results.length, 0);
    });
  });
});
