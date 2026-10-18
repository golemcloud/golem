import { test } from "node:test";
import assert from "node:assert/strict";
import { Readable, PassThrough } from "node:stream";
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
    createDiscardedOutput() {
      calls.push(["discard"]);
      return resource("discard-target");
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
          collect(...args) {
            calls.push(["collect", ...args]);
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

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

test("synchronous inherited output failure preserves a started command and the other channel", () => {
  const f = fixture();
  const stderr = [];
  f.process.stdout = {
    write() {
      throw new Error("output failed");
    },
  };
  f.process.stderr = {
    write(bytes) {
      stderr.push(...bytes);
    },
  };
  const result = f.backend.spawnSync("tsc", [], {
    stdio: ["ignore", "inherit", "inherit"],
  });
  assert.equal(result.error.code, "EIO");
  assert.equal(result.error.path, "tsc");
  assert.equal(result.error.cause.message, "output failed");
  assert.equal(result.pid, 1);
  assert.equal(result.status, null);
  assert.deepEqual(stderr, [128]);
  assert.equal(f.calls.filter(([name]) => name === "invoke").length, 1);
});

function bashFixture(
  result = {
    stdout: "shell output\n",
    stderr: "diagnostic\n",
    exit_code: 7,
    cwd: "/workspace",
  },
) {
  const f = fixture();
  f.prepared.input = () => ({
    commandPath: ["run"],
    input: { canonical: true },
    stdin: false,
    stdout: false,
    stderr: false,
  });
  f.prepared.renderResult = () => ({
    exitCode: 0,
    stdout: Buffer.from(JSON.stringify(result) + "\n"),
    stderr: [],
  });
  f.host.ToolRpc.prototype.asyncInvokeAndAwait = (...args) => {
    f.calls.push(["invoke", ...args]);
    return {
      collect: () => ({ bodyExecuted: true, result: { tag: "ok", val: {} } }),
      [Symbol.dispose]() {
        f.closed.push("future");
      },
    };
  };
  return f;
}

test("shell execution uses bound Bash's canonical CLI and projects both channels and status", () => {
  const f = bashFixture();
  const result = f.backend.spawnSync("printf", ["'%s'", "'a b'", "; exit 7"], {
    shell: true,
    cwd: "src",
  });
  assert.deepEqual(f.calls[0].slice(0, 4), [
    "prepare",
    "bash",
    ["run", "--cwd", "/workspace/src", "--", "printf '%s' 'a b' ; exit 7"],
    [["PWD", "/workspace/src"]],
  ]);
  assert.deepEqual(
    f.calls.find(([name]) => name === "rpc"),
    ["rpc", "bash"],
  );
  assert.equal(result.status, 7);
  assert.equal(result.stdout.toString(), "shell output\n");
  assert.equal(result.stderr.toString(), "diagnostic\n");
  assert.equal(result.error, undefined);
  assert.equal(
    f.calls.some(([name]) => name === "output" || name === "stdin"),
    false,
  );
});

test("conventional shell argv is quoted and its grammar stays in Bash", () => {
  const f = bashFixture();
  f.backend.spawnSync(
    "sh",
    ["-c", "printf '%s' \"$1\"", "label", "a'b;$(not-run)"],
    {},
  );
  assert.equal(f.calls[0][1], "bash");
  assert.equal(
    f.calls[0][2].at(-1),
    String.raw`'sh' '-c' 'printf '\''%s'\'' "$1"' 'label' 'a'\''b;$(not-run)'`,
  );
});

test("an explicitly selected shell is a quoted Bash command, never a host subprocess", () => {
  const f = bashFixture();
  f.backend.spawnSync("echo", ["'it is fine'"], { shell: "/bin/sh" });
  assert.equal(
    f.calls[0][2].at(-1),
    String.raw`'/bin/sh' -c 'echo '\''it is fine'\'''`,
  );
});

test("Bash's explicit tool CLI is not mistaken for a shell command", () => {
  const f = fixture();
  f.backend.spawnSync("bash", ["run", "--", "true"], {});
  assert.deepEqual(f.calls[0].slice(1, 3), ["bash", ["run", "--", "true"]]);
});

test("shell routing preserves per-component disabling and unavailable bindings", () => {
  const f = bashFixture();
  f.process.env.GOLEM_NODE_TOOL_PROCESSES = "0";
  assert.equal(f.backend.spawnSync("echo ok", [], { shell: true }), undefined);
  assert.deepEqual(f.calls, []);
  delete f.process.env.GOLEM_NODE_TOOL_PROCESSES;
  f.host.prepareCommand = (name) => {
    assert.equal(name, "bash");
    return undefined;
  };
  assert.equal(f.backend.spawnSync("echo ok", [], { shell: true }), undefined);
  assert.deepEqual(f.calls, []);
});

test("malformed Bash results are command failures and do not print their payloads", () => {
  for (const result of [
    null,
    { stdout: "private", stderr: "", cwd: "/", exitCode: 7 },
    { stdout: "private", stderr: "", cwd: "/", exit_code: 256 },
    { stdout: { secret: "private" }, stderr: "", cwd: "/", exit_code: 0 },
    { stdout: "private", stderr: "", cwd: "/", exit_code: 0, extra: "private" },
  ]) {
    const f = bashFixture(result);
    const output = f.backend.spawnSync("echo ok", [], { shell: true });
    assert.equal(output.status, 1);
    assert.equal(output.error, undefined);
    assert.equal(output.stdout.length, 0);
    assert.equal(output.stderr.toString(), "bash: invalid shell result\n");
  }
});

test("Bash output obeys independent byte limits, including UTF-8 and nonzero status", () => {
  const f = bashFixture({
    stdout: "éé",
    stderr: "12345",
    cwd: "/",
    exit_code: 9,
  });
  const output = f.backend.spawnSync("echo ok", [], {
    shell: true,
    maxBuffer: 3,
  });
  assert.equal(output.status, null);
  assert.equal(output.error.code, "ENOBUFS");
  assert.deepEqual([...output.stdout], [195, 169, 195]);
  assert.equal(output.stderr.toString(), "123");
});

test("host projection and tool admission failures keep their original shell outcome", () => {
  const f = bashFixture();
  f.prepared.renderResult = () => ({
    exitCode: 1,
    stdout: [],
    stderr: Buffer.from("invalid typed result\n"),
  });
  let output = f.backend.spawnSync("echo ok", [], { shell: true });
  assert.equal(output.status, 1);
  assert.equal(output.stderr.toString(), "invalid typed result\n");
  f.host.ToolRpc.prototype.asyncInvokeAndAwait = () => ({
    collect: () => ({
      bodyExecuted: false,
      result: { tag: "err", val: { tag: "denied" } },
    }),
  });
  f.prepared.renderResult = () => ({
    exitCode: 3,
    stdout: [],
    stderr: Buffer.from("denied\n"),
  });
  output = f.backend.spawnSync("echo ok", [], { shell: true });
  assert.equal(output.status, null);
  assert.equal(output.error.code, "EACCES");
  assert.equal(output.error.path, "echo ok");
  assert.equal(output.stderr.toString(), "denied\n");
});

test("invalid shell options cannot discover or dispatch tools", () => {
  const f = fixture();
  assert.throws(() => f.backend.spawnSync("echo", [], { shell: 7 }), TypeError);
  assert.throws(() => f.backend.spawn("echo", [], { shell: {} }), TypeError);
  assert.deepEqual(f.calls, []);
});

function asyncFixture({
  admitted = true,
  completeOnEof = false,
  blockedWrite = false,
} = {}) {
  const f = fixture();
  const completion = deferred();
  const consumerClosed = deferred();
  const writeReady = deferred();
  const bytes = [];
  let writing = false;
  let watching = false;
  let ended = false;
  let cancelled = false;
  let finished = false;
  const resource = (name, release = () => {}) => {
    let disposed = false;
    return {
      [Symbol.dispose]() {
        assert.equal(disposed, false, `${name} disposed twice`);
        assert.equal(
          writing || watching,
          false,
          `${name} disposed during borrowed I/O`,
        );
        disposed = true;
        f.closed.push(name);
        release();
      },
    };
  };
  const complete = (value = {}) => {
    finished = true;
    writeReady.resolve();
    consumerClosed.resolve();
    completion.resolve({
      bodyExecuted: true,
      result: { tag: "ok", val: {} },
      stdout: { bytes: [0, 255] },
      stderr: { bytes: [128] },
      ...value,
    });
  };
  const writer = {
    ...resource("writer"),
    async write(chunk) {
      assert.equal(writing, false);
      writing = true;
      f.calls.push(["write", [...chunk]]);
      try {
        if (blockedWrite) await writeReady.promise;
        if (finished) throw { tag: "closed" };
        bytes.push(...chunk);
      } finally {
        writing = false;
      }
    },
    async finish() {
      f.calls.push(["finish"]);
      ended = true;
      if (completeOnEof) complete();
    },
    async fail(error) {
      f.calls.push(["fail", error]);
      consumerClosed.resolve();
    },
  };
  // An unused source must close its consumer watcher before writer cleanup.
  const source = {
    [Symbol.dispose]() {
      f.closed.push("source");
      consumerClosed.resolve();
    },
  };
  const closed = {
    ...resource("closed"),
    async wait() {
      watching = true;
      try {
        await consumerClosed.promise;
      } finally {
        watching = false;
      }
    },
  };
  f.host.createStdin = () => {
    f.calls.push(["create-stdin"]);
    return [writer, source, closed];
  };
  const future = {
    ...resource("future"),
    admitted() {
      return admitted;
    },
    collect() {
      assert.fail("async spawn called the synchronous collector");
    },
    collectAsync(...args) {
      f.calls.push(["collect-async", ...args]);
      return completion.promise;
    },
    cancel() {
      f.calls.push(["cancel"]);
      cancelled = true;
      complete({
        bodyExecuted: false,
        result: { tag: "err", val: { tag: "cancelled" } },
        stdout: { bytes: [], failure: { tag: "cancelled" } },
        stderr: { bytes: [], failure: { tag: "cancelled" } },
      });
    },
  };
  f.host.ToolRpc.prototype.asyncInvokeAndAwait = (...args) => {
    f.calls.push(["invoke", ...args]);
    return future;
  };
  return {
    ...f,
    future,
    complete,
    bytes,
    consumerClosed,
    get ended() {
      return ended;
    },
    get cancelled() {
      return cancelled;
    },
  };
}

function observe(child) {
  const events = [];
  const errors = [];
  const stdout = [];
  const stderr = [];
  child.stdout?.on("data", (chunk) => stdout.push(...chunk));
  child.stderr?.on("data", (chunk) => stderr.push(...chunk));
  child.stdout?.on("end", () => events.push(["stdout-end"]));
  child.stderr?.on("end", () => events.push(["stderr-end"]));
  child.on("spawn", () => events.push(["spawn"]));
  child.on("error", (error) => {
    errors.push(error);
    events.push(["error"]);
  });
  child.on("exit", (...args) => events.push(["exit", ...args]));
  const closed = new Promise((resolve) =>
    child.once("close", (...args) => {
      events.push(["close", ...args]);
      resolve();
    }),
  );
  return { events, errors, stdout, stderr, closed };
}

test("ignored stdio supplies EOF and discard targets, never zero-limit collectors", () => {
  const f = fixture();
  const result = f.backend.spawnSync("tsc", [], {
    stdio: "ignore",
    input: "unused",
    maxBuffer: 0,
  });
  assert.equal(result.status, 7);
  assert.equal(result.error, undefined);
  assert.equal(result.stdout, null);
  assert.equal(result.stderr, null);
  assert.deepEqual(
    f.calls.find(([name]) => name === "stdin"),
    ["stdin", []],
  );
  assert.equal(f.calls.filter(([name]) => name === "discard").length, 2);
  assert.equal(
    f.calls.some(([name]) => name === "output" || name === "cancel"),
    false,
  );
  assert.deepEqual(
    f.calls.find(([name]) => name === "collect"),
    ["collect", undefined, undefined],
  );
  assert.deepEqual(f.closed, ["future", "rpc", "prepared"]);
});

test("mixed stdio collects only the piped channel", () => {
  const f = fixture();
  const result = f.backend.spawnSync("tsc", [], {
    stdio: ["ignore", "pipe", "ignore"],
    maxBuffer: 2,
  });
  assert.equal(result.status, 7);
  assert.equal(result.error, undefined);
  assert.deepEqual([...result.stdout], [0, 255]);
  assert.equal(result.stderr, null);
  assert.equal(f.calls.filter(([name]) => name === "output").length, 1);
  assert.equal(f.calls.filter(([name]) => name === "discard").length, 1);
  assert.equal(f.calls.find(([name]) => name === "collect")[2], undefined);
});

test("ignored structured Bash output does not overflow or print JSON", () => {
  const f = bashFixture();
  const result = f.backend.spawnSync("echo ok", [], {
    shell: true,
    stdio: "ignore",
    maxBuffer: 0,
  });
  assert.equal(result.status, 7);
  assert.equal(result.error, undefined);
  assert.equal(result.stdout, null);
  assert.equal(result.stderr, null);
});

test("synchronous inherited output uses the caller's current streams and is not returned", () => {
  const f = fixture();
  const stdout = [];
  const stderr = [];
  f.process.stdout = { write: (bytes) => stdout.push(...bytes) };
  f.process.stderr = { write: (bytes) => stderr.push(...bytes) };
  const result = f.backend.spawnSync("tsc", [], { stdio: ["ignore", 1, 2] });
  assert.equal(result.status, 7);
  assert.equal(result.error, undefined);
  assert.equal(result.stdout, null);
  assert.equal(result.stderr, null);
  assert.deepEqual(stdout, [0, 255]);
  assert.deepEqual(stderr, [128]);
});

test("a command with no stdin can use synchronous inherited stdio", () => {
  const f = bashFixture();
  const stdout = [];
  const stderr = [];
  f.process.stdout = { write: (bytes) => stdout.push(...bytes) };
  f.process.stderr = { write: (bytes) => stderr.push(...bytes) };
  const result = f.backend.spawnSync("echo ok", [], {
    shell: true,
    stdio: "inherit",
  });
  assert.equal(result.status, 7);
  assert.equal(Buffer.from(stdout).toString(), "shell output\n");
  assert.equal(Buffer.from(stderr).toString(), "diagnostic\n");
  assert.equal(result.stdout, null);
});

test("synchronous inherited stdin is explicitly rejected before dispatch when declared", () => {
  const f = fixture();
  const result = f.backend.spawnSync("tsc", [], {
    stdio: ["inherit", "pipe", "pipe"],
  });
  assert.equal(result.error.code, "ENOTSUP");
  assert.equal(result.status, null);
  assert.equal(
    f.calls.some(([name]) => name === "invoke" || name === "stdin"),
    false,
  );
  assert.deepEqual(f.closed, ["prepared"]);
});

test("invalid stdio fails before discovery", () => {
  const f = fixture();
  for (const stdio of [
    false,
    {},
    "ipc",
    ["pipe", 0, "pipe"],
    ["pipe", "pipe", "pipe", "pipe"],
    "inherit",
  ]) {
    assert.throws(() => f.backend.spawnSync("tsc", [], { stdio }), TypeError);
    assert.throws(() => f.backend.spawn("tsc", [], { stdio }), TypeError);
    assert.deepEqual(f.calls, []);
  }
});

test(
  "async ignored streams have no exposed child endpoints or collected prefixes",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    const child = f.backend.spawn("tsc", [], { stdio: "ignore", maxBuffer: 0 });
    const observed = observe(child);
    assert.deepEqual(child.stdio, [null, null, null]);
    assert.equal(
      f.calls.some(([name]) => name === "create-stdin" || name === "output"),
      false,
    );
    assert.equal(f.calls.filter(([name]) => name === "discard").length, 2);
    f.complete();
    await observed.closed;
    assert.equal(child.exitCode, 7);
    assert.deepEqual(observed.errors, []);
    assert.equal(f.cancelled, false);
  },
);

test(
  "async inherited output flushes before close and preserves the other piped channel",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture({ completeOnEof: true });
    const stdout = [];
    let delivered = false;
    f.process.stdout = {
      write(bytes, callback) {
        setTimeout(() => {
          stdout.push(...bytes);
          delivered = true;
          callback();
        }, 5);
      },
    };
    const child = f.backend.spawn("tsc", [], {
      stdio: ["pipe", "inherit", "pipe"],
    });
    const observed = observe(child);
    child.stdin.end();
    await observed.closed;
    assert.equal(delivered, true);
    assert.equal(child.stdout, null);
    assert.deepEqual(stdout, [0, 255]);
    assert.deepEqual(observed.stderr, [128]);
    assert.deepEqual(observed.errors, []);
  },
);

