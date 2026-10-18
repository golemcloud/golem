// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { Buffer } from "node:buffer";
import { resolve } from "node:path";
import { EventEmitter } from "node:events";
import { PassThrough, Writable } from "node:stream";
import { nextTick } from "node:process";

const DEFAULT_MAX_BUFFER = 1024 * 1024;

function finiteInput(input, encoding) {
  if (input === undefined) return Buffer.alloc(0);
  if (typeof input === "string") {
    return Buffer.from(
      input,
      encoding && encoding !== "buffer" ? encoding : "utf8",
    );
  }
  if (ArrayBuffer.isView(input)) {
    return Buffer.from(
      new Uint8Array(input.buffer, input.byteOffset, input.byteLength),
    );
  }
  throw new TypeError("input must be a string, Buffer, TypedArray or DataView");
}

function processError(code, command, message, method = "spawnSync") {
  return Object.assign(new Error(`${method} ${command}: ${message}`), {
    code,
    syscall: `${method} ${command}`,
    path: command,
  });
}

function failed(command, code, message, method = "spawnSync") {
  return {
    pid: 0,
    status: null,
    signal: null,
    stdout: Buffer.alloc(0),
    stderr: Buffer.alloc(0),
    error: processError(code, command, message, method),
  };
}

function dispose(resource) {
  resource?.[Symbol.dispose]?.();
}

function bounded(command, result, maxBuffer, method = "spawnSync") {
  for (const channel of ["stdout", "stderr"]) {
    result[channel] = Buffer.from(result[channel] ?? []);
    if (result[channel].length > maxBuffer) {
      result[channel] = result[channel].subarray(0, maxBuffer);
      result.error ??= processError(
        "ENOBUFS",
        command,
        `${channel} maxBuffer exceeded`,
        method,
      );
      result.status = null;
    }
  }
  return result;
}

function variables(runtimeProcess, options) {
  const values = Object.entries(options.env ?? runtimeProcess.env)
    .filter(([key, value]) => key !== "PWD" && value !== undefined)
    .map(([key, value]) => [key, String(value)]);
  values.push(["PWD", resolve(runtimeProcess.cwd(), options.cwd ?? ".")]);
  return values;
}

function project(command, request, collected, maxBuffer, method = "spawnSync") {
  const rendered = request.renderResult(collected.result);
  const result = bounded(
    command,
    {
      pid: 1,
      status: rendered.exitCode,
      signal: null,
      stdout: Buffer.concat([
        Buffer.from(collected.stdout?.bytes ?? []),
        Buffer.from(rendered.stdout),
      ]),
      stderr: Buffer.concat([
        Buffer.from(collected.stderr?.bytes ?? []),
        Buffer.from(rendered.stderr),
      ]),
    },
    maxBuffer,
    method,
  );
  if (collected.bodyExecuted === false && collected.result.tag === "err") {
    const code = {
      denied: "EACCES",
      "not-found": "ENOENT",
      "resource-exhausted": "ENOMEM",
    }[collected.result.val.tag];
    if (code) {
      result.error ??= processError(
        code,
        command,
        collected.result.val.tag,
        method,
      );
      result.pid = 0;
      result.status = null;
    }
  }
  for (const channel of ["stdout", "stderr"]) {
    if (collected[channel]?.overflow) {
      result.error ??= processError(
        "ENOBUFS",
        command,
        `${channel} maxBuffer exceeded`,
        method,
      );
      result.status = null;
    } else if (collected[channel]?.failure && !result.error) {
      result.error = processError(
        "EIO",
        command,
        `${channel} stream failed`,
        method,
      );
      result.status = null;
    }
  }
  return result;
}

