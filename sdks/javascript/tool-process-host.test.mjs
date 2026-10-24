import { test } from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import { createToolProcessBackend } from "./tool-process-backend.js";
import { createToolProcessHost } from "./tool-process-host.js";

function fixture() {
  const calls = [];
  const prepared = {};
  let close;
  const closed = new Promise((resolve) => {
    close = resolve;
  });
  const terminal = {
    result: { tag: "ok", rendered: { exitCode: 7, stdout: [], stderr: [] } },
    stdout: { bytes: [0, 255] },
    stderr: { bytes: [128] },
  };
  const native = {
    prepareCommand(name, args, variables) {
      calls.push(["prepare", name, args, variables]);
      return {
        tag: "invoke",
        val: {
          commandPath: ["run"],
          input: prepared,
          stdin: true,
          stdout: true,
          stderr: true,
        },
      };
    },
    createInput(bytes) {
      calls.push(["input", bytes]);
      return { kind: "input" };
    },
    createOutput(max) {
      calls.push(["output", max]);
      return {
        kind: "output",
        max,
        channel:
          calls.filter(([name]) => name === "output").length === 1
            ? "stdout"
            : "stderr",
      };
    },
    async readOutput(output) {
      if (output.done) return undefined;
      output.done = true;
      return new Uint8Array(terminal[output.channel].bytes);
    },
    invoke(...args) {
      calls.push(["invoke", ...args]);
      return { kind: "process" };
    },
    collectSync(process, timeout) {
      calls.push(["sync", process, timeout]);
      return terminal;
    },
    async collectAsync(process) {
      calls.push(["async", process]);
      await closed;
      return terminal;
    },
    async write(input, bytes) {
      calls.push(["write", input, [...bytes]]);
    },
    async finish(input, failed) {
      calls.push(["finish", input, failed]);
      close();
    },
    waitInputClosed: () => closed,
    cancel(process) {
      calls.push(["cancel", process]);
      close();
    },
    disposeInput(input, part) {
      calls.push(["dispose-input", input, part]);
      if (part === "source") close();
    },
    disposeOutput(output, target) {
      calls.push(["dispose-output", output, target]);
    },
    disposeProcess(process) {
      calls.push(["dispose-process", process]);
    },
  };
  const host = createToolProcessHost(native);
  const backend = createToolProcessBackend(host, {
    env: {},
    cwd: () => "/workspace",
  });
  return { native, host, backend, calls, prepared, terminal };
}

test("native facade registration is lazy and forwards canonical preparation", () => {
  const f = fixture();
  assert.deepEqual(f.calls, []);
  const result = f.backend.spawnSync("streaming", ["run", "echo"], {
    input: new Uint8Array([0, 128, 255]),
    timeout: 12,
    maxBuffer: 19,
  });
  assert.equal(result.status, 7);
  assert.deepEqual([...result.stdout], [0, 255]);
  assert.deepEqual([...result.stderr], [128]);
  assert.equal(f.calls.find(([name]) => name === "invoke")[1], f.prepared);
  assert.deepEqual(
    [...f.calls.find(([name]) => name === "input")[1]],
    [0, 128, 255],
  );
  assert.deepEqual(
    f.calls.filter(([name]) => name === "output").map((call) => call[1]),
    [19, 19],
  );
  assert.equal(f.calls.find(([name]) => name === "sync")[2], 12);
  assert.equal(
    f.calls.filter(([name]) => name === "dispose-process").length,
    1,
  );
});

test("ignored outputs use native discard drains, not zero-sized collectors", () => {
  const f = fixture();
  const result = f.backend.spawnSync("streaming", [], {
    stdio: "ignore",
    maxBuffer: 0,
  });
  assert.equal(result.status, 7);
  assert.deepEqual(
    f.calls.filter(([name]) => name === "output").map((call) => call[1]),
    [undefined, undefined],
  );
});

test("native async collection and stdin writes make independent progress", async () => {
  const f = fixture();
  const child = f.backend.spawn("streaming", ["run", "echo"]);
  const output = [];
  child.stdout.on("data", (bytes) => output.push(...bytes));
  const done = once(child, "close");
  child.stdin.end(new Uint8Array([0, 255, 128]));
  assert.deepEqual(await done, [7, null]);
  assert.deepEqual(output, [0, 255]);
  assert.deepEqual(
    f.calls.find(([name]) => name === "write")[2],
    [0, 255, 128],
  );
  assert.equal(
    f.calls.filter(([name]) => name === "dispose-process").length,
    1,
  );
  assert.deepEqual(
    f.calls
      .filter(([name]) => name === "dispose-input")
      .map((call) => call[2])
      .sort(),
    ["closed", "writer"],
  );
});