test(
  "async inherited input forwards binary bytes, ends stdin and releases the parent pipe",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture({ completeOnEof: true });
    f.process.stdin = Readable.from([
      Buffer.from([0, 255]),
      Buffer.from([128]),
    ]);
    const child = f.backend.spawn("tsc", [], {
      stdio: ["inherit", "pipe", "pipe"],
    });
    const observed = observe(child);
    assert.equal(child.stdin, null);
    await observed.closed;
    assert.deepEqual(f.bytes, [0, 255, 128]);
    assert.equal(f.ended, true);
    assert.equal(f.process.stdin.listenerCount("error"), 0);
    assert.deepEqual(observed.errors, []);
  },
);

test(
  "cancelling async inherited input joins a blocked write and unpipes without destroying the parent",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture({ blockedWrite: true });
    f.process.stdin = new PassThrough();
    const child = f.backend.spawn("tsc", [], {
      stdio: ["inherit", "pipe", "pipe"],
    });
    const observed = observe(child);
    f.process.stdin.write(Buffer.of(1));
    await Promise.resolve();
    child.kill();
    await observed.closed;
    assert.equal(f.process.stdin.destroyed, false);
    assert.equal(f.process.stdin.listenerCount("error"), 0);
    assert.equal(f.process.stdin.readableFlowing, false);
    assert.deepEqual(observed.errors, []);
    assert.equal(child.signalCode, "SIGTERM");
    f.process.stdin.destroy();
  },
);