function createInput(host, command, resources, onFailure) {
  const [writer, source, closed] = host.createStdin();
  for (const resource of [writer, source, closed]) resources.add(resource);
  let io = Promise.resolve();
  let terminal = false;
  let consumerClosed = false;
  const enqueue = (operation) => {
    const next = io.then(operation);
    io = next.catch(() => {});
    return next;
  };
  const inputError = (error) =>
    processError(
      error?.tag === "closed" ? "EPIPE" : "EIO",
      command,
      "stdin write failed",
      "spawn",
    );
  const stream = new Writable({
    write(chunk, _encoding, callback) {
      enqueue(() => writer.write(Buffer.from(chunk))).then(
        () => callback(),
        (error) => callback(inputError(error)),
      );
    },
    final(callback) {
      enqueue(async () => {
        await writer.finish();
        terminal = true;
      }).then(
        () => callback(),
        (error) => callback(inputError(error)),
      );
    },
    destroy(error, callback) {
      enqueue(async () => {
        if (!terminal && !consumerClosed) {
          try {
            await writer.fail({ tag: "abandoned" });
          } catch (failure) {
            if (failure?.tag !== "closed") throw failure;
          }
        }
        terminal = true;
      }).then(
        () => callback(error),
        (failure) => callback(error ?? inputError(failure)),
      );
    },
  });
  stream.on("error", (error) => {
    if (!consumerClosed && error.code !== "EPIPE") onFailure(error);
  });
  const watched = closed.wait().then(
    () => {
      consumerClosed = true;
      stream.destroy();
    },
    (error) => {
      const failure = inputError(error);
      onFailure(failure);
      stream.destroy(failure);
      throw failure;
    },
  );
  // Keep a rejection observed until stop joins the watcher and any pending I/O.
  watched.catch(() => {});
  return {
    stream,
    source,
    async stop() {
      const stopped = stream.closed
        ? Promise.resolve()
        : new Promise((resolve) => stream.once("close", resolve));
      stream.destroy();
      await stopped;
      await io;
      await watched;
    },
  };
}

function killSignal(value) {
  const signal = { 2: "SIGINT", 9: "SIGKILL", 15: "SIGTERM" }[value] ?? value;
  if (!["SIGTERM", "SIGINT", "SIGKILL"].includes(signal)) {
    throw Object.assign(new TypeError(`Unsupported signal: ${value}`), {
      code: "ERR_UNKNOWN_SIGNAL",
    });
  }
  return signal;
}

