import { test } from "node:test";
import assert from "node:assert/strict";
import { createToolProcessBackend } from "./tool-process-backend.js";

function fixture(overrides = {}) {
  const calls = [];
  const closed = [];
  const resource = (name) => ({
    [Symbol.dispose]() {
      closed.push(name);
    },
  });
  const prepared = {
    ...resource("prepared"),
    input() {
      return {
        commandPath: ["run"],
        input: { canonical: true },
        stdin: true,
        stdout: true,
        stderr: true,
      };
    },
    renderResult(terminal) {
      calls.push(["render", terminal]);
      return { exitCode: 7, stdout: [], stderr: [] };
    },
  };
  const host = {
    prepareCommand(...args) {
      calls.push(["prepare", ...args]);
      return { tag: "invoke", val: prepared };
    },
    createStdinFromBytes(bytes) {
      calls.push(["stdin", [...bytes]]);
      return resource("stdin");
    },
    createCollectedOutput(limit) {
      calls.push(["output", limit]);
      return [resource("target"), resource("collection")];
    },
    ToolRpc: class {
      constructor(name) {
        calls.push(["rpc", name]);
      }
      [Symbol.dispose]() {
        closed.push("rpc");
      }
      asyncInvokeAndAwait(...args) {
        calls.push(["invoke", ...args]);
        return {
          ...resource("future"),
          collect() {
            calls.push(["collect"]);
            return {
              bodyExecuted: true,
              result: { tag: "ok", val: {} },
              stdout: { bytes: [0, 255] },
              stderr: { bytes: [128] },
            };
          },
          cancel() {
            calls.push(["cancel"]);
          },
        };
      }
    },
    ...overrides,
  };
  const process = { env: {}, cwd: () => "/workspace" };
  return {
    calls,
    closed,
    prepared,
    host,
    process,
    backend: createToolProcessBackend(host, process),
  };
}

test("registration is pure and the component compatibility switch is checked per call", () => {
  const f = fixture();
  assert.deepEqual(f.calls, []);
  for (const value of ["0", "false"]) {
    f.process.env.GOLEM_NODE_TOOL_PROCESSES = value;
    assert.equal(f.backend.spawnSync("tsc", [], {}), undefined);
    assert.deepEqual(f.calls, []);
  }
  f.process.env.GOLEM_NODE_TOOL_PROCESSES = "1";
  assert.equal(f.backend.spawnSync("tsc", [], {}).status, 7);
});

test("canonical input, finite binary stdin, distinct channels and status reach the caller", () => {
  const f = fixture();
  const result = f.backend.spawnSync("tsc", ["file.ts"], {
    cwd: "src",
    env: { PWD: "/wrong", DECLARED: "value" },
    input: Uint8Array.of(0, 128, 255),
    maxBuffer: 13,
  });
  assert.equal(result.status, 7);
  assert.deepEqual([...result.stdout], [0, 255]);
  assert.deepEqual([...result.stderr], [128]);
  assert.deepEqual(f.calls[0], [
    "prepare",
    "tsc",
    ["file.ts"],
    [
      ["DECLARED", "value"],
      ["PWD", "/workspace/src"],
    ],
  ]);
  assert.deepEqual(
    f.calls.find((call) => call[0] === "stdin"),
    ["stdin", [0, 128, 255]],
  );
  assert.deepEqual(
    f.calls.filter((call) => call[0] === "output"),
    [
      ["output", 13n],
      ["output", 13n],
    ],
  );
  assert.equal(f.calls.filter((call) => call[0] === "invoke").length, 1);
  assert.equal(
    f.calls.some((call) => call[0] === "cancel"),
    false,
  );
  assert.deepEqual(f.closed, ["future", "rpc", "prepared"]);
});

test("a declared stdin gets EOF when the caller supplies no input", () => {
  const f = fixture();
  f.backend.spawnSync("tsc", [], {});
  assert.deepEqual(
    f.calls.find((call) => call[0] === "stdin"),
    ["stdin", []],
  );
});

test("help and unknown commands do not allocate attachments or dispatch work", () => {
  const f = fixture({
    prepareCommand: () => ({
      tag: "output",
      val: { exitCode: 0, stdout: [104], stderr: [] },
    }),
  });
  const result = f.backend.spawnSync("tsc", ["--help"], {});
  assert.equal(result.stdout.toString(), "h");
  assert.equal(result.status, 0);
  assert.deepEqual(f.calls, []);
  f.host.prepareCommand = () => undefined;
  assert.equal(f.backend.spawnSync("missing", [], {}), undefined);
  assert.deepEqual(f.calls, []);
});