test(
  "inherited output failure still joins the other channel before close",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    let stderrDone = false;
    f.process.stdout = {
      write(_bytes, callback) {
        callback(new Error("stdout failed"));
      },
    };
    f.process.stderr = {
      write(_bytes, callback) {
        setTimeout(() => {
          stderrDone = true;
          callback();
        }, 5);
      },
    };
    const child = f.backend.spawn("tsc", [], {
      stdio: ["pipe", "inherit", "inherit"],
    });
    const observed = observe(child);
    f.complete();
    await observed.closed;
    assert.equal(stderrDone, true);
    assert.equal(observed.errors[0].code, "EIO");
    assert.equal(observed.errors[0].cause.message, "stdout failed");
    assert.equal(child.exitCode, null);
  },
);

test(
  "a broken abort-listener cleanup cannot strand an unused stdin source",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    f.host.createCollectedOutput = () => {
      throw { tag: "resource-exhausted" };
    };
    const signal = {
      aborted: false,
      addEventListener() {},
      removeEventListener() {
        throw new Error("cleanup failed");
      },
    };
    const child = f.backend.spawn("tsc", [], { signal });
    const observed = observe(child);
    await observed.closed;
    assert.equal(observed.errors[0].code, "ENOMEM");
    assert.equal(
      f.calls.some(([name]) => name === "invoke"),
      false,
    );
    assert.deepEqual(f.closed, ["source", "closed", "writer", "prepared"]);
  },
);