function spawnTool(host, runtimeProcess, command, args, options) {
  const enabled = runtimeProcess.env.GOLEM_NODE_TOOL_PROCESSES;
  if (enabled === "0" || enabled === "false") return undefined;
  const maxBuffer = options.maxBuffer ?? DEFAULT_MAX_BUFFER;
  if (!Number.isSafeInteger(maxBuffer) || maxBuffer < 0)
    throw new RangeError("maxBuffer must be a non-negative safe integer");
  if (
    options.timeout !== undefined &&
    (!Number.isSafeInteger(options.timeout) || options.timeout < 0)
  ) {
    throw new RangeError("timeout must be a non-negative safe integer");
  }
  const defaultSignal = killSignal(options.killSignal ?? "SIGTERM");
  if (
    options.signal !== undefined &&
    (typeof options.signal?.aborted !== "boolean" ||
      typeof options.signal?.addEventListener !== "function" ||
      typeof options.signal?.removeEventListener !== "function")
  ) {
    throw new TypeError("signal must be an AbortSignal");
  }
  const prepared = host.prepareCommand(
    command,
    args.map(String),
    variables(runtimeProcess, options),
  );
  if (prepared === undefined) return undefined;
  const child = new EventEmitter();
  child.connected = false;
  child.killed = false;
  child.exitCode = null;
  child.signalCode = null;
  child.spawnfile = command;
  child.spawnargs = [command, ...args.map(String)];
  child.stdout = new PassThrough();
  child.stderr = new PassThrough();
  child.stdin = null;
  const resources = new Set();
  const transfer = (resource) => {
    resources.delete(resource);
    return resource;
  };
  let future;
  let input;
  let completed = false;
  let terminal;
  let signal;
  let timer;
  let abortError;
  let stdinError;
  let cancellationRequested = false;
  let admitted = prepared.tag === "output";
  const cancel = () => {
    if (!future || cancellationRequested) return;
    future.cancel();
    cancellationRequested = true;
  };
  const inputFailed = (error) => {
    stdinError ??= error;
    try {
      cancel();
    } catch (failure) {
      stdinError.cause ??= failure;
    }
  };
  const abort = () => {
    abortError = Object.assign(
      new Error("The operation was aborted", { cause: options.signal.reason }),
      { name: "AbortError", code: "ABORT_ERR" },
    );
    child.kill(defaultSignal);
  };
  child.kill = (requested = "SIGTERM") => {
    if (requested === 0) return admitted && !completed && !!future;
    const normalized = killSignal(requested);
    if (completed || !admitted || !future) return false;
    cancel();
    signal ??= normalized;
    child.killed = true;
    return true;
  };
  child.ref = () => child;
  child.unref = () => {
    throw processError(
      "ENOTSUP",
      command,
      "detached tool processes are not supported",
      "spawn",
    );
  };

  let collection;
  try {
    if (prepared.tag === "invoke") resources.add(prepared.val);
    if (options.shell || (options.stdio && options.stdio !== "pipe")) {
      throw processError(
        "ENOTSUP",
        command,
        "shell and non-pipe stdio are not implemented yet",
        "spawn",
      );
    }
    if (options.signal?.aborted) {
      abortError = Object.assign(
        new Error("The operation was aborted", {
          cause: options.signal.reason,
        }),
        { name: "AbortError", code: "ABORT_ERR" },
      );
      throw abortError;
    }
    if (prepared.tag === "output") {
      terminal = bounded(
        command,
        {
          pid: 1,
          status: prepared.val.exitCode,
          signal: null,
          stdout: prepared.val.stdout,
          stderr: prepared.val.stderr,
        },
        maxBuffer,
        "spawn",
      );
    } else {
      const request = prepared.val;
      const spec = request.input();
      if (spec.stdin) {
        input = createInput(host, command, resources, inputFailed);
        child.stdin = input.stream;
      }
      const output = (wanted) => {
        if (!wanted) return undefined;
        const pair = host.createCollectedOutput(BigInt(maxBuffer));
        for (const resource of pair) resources.add(resource);
        return pair;
      };
      const stdout = output(spec.stdout);
      const stderr = output(spec.stderr);
      const rpc = new host.ToolRpc(command);
      resources.add(rpc);
      future = rpc.asyncInvokeAndAwait(
        spec.commandPath,
        spec.input,
        transfer(input?.source),
        transfer(stdout?.[0]),
        transfer(stderr?.[0]),
      );
      resources.add(future);
      admitted = future.admitted();
      collection = future.collectAsync(
        transfer(stdout?.[1]),
        transfer(stderr?.[1]),
      );
    }
    if (future && admitted && options.signal) {
      options.signal.addEventListener("abort", abort, { once: true });
      if (options.signal.aborted) abort();
    }
    if (future && admitted && options.timeout > 0) {
      timer = setTimeout(() => child.kill(defaultSignal), options.timeout);
    }
  } catch (error) {
    admitted = false;
    const code =
      error?.code ?? (error?.tag === "resource-exhausted" ? "ENOMEM" : "EIO");
    terminal = failed(
      command,
      code,
      error instanceof Error ? error.message : "tool process failed",
      "spawn",
    );
    try {
      cancel();
    } catch (failure) {
      terminal.error.cause ??= failure;
    }
  }
  child.stdio = [child.stdin, child.stdout, child.stderr];
  if (admitted) child.pid = 1;
  nextTick(() => {
    if (admitted) child.emit("spawn");
  });
  Promise.resolve().then(async () => {
    try {
      if (collection) {
        const collected = await collection;
        terminal ??= project(
          command,
          prepared.val,
          collected,
          maxBuffer,
          "spawn",
        );
        if (
          signal &&
          collected.result.tag === "err" &&
          collected.result.val.tag === "cancelled" &&
          !collected.stdout?.overflow &&
          !collected.stderr?.overflow &&
          [collected.stdout, collected.stderr].every(
            (value) => !value?.failure || value.failure.tag === "cancelled",
          )
        ) {
          terminal.error = undefined;
          terminal.status = null;
          terminal.signal = signal;
        }
      }
    } catch (error) {
      terminal = failed(
        command,
        "EIO",
        error instanceof Error ? error.message : "collection failed",
        "spawn",
      );
      try {
        cancel();
      } catch (failure) {
        terminal.error.cause ??= failure;
      }
    } finally {
      completed = true;
      if (timer !== undefined) clearTimeout(timer);
      const cleanupFailed = (error) => {
        terminal.error ??= processError(
          "EIO",
          command,
          "tool process cleanup failed",
          "spawn",
        );
        terminal.error.cause ??= error;
        terminal.status = null;
      };
      try {
        options.signal?.removeEventListener("abort", abort);
        if (input && resources.has(input.source))
          dispose(transfer(input.source));
      } catch (error) {
        cleanupFailed(error);
      }
      try {
        await input?.stop();
      } catch (error) {
        cleanupFailed(error);
      }
      for (const resource of [...resources].reverse()) {
        try {
          dispose(resource);
        } catch (error) {
          cleanupFailed(error);
        }
      }
    }
    terminal.error = abortError ?? stdinError ?? terminal.error;
    if (terminal.error) terminal.status = null;
    child.exitCode = terminal.status;
    child.signalCode = terminal.signal;
    child.stdout.end(terminal.stdout);
    child.stderr.end(terminal.stderr);
    if (terminal.error) child.emit("error", terminal.error);
    else child.emit("exit", terminal.status, terminal.signal);
    nextTick(() => child.emit("close", terminal.status, terminal.signal));
  });
  return child;
}