test("admission failure cleans resources and cannot start a child effect", () => {
  const f = fixture({
    createCollectedOutput() {
      throw { tag: "resource-exhausted", val: "pressure" };
    },
  });
  const result = f.backend.spawnSync("tsc", [], {});
  assert.equal(result.status, null);
  assert.equal(result.error.code, "ENOMEM");
  assert.equal(
    f.calls.some((call) => call[0] === "invoke"),
    false,
  );
  assert.deepEqual(f.closed, ["stdin", "prepared"]);
});

test("structured output obeys the same bounded-output contract as streamed output", () => {
  const f = fixture({
    prepareCommand: () => ({
      tag: "output",
      val: { exitCode: 0, stdout: [1, 2, 3], stderr: [4, 5, 6] },
    }),
  });
  const result = f.backend.spawnSync("tsc", ["--help"], { maxBuffer: 2 });
  assert.equal(result.status, null);
  assert.equal(result.error.code, "ENOBUFS");
  assert.deepEqual([...result.stdout], [1, 2]);
  assert.deepEqual([...result.stderr], [4, 5]);
});

test("collection failure cancels accepted work before dropping the observer", () => {
  const f = fixture();
  f.host.ToolRpc.prototype.asyncInvokeAndAwait = () => ({
    collect() {
      throw new Error("collection failed");
    },
    cancel() {
      f.calls.push(["cancel"]);
    },
    [Symbol.dispose]() {
      f.calls.push(["dispose-future"]);
    },
  });
  const result = f.backend.spawnSync("tsc", [], {});
  assert.equal(result.error.code, "EIO");
  assert.equal(result.status, null);
  assert.deepEqual(f.calls.slice(-2), [["cancel"], ["dispose-future"]]);
});

test("a denied spawn is not a started command with a nonzero exit status", () => {
  const f = fixture();
  f.host.ToolRpc.prototype.asyncInvokeAndAwait = () => ({
    collect() {
      return {
        bodyExecuted: false,
        result: {
          tag: "err",
          val: { tag: "denied", val: "permission denied" },
        },
      };
    },
  });
  f.prepared.renderResult = () => ({ exitCode: 3, stdout: [], stderr: [100] });
  const result = f.backend.spawnSync("tsc", [], {});
  assert.equal(result.error.code, "EACCES");
  assert.equal(result.pid, 0);
  assert.equal(result.status, null);
  assert.equal(result.stderr.toString(), "d");
});

test("host output overflow retains the prefix and reports ENOBUFS", () => {
  const f = fixture();
  f.host.ToolRpc.prototype.asyncInvokeAndAwait = () => ({
    collect() {
      return {
        bodyExecuted: true,
        result: { tag: "ok", val: {} },
        stdout: { bytes: [0, 255], overflow: true },
      };
    },
  });
  const result = f.backend.spawnSync("tsc", [], { maxBuffer: 2 });
  assert.equal(result.error.code, "ENOBUFS");
  assert.equal(result.status, null);
  assert.deepEqual([...result.stdout], [0, 255]);
});

test("the recorded execution fact, not the error tag, distinguishes spawn refusal", () => {
  for (const [tag, code] of [
    ["resource-exhausted", "ENOMEM"],
    ["denied", "EACCES"],
  ]) {
    for (const bodyExecuted of [false, true]) {
      const f = fixture();
      f.host.ToolRpc.prototype.asyncInvokeAndAwait = () => ({
        collect() {
          return {
            bodyExecuted,
            result: { tag: "err", val: { tag, val: "failed" } },
            stdout: {
              bytes: bodyExecuted ? [111] : [],
              failure: bodyExecuted
                ? undefined
                : { tag: "failed", val: "refused" },
            },
          };
        },
      });
      f.prepared.renderResult = () => ({
        exitCode: 1,
        stdout: [],
        stderr: [100],
      });
      const result = f.backend.spawnSync("tsc", [], {});
      assert.equal(result.status, bodyExecuted ? 1 : null);
      assert.equal(result.pid, bodyExecuted ? 1 : 0);
      assert.equal(result.error?.code, bodyExecuted ? undefined : code);
      assert.equal(result.stdout.toString(), bodyExecuted ? "o" : "");
      assert.equal(result.stderr.toString(), "d");
    }
  }
});

test("finite input preserves a view slice and the requested string encoding", () => {
  const view = fixture();
  view.backend.spawnSync("tsc", [], {
    input: new DataView(Uint8Array.of(1, 2, 3, 4).buffer, 1, 2),
  });
  assert.deepEqual(
    view.calls.find((call) => call[0] === "stdin"),
    ["stdin", [2, 3]],
  );
  const text = fixture();
  text.backend.spawnSync("tsc", [], { input: "ff00", encoding: "hex" });
  assert.deepEqual(
    text.calls.find((call) => call[0] === "stdin"),
    ["stdin", [255, 0]],
  );
});

test("invalid input is rejected before tool discovery or admission", () => {
  const f = fixture();
  assert.throws(() => f.backend.spawnSync("tsc", [], { input: 7 }), TypeError);
  assert.deepEqual(f.calls, []);
});