test(
  "async shell execution uses the same Bash result projection",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    f.prepared.input = () => ({
      commandPath: ["run"],
      input: { canonical: true },
      stdin: false,
      stdout: false,
      stderr: false,
    });
    f.prepared.renderResult = () => ({
      exitCode: 0,
      stdout: Buffer.from(
        JSON.stringify({
          stdout: "async shell\n",
          stderr: "shell diagnostic\n",
          exit_code: 9,
          cwd: "/workspace/src",
        }),
      ),
      stderr: [],
    });
    const child = f.backend.spawn("echo async; exit 9", [], {
      shell: true,
      cwd: "src",
    });
    const observed = observe(child);
    f.complete({ stdout: undefined, stderr: undefined });
    await observed.closed;
    assert.deepEqual(f.calls[0].slice(0, 3), [
      "prepare",
      "bash",
      ["run", "--cwd", "/workspace/src", "--", "echo async; exit 9"],
    ]);
    assert.deepEqual(
      f.calls.find(([name]) => name === "rpc"),
      ["rpc", "bash"],
    );
    assert.equal(child.stdin, null);
    assert.deepEqual(observed.events[0], ["spawn"]);
    assert.deepEqual(observed.events.at(-1), ["close", 9, null]);
    assert.equal(Buffer.from(observed.stdout).toString(), "async shell\n");
    assert.equal(Buffer.from(observed.stderr).toString(), "shell diagnostic\n");
    assert.deepEqual(observed.errors, []);
  },
);