test("spawn precedes already-ready native output", async () => {
  const f = fixture();
  const child = f.backend.spawn("streaming", []);
  const events = [];
  child.on("spawn", () => events.push("spawn"));
  child.stdout.on("data", () => events.push("stdout"));
  child.stderr.on("data", () => events.push("stderr"));
  const done = once(child, "close");
  child.stdin.end();
  await done;
  assert.equal(events[0], "spawn");
  assert.ok(events.includes("stdout"));
  assert.ok(events.includes("stderr"));
});

test("spawn uses the nextTick queue before already queued Promise work", async () => {
  const events = [];
  await new Promise((resolve, reject) => {
    setImmediate(() => {
      Promise.resolve().then(() => events.push("promise"));
      const f = fixture();
      const child = f.backend.spawn("streaming", []);
      child.on("spawn", () => events.push("spawn"));
      child.stdout.resume();
      child.stderr.resume();
      child.on("error", reject);
      child.once("close", resolve);
      child.stdin.end();
    });
  });
  assert.deepEqual(events, ["spawn", "promise"]);
});

test(
  "a child can read live output before it supplies stdin",
  { timeout: 1000 },
  async () => {
    const f = fixture();
    const child = f.backend.spawn("streaming", ["run", "echo"]);
    const done = once(child, "close");
    child.stdout.once("data", () =>
      child.stdin.end(new Uint8Array([9, 0, 255])),
    );
    assert.deepEqual(await done, [7, null]);
    assert.deepEqual(
      f.calls.find(([name]) => name === "write")[2],
      [9, 0, 255],
    );
  },
);

test(
  "kill releases an async collector blocked by output backpressure",
  { timeout: 1000 },
  async () => {
    const f = fixture();
    f.terminal.result = {
      tag: "err",
      val: { tag: "cancelled" },
      rendered: { exitCode: 130, stdout: [], stderr: [] },
    };
    f.terminal.stdout.bytes = Buffer.alloc(256 * 1024, 65);
    const child = f.backend.spawn("streaming", []);
    const done = once(child, "close");
    await once(child.stdout, "readable");
    assert.equal(child.kill(), true);
    assert.deepEqual(await done, [null, "SIGTERM"]);
    assert.equal(
      f.calls.filter(([name]) => name === "dispose-process").length,
      1,
    );
  },
);

test(
  "streamed output and terminal diagnostics share one channel limit",
  { timeout: 1000 },
  async () => {
    const f = fixture();
    f.terminal.result.rendered.stderr = [7, 8];
    const child = f.backend.spawn("streaming", [], { maxBuffer: 2 });
    const bytes = [];
    const errors = [];
    child.stderr.on("data", (chunk) => bytes.push(...chunk));
    child.on("error", (error) => errors.push(error));
    const done = new Promise((resolve) => child.once("close", resolve));
    child.stdin.end();
    await done;
    assert.deepEqual(bytes, [128, 7]);
    assert.equal(errors[0]?.code, "ENOBUFS");
  },
);

test(
  "close joins admitted output writes after kill",
  { timeout: 1000 },
  async () => {
    const f = fixture();
    f.terminal.result = {
      tag: "err",
      val: { tag: "cancelled" },
      rendered: { exitCode: 130, stdout: [], stderr: [] },
    };
    f.terminal.stdout.bytes = Buffer.alloc(256 * 1024, 65);
    const child = f.backend.spawn("streaming", []);
    const write = child.stdout.write.bind(child.stdout);
    let completed = false;
    child.stdout.write = (bytes, callback) =>
      write(bytes, (error) => {
        completed = true;
        callback(error);
      });
    const done = once(child, "close");
    await once(child.stdout, "readable");
    assert.equal(child.kill(), true);
    assert.deepEqual(await done, [null, "SIGTERM"]);
    assert.equal(completed, true, "close preceded the admitted write callback");
  },
);