/// Constructing the backend performs no discovery, host I/O or authority lookup.
/// The host facade is explicit so this module is shared by all JavaScript SDKs.
export function createToolProcessBackend(host, runtimeProcess) {
  return {
    spawn(command, args, options = {}) {
      return spawnTool(host, runtimeProcess, command, args, options);
    },
    spawnSync(command, args, options = {}) {
      const enabled = runtimeProcess.env.GOLEM_NODE_TOOL_PROCESSES;
      if (enabled === "0" || enabled === "false") return undefined;
      // These cases need the asynchronous/cancellation adapter. Do not silently
      // run a command with an ignored option while that adapter is unavailable.
      if (
        options.shell ||
        options.timeout > 0 ||
        (options.stdio && options.stdio !== "pipe")
      ) {
        return failed(
          command,
          "ENOTSUP",
          "shell, timeout and non-pipe stdio are not implemented yet",
        );
      }
      const maxBuffer = options.maxBuffer ?? DEFAULT_MAX_BUFFER;
      if (!Number.isSafeInteger(maxBuffer) || maxBuffer < 0) {
        throw new RangeError("maxBuffer must be a non-negative safe integer");
      }
      const inputBytes = finiteInput(options.input, options.encoding);
      const prepared = host.prepareCommand(
        command,
        args.map(String),
        variables(runtimeProcess, options),
      );
      if (prepared === undefined) return undefined;
      if (prepared.tag === "output") {
        return bounded(
          command,
          {
            pid: 1,
            status: prepared.val.exitCode,
            signal: null,
            stdout: prepared.val.stdout,
            stderr: prepared.val.stderr,
          },
          maxBuffer,
        );
      }

      const request = prepared.val;
      const resources = new Set([request]);
      const transfer = (resource) => {
        resources.delete(resource);
        return resource;
      };
      let future;
      let completed = false;
      try {
        const spec = request.input();
        const input = spec.stdin
          ? host.createStdinFromBytes(inputBytes)
          : undefined;
        if (input) resources.add(input);
        const stdout = spec.stdout
          ? host.createCollectedOutput(BigInt(maxBuffer))
          : undefined;
        if (stdout) for (const resource of stdout) resources.add(resource);
        const stderr = spec.stderr
          ? host.createCollectedOutput(BigInt(maxBuffer))
          : undefined;
        if (stderr) for (const resource of stderr) resources.add(resource);
        const rpc = new host.ToolRpc(command);
        resources.add(rpc);
        future = rpc.asyncInvokeAndAwait(
          spec.commandPath,
          spec.input,
          transfer(input),
          transfer(stdout?.[0]),
          transfer(stderr?.[0]),
        );
        resources.add(future);
        const collected = future.collect(
          transfer(stdout?.[1]),
          transfer(stderr?.[1]),
        );
        completed = true;
        return project(command, request, collected, maxBuffer);
      } catch (error) {
        const code = error?.tag === "resource-exhausted" ? "ENOMEM" : "EIO";
        return failed(
          command,
          code,
          error instanceof Error ? error.message : "tool process failed",
        );
      } finally {
        if (future && !completed) {
          try {
            future.cancel();
          } catch {
            /* Preserve the original failure. */
          }
        }
        for (const resource of [...resources].reverse()) dispose(resource);
      }
    },
  };
}