test(
  "async startup returns before completion and stdin can wait for spawn",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture({ completeOnEof: true });
    const child = f.backend.spawn("tsc", ["file.ts"], { maxBuffer: 13 });
    const observed = observe(child);
    assert.equal(f.ended, false);
    assert.deepEqual(f.bytes, []);
    child.once("spawn", () => child.stdin.end(Buffer.from([0, 128, 255])));
    await observed.closed;
    assert.deepEqual(f.bytes, [0, 128, 255]);
    assert.equal(f.ended, true);
    assert.deepEqual(observed.stdout, [0, 255]);
    assert.deepEqual(observed.stderr, [128]);
    assert.deepEqual(observed.errors, []);
    assert.deepEqual(observed.events[0], ["spawn"]);
    assert.deepEqual(observed.events.at(-1), ["close", 7, null]);
    assert.ok(
      observed.events.findIndex(([name]) => name === "stdout-end") <
        observed.events.length - 1,
    );
    assert.equal(child.exitCode, 7);
    assert.equal(child.kill(0), false);
    assert.deepEqual(f.closed, [
      "future",
      "rpc",
      "closed",
      "writer",
      "prepared",
    ]);
  },
);

test(
  "async allocation failure has no dispatch or spawn and closes unused stdin",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    f.host.createCollectedOutput = () => {
      throw { tag: "resource-exhausted" };
    };
    const child = f.backend.spawn("tsc", []);
    const observed = observe(child);
    await observed.closed;
    assert.equal(observed.errors[0].code, "ENOMEM");
    assert.equal(
      f.calls.some(([name]) => name === "invoke"),
      false,
    );
    assert.equal(
      observed.events.some(([name]) => name === "spawn"),
      false,
    );
    assert.equal(child.pid, undefined);
    assert.equal(child.kill(), false);
    assert.deepEqual(f.closed, ["source", "closed", "writer", "prepared"]);
  },
);

