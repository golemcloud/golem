// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { Buffer } from "node:buffer";
import { resolve } from "node:path";
import { EventEmitter } from "node:events";
import { nextTick } from "node:process";
import { PassThrough, Writable } from "node:stream";
import { finished } from "node:stream/promises";

const DEFAULT_MAX_BUFFER = 1024 * 1024;
const PIPE_STDIO = ["pipe", "pipe", "pipe"];

function stdioModes(runtimeProcess, options) {
  const values = options.stdio ?? "pipe";
  if (Array.isArray(values) && values.length > 3) {
    throw new TypeError(
      "Golem tool processes have only stdin, stdout and stderr",
    );
  }
  if (!Array.isArray(values) && typeof values !== "string") {
    throw new TypeError("stdio must be pipe, ignore, inherit or an array");
  }
  return PIPE_STDIO.map((_, index) => {
    const value = Array.isArray(values) ? values[index] : values;
    const mode = value == null ? "pipe" : value === index ? "inherit" : value;
    if (!["pipe", "ignore", "inherit"].includes(mode))
      throw new TypeError("Unsupported stdio mode");
    if (
      mode === "inherit" &&
      index > 0 &&
      typeof runtimeProcess[index === 1 ? "stdout" : "stderr"]?.write !==
        "function"
    ) {
      throw new TypeError("Inherited output must be a writable process stream");
    }
    return mode;
  });
}

function routeOutput(runtimeProcess, result, modes, command) {
  for (const [index, channel] of [
    [1, "stdout"],
    [2, "stderr"],
  ]) {
    try {
      if (modes[index] === "inherit" && result[channel].length)
        runtimeProcess[channel].write(result[channel]);
    } catch (error) {
      result.error ??= processError("EIO", command, "inherited output failed");
      result.error.cause ??= error;
      result.status = null;
    }
    if (modes[index] !== "pipe") result[channel] = null;
  }
  return result;
}

async function routeOutputAsync(runtimeProcess, result, modes) {
  const routed = await Promise.allSettled(
    [
      [1, "stdout"],
      [2, "stderr"],
    ].map(async ([index, channel]) => {
      if (modes[index] === "inherit" && result[channel].length) {
        await new Promise((resolve, reject) =>
          runtimeProcess[channel].write(result[channel], (error) =>
            error ? reject(error) : resolve(),
          ),
        );
      }
      if (modes[index] !== "pipe") result[channel] = null;
    }),
  );
  const failure = routed.find((result) => result.status === "rejected");
  if (failure) throw failure.reason;
}

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

