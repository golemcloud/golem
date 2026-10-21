// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import * as native from "golem-tool-process-bridge";
import { createToolProcessBackend } from "golem-tool-process-backend";
import { createToolProcessHost } from "golem-tool-process-host";
import * as childProcess from "node:child_process";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, symlinkSync, writeFileSync } from "node:fs";
import { once } from "node:events";
import { sdkOverflowControl } from "golem-tool-process-sdk-control";
import process from "node:process";
import { runJavaScript } from "wasm-rquickjs:execution";
import {
  ok,
  processExitStatusSchema,
  toolDefinition,
  z,
} from "golem-tool-process-sdk-control";

export {
  golemTool010Guest,
  golemTool010Guest as guest,
} from "golem-tool-process-sdk-control";
export {
  golemAgent200Guest,
  toolMiddlewareGuest,
  toolMiddlewareGuest as golemTool010ToolMiddlewareGuest,
  saveSnapshot,
  loadSnapshot,
} from "golem-tool-process-sdk-control";

const adapter = createToolProcessBackend(
  createToolProcessHost(native),
  process,
);

const probe = toolDefinition("node-tool-probe", { requiresFilesystem: true })
  .version("0.0.1")
  .body((body) =>
    body
      .option("cwd", z.string(), { default: "/workspace" })
      .tail("args", z.string(), { separator: "--", verbatim: true })
      .stdout({ required: true })
      .stderr({ required: true })
      .returns(processExitStatusSchema),
  );

function startCapture(backend, args, options = {}) {
  const child = backend.spawn("streaming", args, {
    maxBuffer: 4096,
    ...options,
  });
  const stdout = [];
  const stderr = [];
  let error;
  let first;
  const firstOutput = new Promise((resolve) => {
    first = resolve;
  });
  child.stdout.on("data", (bytes) => {
    stdout.push(...bytes);
    first();
  });
  child.stderr.on("data", (bytes) => stderr.push(...bytes));
  child.on("error", (failure) => {
    error ??= failure;
  });
  const done = new Promise((resolve) =>
    child.once("close", (status, signal) => {
      first();
      resolve({
        status,
        signal,
        error,
        stdout: Uint8Array.from(stdout),
        stderr: Uint8Array.from(stderr),
      });
    }),
  );
  return { child, done, firstOutput, stdout };
}

async function checkPublicProcessContract() {
  const saved = process.env.GOLEM_NODE_TOOL_PROCESSES;
  try {
    for (const disabled of ["0", "false"]) {
      process.env.GOLEM_NODE_TOOL_PROCESSES = disabled;
      assert.equal(
        childProcess.spawnSync("streaming", ["--help"]).error?.code,
        "ENOSYS",
      );
      const child = childProcess.spawn("streaming", ["--help"]);
      const failure = once(child, "error");
      const closed = new Promise((resolve) => child.once("close", resolve));
      assert.equal((await failure)[0].code, "ENOSYS");
      await closed;
      const local = childProcess.spawnSync(process.execPath, ["-p", "6 * 7"], {
        encoding: "utf8",
      });
      assert.equal(local.status, 0);
      assert.equal(local.stdout.trim(), "42");
    }
  } finally {
    if (saved === undefined) delete process.env.GOLEM_NODE_TOOL_PROCESSES;
    else process.env.GOLEM_NODE_TOOL_PROCESSES = saved;
  }
  assert.equal(
    childProcess.spawnSync("unbound-process-probe").error?.code,
    "ENOSYS",
  );
  const directory = "/workspace/node-tool-process-bin";
  mkdirSync(directory, { recursive: true });
  writeFileSync(
    `${directory}/entry.cjs`,
    '#!/usr/bin/env node\nconsole.log("local:" + process.argv[2]);\n',
  );
  if (!existsSync(`${directory}/streaming`))
    symlinkSync("entry.cjs", `${directory}/streaming`);
  // A filesystem executable with the bound tool's name must still run locally.
  const options = {
    env: { ...process.env, PATH: directory },
    encoding: "utf8",
  };
  assert.equal(
    childProcess.spawnSync("streaming", ["priority"], options).stdout,
    "local:priority\n",
  );
  assert.equal(
    childProcess.execFileSync(`${directory}/streaming`, ["absolute"], options),
    "local:absolute\n",
  );
  await new Promise((resolve, reject) => {
    childProcess.execFile("streaming", ["callback"], options, (error, out) => {
      try {
        assert.ifError(error);
        assert.equal(out, "local:callback\n");
        resolve();
      } catch (failure) {
        reject(failure);
      }
    });
  });
}