test(
  "an immutable dispatch refusal never emits spawn or accepts kill",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture({ admitted: false });
    const child = f.backend.spawn("tsc", []);
    const observed = observe(child);
    assert.equal(child.kill(0), false);
    assert.equal(child.kill(), false);
    f.complete({
      bodyExecuted: false,
      result: { tag: "err", val: { tag: "denied" } },
    });
    await observed.closed;
    assert.equal(observed.errors[0].code, "EACCES");
    assert.equal(
      observed.events.some(([name]) => name === "spawn"),
      false,
    );
    assert.equal(f.cancelled, false);
  },
);

test(
  "async cancellation joins a blocked write before disposing borrowed resources",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture({ blockedWrite: true });
    const child = f.backend.spawn("tsc", []);
    const observed = observe(child);
    const written = new Promise((resolve) =>
      child.stdin.write(Buffer.of(1), (error) => resolve(error)),
    );
    await Promise.resolve();
    assert.deepEqual(f.closed, []);
    assert.equal(child.kill(0), true);
    assert.equal(child.kill(15), true);
    assert.equal(child.kill(), true);
    await observed.closed;
    assert.equal((await written).code, "EPIPE");
    assert.equal(f.calls.filter(([name]) => name === "cancel").length, 1);
    assert.deepEqual(observed.errors, []);
    assert.deepEqual(observed.events.at(-1), ["close", null, "SIGTERM"]);
    assert.equal(child.killed, true);
  },
);