test("terminal cleanup waits for stdin closure instead of failing it again", async () => {
  const f = fixture();
  let terminalReady;
  const terminal = new Promise((resolve) => {
    terminalReady = resolve;
  });
  let peerClosed;
  const closed = new Promise((resolve) => {
    peerClosed = resolve;
  });
  f.terminal.result = {
    tag: "err",
    val: { tag: "cancelled" },
    rendered: { exitCode: 130, stdout: [], stderr: [] },
  };
  f.native.collectAsync = async () => {
    await terminal;
    return f.terminal;
  };
  f.native.waitInputClosed = () => closed;
  f.native.cancel = () => {
    terminalReady();
    setImmediate(peerClosed);
  };
  const child = f.backend.spawn("streaming", []);
  child.stdout.resume();
  child.stderr.resume();
  const done = once(child, "close");
  await once(child, "spawn");
  child.kill();
  assert.deepEqual(await done, [null, "SIGTERM"]);
  assert.equal(
    f.calls.some(([operation, , failed]) => operation === "finish" && failed),
    false,
    "cleanup issued a second stdin terminal before observing peer closure",
  );
});

test(
  "close waits for owned output finalization",
  { timeout: 1000 },
  async () => {
    const f = fixture();
    const child = f.backend.spawn("streaming", []);
    let finalized = false;
    child.stdout._final = (callback) =>
      setImmediate(() => {
        finalized = true;
        callback();
      });
    child.stdout.resume();
    child.stderr.resume();
    const done = once(child, "close");
    child.stdin.end();
    assert.deepEqual(await done, [7, null]);
    assert.equal(finalized, true, "close preceded output finalization");
    assert.equal(child.stdout.readableEnded, true);
    assert.equal(child.stderr.readableEnded, true);
  },
);

for (const readOnExit of [false, true]) {
  test(
    `bounded owned output can exit ${readOnExit ? "before the caller starts reading" : "without a reader"}`,
    { timeout: 1000 },
    async () => {
      const f = fixture();
      f.terminal.stdout.bytes = Buffer.alloc(256 * 1024, 65);
      const child = f.backend.spawn("streaming", []);
      const output = [];
      let exited = false;
      child.once("exit", () => {
        exited = true;
        if (readOnExit)
          child.stdout.on("data", (chunk) => output.push(Buffer.from(chunk)));
      });
      const done = once(child, "close");
      child.stdin.end();
      assert.deepEqual(await done, [7, null]);
      assert.equal(exited, true);
      assert.equal(child.stdout.readableEnded, true);
      if (readOnExit)
        assert.deepEqual(Buffer.concat(output), f.terminal.stdout.bytes);
    },
  );
}

test("native collection failures cancel the call and release its resources", () => {
  const f = fixture();
  f.native.collectSync = () => {
    throw new Error("read failed");
  };
  assert.equal(f.backend.spawnSync("streaming", [], {}).error.code, "EIO");
  assert.deepEqual(
    f.calls.slice(-2).map((call) => call[0]),
    ["cancel", "dispose-process"],
  );
});

for (const asynchronous of [false, true]) {
  test(`native dispatch failure releases transferred attachments (${asynchronous ? "async" : "sync"})`, async () => {
    const f = fixture();
    f.native.invoke = () => {
      throw new Error("dispatch failed");
    };
    if (asynchronous) {
      const child = f.backend.spawn("streaming", []);
      const errors = [];
      child.on("error", (error) => errors.push(error));
      await new Promise((resolve) => child.once("close", resolve));
      assert.equal(errors[0]?.code, "EIO");
    } else {
      assert.equal(f.backend.spawnSync("streaming", []).error.code, "EIO");
    }
    assert.equal(
      f.calls.filter(
        ([name, , part]) => name === "dispose-input" && part === "source",
      ).length,
      1,
    );
    assert.equal(
      f.calls.filter(
        ([name, , target]) => name === "dispose-output" && target === true,
      ).length,
      2,
    );
    assert.equal(
      f.calls.filter(
        ([name, , target]) => name === "dispose-output" && target === false,
      ).length,
      2,
    );
    assert.equal(
      f.calls.some(([name]) => name === "dispose-process"),
      false,
    );
  });
}

test("help and missing bindings perform no native dispatch or attachment creation", () => {
  const f = fixture();
  f.native.prepareCommand = () => ({
    tag: "output",
    val: { exitCode: 0, stdout: [104], stderr: [] },
  });
  assert.equal(
    f.backend.spawnSync("streaming", ["--help"]).stdout.toString(),
    "h",
  );
  f.native.prepareCommand = () => undefined;
  assert.equal(f.backend.spawnSync("unbound", []), undefined);
  assert.deepEqual(f.calls, []);
});