async function cancelNative(native) {
  const prepared = native.prepareCommand(
    "streaming",
    ["run", "marker-echo"],
    [],
  );
  if (prepared?.tag !== "invoke")
    throw new Error("Native command not prepared");
  const stdin = native.createInput(undefined);
  const stdout = native.createOutput(4096);
  let child;
  let pending;
  let closed;
  let tail;
  try {
    child = native.invoke(prepared.val.input, stdin, stdout, undefined);
    pending = native.collectAsync(child);
    pending.catch(() => {});
    closed = native.waitInputClosed(stdin);
    closed.catch(() => {});
    const marker = await native.readOutput(stdout);
    if (new TextDecoder().decode(marker) !== "marker:")
      throw new Error("Native child did not reach its input wait");
    tail = native.readOutput(stdout);
    tail.catch(() => {});
    native.cancel(child);
    const result = await pending;
    if (result.result?.tag !== "err" || result.result.val?.tag !== "cancelled")
      throw new Error("Native cancellation did not settle");
    const [output] = await Promise.allSettled([tail, closed]);
    if (output.status === "fulfilled" && output.value !== undefined)
      throw new Error("Native child wrote after cancellation");
  } finally {
    try {
      if (child) {
        native.cancel(child);
        await Promise.allSettled([pending, tail, closed]);
      }
    } finally {
      native.disposeInput(stdin, "writer");
      native.disposeInput(stdin, "source");
      native.disposeInput(stdin, "closed");
      native.disposeOutput(stdout, true);
      native.disposeOutput(stdout, false);
      if (child) native.disposeProcess(child);
    }
  }
}