test(
  "async timeout and abort cancel once and a later invocation succeeds",
  { timeout: 2000 },
  async () => {
    for (const mode of ["timeout", "abort"]) {
      const f = asyncFixture();
      const controller = new AbortController();
      const options =
        mode === "timeout"
          ? { timeout: 5, killSignal: 2 }
          : { signal: controller.signal };
      const child = f.backend.spawn("tsc", [], options);
      const observed = observe(child);
      if (mode === "abort") child.once("spawn", () => controller.abort("test"));
      await observed.closed;
      assert.equal(f.calls.filter(([name]) => name === "cancel").length, 1);
      assert.equal(child.signalCode, mode === "timeout" ? "SIGINT" : "SIGTERM");
      assert.equal(
        observed.errors[0]?.code,
        mode === "abort" ? "ABORT_ERR" : undefined,
      );
      const next = asyncFixture({ completeOnEof: true });
      const success = next.backend.spawn("tsc", []);
      const successObserved = observe(success);
      success.stdin.end();
      await successObserved.closed;
      assert.deepEqual(successObserved.errors, []);
      assert.equal(success.exitCode, 7);
    }
  },
);

test("invalid async options throw before discovery or effect admission", () => {
  const f = asyncFixture();
  for (const options of [
    { signal: {} },
    { signal: null },
    { timeout: -1 },
    { maxBuffer: -1 },
    { killSignal: "invalid" },
  ]) {
    assert.throws(() => f.backend.spawn("tsc", [], options));
    assert.deepEqual(f.calls, []);
  }
});

test(
  "an already aborted signal cannot allocate streams or dispatch an effect",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    const controller = new AbortController();
    controller.abort("before dispatch");
    const child = f.backend.spawn("tsc", [], { signal: controller.signal });
    const observed = observe(child);
    await observed.closed;
    assert.equal(observed.errors[0].code, "ABORT_ERR");
    assert.equal(
      f.calls.some(([name]) => name === "invoke" || name === "create-stdin"),
      false,
    );
    assert.deepEqual(f.closed, ["prepared"]);
  },
);

test(
  "a failed stdin watcher cancels collection instead of hanging",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture({ blockedWrite: true });
    const child = f.backend.spawn("tsc", []);
    const observed = observe(child);
    child.stdin.write(Buffer.of(1), () => {});
    f.consumerClosed.reject(new Error("watcher failed"));
    await observed.closed;
    assert.equal(f.cancelled, true);
    assert.equal(observed.errors[0].code, "EIO");
    assert.deepEqual(observed.events.at(-1), ["close", null, null]);
  },
);