function bounded(
  command,
  result,
  maxBuffer,
  method = "spawnSync",
  modes = PIPE_STDIO,
) {
  for (const [index, channel] of [
    [1, "stdout"],
    [2, "stderr"],
  ]) {
    result[channel] =
      modes[index] === "ignore"
        ? Buffer.alloc(0)
        : Buffer.from(result[channel] ?? []);
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

function shellWord(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

function invocation(runtimeProcess, command, args, options) {
  if (
    options.shell != null &&
    typeof options.shell !== "boolean" &&
    typeof options.shell !== "string"
  ) {
    throw new TypeError("shell must be a boolean or string");
  }
  const argv = args.map(String);
  const conventionalShell = [
    "sh",
    "bash",
    "/bin/sh",
    "/bin/bash",
    "/usr/bin/sh",
    "/usr/bin/bash",
  ].includes(command);
  let script;
  if (options.shell) {
    // Node passes shell arguments as one space-separated command, not argv.
    script = [command, ...argv].join(" ");
    if (typeof options.shell === "string") {
      script = `${shellWord(options.shell)} -c ${shellWord(script)}`;
    }
  } else if (conventionalShell && !(command === "bash" && argv[0] === "run")) {
    // Bash's own sh/bash command implements option and positional-argument
    // semantics. Preserve argv as shell words instead of parsing its flags here.
    script = [command, ...argv].map(shellWord).join(" ");
  }
  return script === undefined
    ? { name: command, args: argv, shell: false }
    : {
        name: "bash",
        args: [
          "run",
          "--cwd",
          resolve(runtimeProcess.cwd(), options.cwd ?? "."),
          "--",
          script,
        ],
        shell: true,
      };
}

function shellOutput(rendered, terminal, modes) {
  if (terminal.tag !== "ok" || rendered.exitCode !== 0) return rendered;
  try {
    // The host renders typed results with capability redaction. Bash has no
    // output streams: its four-field result carries both channels and status.
    const result = JSON.parse(Buffer.from(rendered.stdout).toString("utf8"));
    if (
      !result ||
      Object.keys(result).length !== 4 ||
      typeof result.stdout !== "string" ||
      typeof result.stderr !== "string" ||
      typeof result.cwd !== "string" ||
      !Number.isInteger(result.exit_code) ||
      result.exit_code < 0 ||
      result.exit_code > 255
    )
      throw new TypeError("Invalid BashResult");
    return {
      exitCode: result.exit_code,
      stdout:
        modes[1] === "ignore" ? Buffer.alloc(0) : Buffer.from(result.stdout),
      stderr:
        modes[2] === "ignore" ? Buffer.alloc(0) : Buffer.from(result.stderr),
    };
  } catch {
    // A malformed command result is a command failure, not a failed spawn.
    return {
      exitCode: 1,
      stdout: Buffer.alloc(0),
      stderr: Buffer.from("bash: invalid shell result\n"),
    };
  }
}

function project(
  command,
  request,
  collected,
  maxBuffer,
  method = "spawnSync",
  shell = false,
  modes = PIPE_STDIO,
) {
  const output = request.renderResult(collected.result);
  const rendered = shell
    ? shellOutput(output, collected.result, modes)
    : output;
  const result = bounded(
    command,
    {
      pid: 1,
      status: rendered.exitCode,
      signal: null,
      stdout:
        modes[1] === "ignore"
          ? Buffer.alloc(0)
          : Buffer.concat([
              Buffer.from(collected.stdout?.bytes ?? []),
              Buffer.from(rendered.stdout),
            ]),
      stderr:
        modes[2] === "ignore"
          ? Buffer.alloc(0)
          : Buffer.concat([
              Buffer.from(collected.stderr?.bytes ?? []),
              Buffer.from(rendered.stderr),
            ]),
    },
    maxBuffer,
    method,
    modes,
  );
  for (const [index, channel] of [
    [1, "stdout"],
    [2, "stderr"],
  ]) {
    if (modes[index] === "ignore") continue;
    const remaining = maxBuffer - (collected[channel]?.forwarded ?? 0);
    const overflow = result[channel].length > remaining;
    if (overflow) result[channel] = result[channel].subarray(0, remaining);
    if (collected[channel]?.overflow || overflow) {
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

function deadlineCancelled(collected) {
  return (
    collected.timedOut &&
    collected.result.tag === "err" &&
    collected.result.val.tag === "cancelled" &&
    [collected.stdout, collected.stderr].every(
      (value) =>
        !value?.overflow &&
        (!value?.failure || value.failure.tag === "cancelled"),
    )
  );
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
      // The terminal result (or an unused source drop) closes the consumer.
      // Observe that closure before deciding whether stdin needs a terminal.
      const [closure] = await Promise.allSettled([watched]);
      const stopped = stream.closed
        ? Promise.resolve()
        : new Promise((resolve) => stream.once("close", resolve));
      stream.destroy();
      await stopped;
      await io;
      if (closure.status === "rejected") throw closure.reason;
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

function validateTimeout(timeout) {
  if (
    timeout !== undefined &&
    (!Number.isSafeInteger(timeout) || timeout < 0)
  ) {
    throw new RangeError("timeout must be a non-negative safe integer");
  }
}

function spawnTool(host, runtimeProcess, command, args, options) {
  const enabled = runtimeProcess.env.GOLEM_NODE_TOOL_PROCESSES;
  if (enabled === "0" || enabled === "false") return undefined;
  const maxBuffer = options.maxBuffer ?? DEFAULT_MAX_BUFFER;
  if (!Number.isSafeInteger(maxBuffer) || maxBuffer < 0)
    throw new RangeError("maxBuffer must be a non-negative safe integer");
  validateTimeout(options.timeout);
  const defaultSignal = killSignal(options.killSignal ?? "SIGTERM");
  const modes = stdioModes(runtimeProcess, options);
  if (
    options.signal !== undefined &&
    (typeof options.signal?.aborted !== "boolean" ||
      typeof options.signal?.addEventListener !== "function" ||
      typeof options.signal?.removeEventListener !== "function")
  ) {
    throw new TypeError("signal must be an AbortSignal");
  }
  const target = invocation(runtimeProcess, command, args, options);
  const prepared = host.prepareCommand(
    target.name,
    target.args,
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
  child.stdout = modes[1] === "pipe" ? new PassThrough() : null;
  child.stderr = modes[2] === "pipe" ? new PassThrough() : null;
  child.stdin = null;
  const resources = new Set();
  const transfer = (resource) => {
    resources.delete(resource);
    return resource;
  };
  let future;
  let input;
  let parentInput;
  let completed = false;
  let terminal;
  let signal;
  let timer;
  let abortError;
  let stdinError;
  let outputError;
  const outputWrites = new Set();
  let cancellationRequested = false;
  let started = prepared.tag === "output";
  let resolveStartup;
  const startup = new Promise((resolve) => {
    resolveStartup = resolve;
  });
  const cancel = () => {
    if (!future || cancellationRequested) return;
    try {
      future.cancel();
      cancellationRequested = true;
    } finally {
      // Flush owned pipes so admitted writes settle before the process closes.
      child.stdout?.resume();
      child.stderr?.resume();
    }
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
    if (requested === 0) return started && !completed && !!future;
    const normalized = killSignal(requested);
    if (completed || !started || !future) return false;
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
        modes,
      );
    } else {
      const request = prepared.val;
      const spec = request.input();
      let source;
      if (spec.stdin) {
        if (modes[0] === "ignore") {
          source = host.createStdinFromBytes(Buffer.alloc(0));
          resources.add(source);
        } else {
          if (
            modes[0] === "inherit" &&
            (typeof runtimeProcess.stdin?.pipe !== "function" ||
              typeof runtimeProcess.stdin?.unpipe !== "function")
          ) {
            throw processError(
              "ENOTSUP",
              command,
              "inherited stdin must be a readable process stream",
              "spawn",
            );
          }
          input = createInput(host, command, resources, inputFailed);
          source = input.source;
          if (modes[0] === "pipe") child.stdin = input.stream;
          else {
            parentInput = runtimeProcess.stdin;
            parentInput.on("error", inputFailed);
            parentInput.pipe(input.stream);
            if (stdinError) throw stdinError;
          }
        }
      }
      const output = (wanted, mode) => {
        if (!wanted) return undefined;
        if (mode === "ignore") {
          const target = host.createDiscardedOutput();
          resources.add(target);
          return [target];
        }
        const pair = host.createCollectedOutput(BigInt(maxBuffer));
        for (const resource of pair) resources.add(resource);
        return pair;
      };
      const stdout = output(spec.stdout, modes[1]);
      const stderr = output(spec.stderr, modes[2]);
      const rpc = new host.ToolRpc(target.name);
      resources.add(rpc);
      future = rpc.asyncInvokeAndAwait(
        spec.commandPath,
        spec.input,
        transfer(source),
        transfer(stdout?.[0]),
        transfer(stderr?.[0]),
      );
      resources.add(future);
      collection = future.collectAsync(
        transfer(stdout?.[1]),
        transfer(stderr?.[1]),
        undefined,
        async (channel, bytes) => {
          await startup;
          if (!started) return;
          const index = channel === "stdout" ? 1 : 2;
          const destination =
            modes[index] === "inherit"
              ? runtimeProcess[channel]
              : child[channel];
          if (!destination) return;
          const writing = new Promise((resolve, reject) =>
            destination.write(bytes, (error) =>
              error ? reject(error) : resolve(),
            ),
          );
          if (modes[index] === "inherit") {
            await writing;
          } else {
            // These pipes are bounded by maxBuffer. Let collection reach the
            // terminal even if the caller only starts reading at exit.
            outputWrites.add(writing);
            writing.then(
              () => outputWrites.delete(writing),
              (error) => {
                outputWrites.delete(writing);
                outputError ??= error;
                try {
                  cancel();
                } catch (failure) {
                  outputError.cause ??= failure;
                }
              },
            );
          }
        },
      );
      // This marks a local process handle, not remote permission or execution.
      started = true;
    }
    if (future && started && options.signal) {
      options.signal.addEventListener("abort", abort, { once: true });
      if (options.signal.aborted) abort();
    }
    if (future && started && options.timeout > 0) {
      timer = setTimeout(() => child.kill(defaultSignal), options.timeout);
    }
  } catch (error) {
    started = false;
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
  if (started) child.pid = 1;
  nextTick(() => {
    try {
      if (started) child.emit("spawn");
    } finally {
      resolveStartup();
    }
  });
  // Collection starts at dispatch. Observe failures until startup has delivered
  // its event; the completion task still awaits the original result below.
  collection?.catch(() => {});
  startup.then(async () => {
    try {
      if (collection) {
        const collected = await collection;
        terminal ??= project(
          command,
          prepared.val,
          collected,
          maxBuffer,
          "spawn",
          target.shell,
          modes,
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
      for (const cleanup of [
        () => options.signal?.removeEventListener("abort", abort),
        () => parentInput?.unpipe(input.stream),
        () => parentInput?.removeListener("error", inputFailed),
        () => {
          if (input && resources.has(input.source))
            dispose(transfer(input.source));
        },
      ]) {
        try {
          cleanup();
        } catch (error) {
          cleanupFailed(error);
        }
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
    try {
      await routeOutputAsync(runtimeProcess, terminal, modes);
    } catch (error) {
      terminal.error ??= processError(
        "EIO",
        command,
        "inherited output failed",
        "spawn",
      );
      terminal.error.cause ??= error;
      terminal.status = null;
    }
    child.exitCode = terminal.status;
    child.signalCode = terminal.signal;
    const outputCompletion = Promise.allSettled([
      ...outputWrites,
      ...["stdout", "stderr"].map(async (channel) => {
        const stream = child[channel];
        if (!stream) return;
        const completion = finished(stream, { cleanup: true });
        try {
          stream.end(terminal[channel]);
        } catch (error) {
          stream.destroy(error);
        }
        await completion;
      }),
    ]);
    if (terminal.error) child.emit("error", terminal.error);
    else child.emit("exit", terminal.status, terminal.signal);
    child.stdout?.resume();
    child.stderr?.resume();
    const outputFailure = (await outputCompletion).find(
      (result) => result.status === "rejected",
    );
    if ((outputFailure || outputError) && !terminal.error) {
      terminal.error = processError(
        "EIO",
        command,
        "output finalization failed",
        "spawn",
      );
      terminal.error.cause = outputFailure?.reason ?? outputError;
      terminal.status = null;
      child.exitCode = null;
      child.emit("error", terminal.error);
    }
    child.emit("close", terminal.status, terminal.signal);
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
      validateTimeout(options.timeout);
      const timeoutSignal = killSignal(options.killSignal ?? "SIGTERM");
      const maxBuffer = options.maxBuffer ?? DEFAULT_MAX_BUFFER;
      if (!Number.isSafeInteger(maxBuffer) || maxBuffer < 0) {
        throw new RangeError("maxBuffer must be a non-negative safe integer");
      }
      const inputBytes = finiteInput(options.input, options.encoding);
      const modes = stdioModes(runtimeProcess, options);
      const target = invocation(runtimeProcess, command, args, options);
      const prepared = host.prepareCommand(
        target.name,
        target.args,
        variables(runtimeProcess, options),
      );
      if (prepared === undefined) return undefined;
      if (prepared.tag === "output") {
        return routeOutput(
          runtimeProcess,
          bounded(
            command,
            {
              pid: 1,
              status: prepared.val.exitCode,
              signal: null,
              stdout: prepared.val.stdout,
              stderr: prepared.val.stderr,
            },
            maxBuffer,
            "spawnSync",
            modes,
          ),
          modes,
          command,
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
        if (spec.stdin && modes[0] === "inherit") {
          throw processError(
            "ENOTSUP",
            command,
            "synchronous inherited stdin needs a host-owned source",
          );
        }
        const input = spec.stdin
          ? host.createStdinFromBytes(
              modes[0] === "ignore" ? Buffer.alloc(0) : inputBytes,
            )
          : undefined;
        if (input) resources.add(input);
        const output = (wanted, mode) => {
          if (!wanted) return undefined;
          const pair =
            mode === "ignore"
              ? [host.createDiscardedOutput()]
              : host.createCollectedOutput(BigInt(maxBuffer));
          for (const resource of pair) resources.add(resource);
          return pair;
        };
        const stdout = output(spec.stdout, modes[1]);
        const stderr = output(spec.stderr, modes[2]);
        const rpc = new host.ToolRpc(target.name);
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
          options.timeout > 0 ? BigInt(options.timeout) : undefined,
        );
        completed = true;
        const result = project(
          command,
          request,
          collected,
          maxBuffer,
          "spawnSync",
          target.shell,
          modes,
        );
        if (deadlineCancelled(collected) && result.error?.code !== "ENOBUFS") {
          result.pid = 1;
          result.status = null;
          result.signal = timeoutSignal;
          result.error = processError(
            "ETIMEDOUT",
            command,
            "deadline exceeded",
          );
        }
        return routeOutput(runtimeProcess, result, modes, command);
      } catch (error) {
        const code =
          error?.code ??
          (error?.tag === "resource-exhausted" ? "ENOMEM" : "EIO");
        return routeOutput(
          runtimeProcess,
          failed(
            command,
            code,
            error instanceof Error ? error.message : "tool process failed",
          ),
          modes,
          command,
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