probe.implement({
  async "node-tool-probe"(input, context) {
    const [mode, effectUrl, execution = "all", dispatch = "public-api"] =
      input.args;
    assert.ok(["public-api", "adapter"].includes(dispatch));
    const control = execution === "fresh-stream-control";
    const nativeControl = execution.endsWith("native-cancel");
    const sdkControl = execution === "fresh-sdk-overflow";
    const synchronous = [
      "all",
      "main-sync",
      "main-both",
      "fresh-sync",
    ].includes(execution);
    const asynchronous = [
      "all",
      "main-async",
      "main-both",
      "fresh-async",
    ].includes(execution);
    const extra = execution === "all";
    const fresh = extra || execution.startsWith("fresh-");
    const echo = mode.includes("echo");
    const pressure = mode.includes("pressure");
    const args = echo
      ? ["run", "echo"]
      : pressure
        ? ["produce", "512", "65536"]
        : ["dual-reconstruct", "redaction-terminals"];
    if (effectUrl) {
      const response = await fetch(`${effectUrl}/${mode}-main`, {
        method: "POST",
      });
      await response.arrayBuffer();
    }
    const bytes = echo
      ? new Uint8Array([0, 255, 1, 128, 0, 13, 10, 254])
      : new Uint8Array();
    const backend = dispatch === "adapter" ? adapter : childProcess;
    if (execution === "all" && dispatch === "public-api")
      await checkPublicProcessContract();
    const summarize = (result) => {
      if (result.error?.code === "ENOBUFS")
        return `overflow:${result.stdout.length}`;
      if (result.error || result.status !== 0)
        throw new Error(
          `process failure: ${result.error?.message ?? result.stderr.toString()}`,
        );
      const sum = (bytes) => [...bytes].reduce((sum, byte) => sum + byte, 0);
      return `${result.stdout.length}:${sum(result.stdout)}:${result.stderr.length}:${sum(result.stderr)}`;
    };
    let value =
      control ||
      nativeControl ||
      execution === "main-cancel" ||
      execution === "main-overlap"
        ? "7:700:14:1455"
        : undefined;
    if (nativeControl) await cancelNative(native);
    if (sdkControl) value = await sdkOverflowControl();
    if (synchronous) {
      value = summarize(
        backend.spawnSync("streaming", args, {
          input: bytes,
          maxBuffer: 4096,
        }),
      );
    }
    if (asynchronous) {
      const child = startCapture(backend, args);
      child.child.stdin?.end(bytes);
      const result = summarize(await child.done);
      value ??= result;
      if (result !== value) throw new Error("Async process differs");
    }
    if (extra && !pressure && dispatch === "public-api") {
      const stdout = backend.execFileSync("streaming", args, {
        input: bytes,
        maxBuffer: 4096,
      });
      const callback = await new Promise((resolve, reject) => {
        const child = backend.execFile(
          "streaming",
          args,
          { encoding: "buffer", maxBuffer: 4096 },
          (error, stdout, stderr) => {
            if (error) reject(error);
            else resolve({ status: 0, stdout, stderr });
          },
        );
        child.stdin?.end(bytes);
      });
      assert.deepEqual(stdout, callback.stdout);
      assert.equal(summarize(callback), value);
    }
    if (echo && extra) {
      const large = Uint8Array.from(
        { length: 131089 },
        (_, index) => index % 256,
      );
      const options = { maxBuffer: large.length };
      const matches = (result) =>
        !result.error &&
        result.status === 0 &&
        result.stderr.length === 0 &&
        result.stdout.length === large.length &&
        large.every((byte, index) => result.stdout[index] === byte);
      const synchronous = backend.spawnSync("streaming", args, {
        ...options,
        input: large,
      });
      if (!matches(synchronous))
        throw new Error("Large synchronous stdin differs");
      const asynchronous = startCapture(backend, args, options);
      asynchronous.child.stdin.end(large);
      if (!matches(await asynchronous.done))
        throw new Error("Large asynchronous stdin differs");
    }
    if (!echo && !pressure && (extra || execution === "main-overlap")) {
      const overlap = startCapture(backend, ["run", "marker-echo"]);
      try {
        await overlap.firstOutput;
        if (
          new TextDecoder().decode(Uint8Array.from(overlap.stdout)) !==
          "marker:"
        )
          throw new Error("Async child did not reach its input wait");
        if (
          summarize(
            backend.spawnSync("streaming", args, { maxBuffer: 4096 }),
          ) !== value
        )
          throw new Error("Overlapping synchronous process differs");
        overlap.child.stdin.end(new Uint8Array([0, 255, 128]));
        const result = await overlap.done;
        if (
          result.error ||
          result.status !== 0 ||
          result.stdout.length !== 10 ||
          result.stdout[8] !== 255 ||
          result.stdout[9] !== 128
        )
          throw new Error("Async child did not survive synchronous execution");
      } finally {
        overlap.child.kill();
        await overlap.done;
      }
    }
    if (!echo && !pressure && (extra || execution === "main-cancel")) {
      const cancelled = startCapture(backend, ["run", "marker-echo"]);
      try {
        await cancelled.firstOutput;
        if (!cancelled.child.kill())
          throw new Error("Child cancellation refused");
        const stopped = await cancelled.done;
        if (
          stopped.error ||
          stopped.status !== null ||
          stopped.signal !== "SIGTERM"
        )
          throw new Error("Child cancellation did not settle");
      } finally {
        cancelled.child.kill();
        await cancelled.done;
      }
      if (summarize(backend.spawnSync("streaming", args)) !== value)
        throw new Error("Follow-up process after cancellation differs");
    }
    if (!echo && fresh) {
      const job = await runJavaScript({
        source: sdkControl
          ? `
                const { sdkOverflowControl } = await import('golem-tool-process-sdk-control');
                if (${JSON.stringify(effectUrl)}) {
                    const response = await fetch(${JSON.stringify(effectUrl)} + '/' + ${JSON.stringify(mode)} + '-job', { method: 'POST' });
                    await response.arrayBuffer();
                }
                return await sdkOverflowControl();
            `
          : control
            ? `
                const { PassThrough } = await import('node:stream');
                const { finished } = await import('node:stream/promises');
                const stdout = new PassThrough();
                const stderr = new PassThrough();
                const result = { stdout: [], stderr: [] };
                stdout.on('data', bytes => result.stdout.push(...bytes));
                stderr.on('data', bytes => result.stderr.push(...bytes));
                const done = Promise.all([finished(stdout), finished(stderr)]);
                if (${JSON.stringify(effectUrl)}) {
                    const response = await fetch(${JSON.stringify(effectUrl)} + '/' + ${JSON.stringify(mode)} + '-job', { method: 'POST' });
                    await response.arrayBuffer();
                }
                stdout.end(new Uint8Array([100, 100, 100, 100, 100, 100, 100]));
                stderr.end(new Uint8Array([103, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104]));
                await done;
                const summarize = ${summarize.toString()};
                return summarize({ status: 0, ...result });
            `
            : nativeControl
              ? `
                const native = await import('golem-tool-process-bridge');
                if (${JSON.stringify(effectUrl)}) {
                    const response = await fetch(${JSON.stringify(effectUrl)} + '/' + ${JSON.stringify(mode)} + '-job', { method: 'POST' });
                    await response.arrayBuffer();
                }
                const cancelNative = ${cancelNative.toString()};
                await cancelNative(native);
                return '7:700:14:1455';
            `
              : `
                const { default: process } = await import('node:process');
                const backend = ${JSON.stringify(dispatch)} === 'adapter'
                    ? (await import('golem-tool-process-backend')).createToolProcessBackend(
                        (await import('golem-tool-process-host')).createToolProcessHost(
                            await import('golem-tool-process-bridge')), process)
                    : await import('node:child_process');
                if (${JSON.stringify(effectUrl)}) {
                    const response = await fetch(${JSON.stringify(effectUrl)} + '/' + ${JSON.stringify(mode)} + '-job', { method: 'POST' });
                    await response.arrayBuffer();
                }
                const summarize = ${summarize.toString()};
                const startCapture = ${startCapture.toString()};
                let value;
                if (${synchronous}) value = summarize(backend.spawnSync('streaming', ${JSON.stringify(args)}, { maxBuffer: 4096 }));
                if (${asynchronous}) {
                    const child = startCapture(backend, ${JSON.stringify(args)});
                    const result = summarize(await child.done);
                    value ??= result;
                    if (result !== value) throw new Error('Fresh async process differs');
                }
                return value;
            `,
        cwd: "/",
        timeoutMs: 10000,
        maxBytes: 4096,
      });
      if (job.value !== value || job.stdout || job.stderr || job.overflowed)
        throw new Error("Fresh runtime differs");
    }
    const writer = context.stdout.getWriter();
    try {
      const path = execution === "all" ? mode : `${mode}[${execution}]`;
      const label = dispatch === "adapter" ? `${path}[adapter]` : path;
      await writer.write(new TextEncoder().encode(`${label}:${value}\n`));
    } finally {
      writer.releaseLock();
    }
    return ok({ exit_code: 0 });
  },
});