test(
  "async overflow retains each channel prefix and reports a process error",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    const child = f.backend.spawn("tsc", [], { maxBuffer: 2 });
    const observed = observe(child);
    f.complete({
      stdout: { bytes: [0, 255], overflow: true },
      stderr: { bytes: [128, 1] },
    });
    await observed.closed;
    assert.equal(observed.errors[0].code, "ENOBUFS");
    assert.deepEqual(observed.stdout, [0, 255]);
    assert.deepEqual(observed.stderr, [128, 1]);
    assert.equal(child.exitCode, null);
  },
);

test(
  "failure while attaching an abort listener joins the already started collector",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    const child = f.backend.spawn("tsc", [], {
      signal: {
        aborted: false,
        addEventListener() {
          throw new Error("listener failed");
        },
        removeEventListener() {},
      },
    });
    const observed = observe(child);
    await observed.closed;
    assert.equal(f.cancelled, true);
    assert.equal(observed.errors[0].code, "EIO");
    assert.match(observed.errors[0].message, /listener failed/);
    assert.equal(
      observed.events.some(([name]) => name === "spawn"),
      false,
    );
    assert.deepEqual(f.closed, [
      "future",
      "rpc",
      "closed",
      "writer",
      "prepared",
    ]);
  },
);

test(
  "a pending write that loses its consumer does not turn a command result into a spawn error",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture({ blockedWrite: true });
    const child = f.backend.spawn("tsc", []);
    const observed = observe(child);
    const written = new Promise((resolve) =>
      child.stdin.write(Buffer.of(1), (error) => resolve(error)),
    );
    await Promise.resolve();
    f.complete();
    await observed.closed;
    assert.equal((await written).code, "EPIPE");
    assert.equal(f.cancelled, false);
    assert.deepEqual(observed.errors, []);
    assert.equal(child.exitCode, 7);
  },
);

test(
  "resource cleanup continues after one disposer fails",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    f.future[Symbol.dispose] = () => {
      throw new Error("drop failed");
    };
    const child = f.backend.spawn("tsc", []);
    const observed = observe(child);
    f.complete();
    await observed.closed;
    assert.equal(observed.errors[0].code, "EIO");
    assert.equal(observed.errors[0].cause.message, "drop failed");
    assert.deepEqual(f.closed, ["rpc", "closed", "writer", "prepared"]);
  },
);

test(
  "async help, disabled and unknown commands remain lazy",
  { timeout: 2000 },
  async () => {
    const f = asyncFixture();
    f.process.env.GOLEM_NODE_TOOL_PROCESSES = "false";
    assert.equal(f.backend.spawn("tsc", []), undefined);
    assert.deepEqual(f.calls, []);
    delete f.process.env.GOLEM_NODE_TOOL_PROCESSES;
    f.host.prepareCommand = () => undefined;
    assert.equal(f.backend.spawn("unknown", []), undefined);
    f.host.prepareCommand = () => ({
      tag: "output",
      val: { exitCode: 0, stdout: [104], stderr: [] },
    });
    const child = f.backend.spawn("tsc", ["--help"]);
    const observed = observe(child);
    await observed.closed;
    assert.equal(Buffer.from(observed.stdout).toString(), "h");
    assert.deepEqual(observed.events[0], ["spawn"]);
    assert.deepEqual(f.calls, []);
  },
);

test(
  "an immediately completed async tool emits spawn before exit",
  { timeout: 2000 },
  async () => {
    await Promise.resolve().then(async () => {
      const f = asyncFixture();
      f.prepared.input = () => ({
        commandPath: ["run"],
        input: {},
        stdin: false,
        stdout: false,
        stderr: false,
      });
      f.complete();
      const child = f.backend.spawn("tsc", []);
      const observed = observe(child);
      await observed.closed;
      assert.deepEqual(observed.events[0], ["spawn"]);
      assert.deepEqual(observed.events.at(-1), ["close", 7, null]);
    });
  },
);
