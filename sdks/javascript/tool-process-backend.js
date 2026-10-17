// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { Buffer } from "node:buffer";
import { resolve } from "node:path";

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

function processError(code, command, message) {
  return Object.assign(new Error(`spawnSync ${command}: ${message}`), {
    code,
    syscall: `spawnSync ${command}`,
    path: command,
  });
}

function failed(command, code, message) {
  return {
    pid: 0,
    status: null,
    signal: null,
    stdout: Buffer.alloc(0),
    stderr: Buffer.alloc(0),
    error: processError(code, command, message),
  };
}

function dispose(resource) {
  resource?.[Symbol.dispose]?.();
}

function bounded(command, result, maxBuffer) {
  for (const channel of ["stdout", "stderr"]) {
    result[channel] = Buffer.from(result[channel] ?? []);
    if (result[channel].length > maxBuffer) {
      result[channel] = result[channel].subarray(0, maxBuffer);
      result.error ??= processError(
        "ENOBUFS",
        command,
        `${channel} maxBuffer exceeded`,
      );
      result.status = null;
    }
  }
  return result;
}

/// Constructing the backend performs no discovery, host I/O or authority lookup.
/// The host facade is explicit so this module is shared by all JavaScript SDKs.
export function createToolProcessBackend(host, runtimeProcess) {
  return {
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
      const variables = Object.entries(options.env ?? runtimeProcess.env)
        .filter(([, value]) => value !== undefined)
        .map(([key, value]) => [key, String(value)]);
      const cwd = resolve(runtimeProcess.cwd(), options.cwd ?? ".");
      const withoutPwd = variables.filter(([key]) => key !== "PWD");
      withoutPwd.push(["PWD", cwd]);

      const prepared = host.prepareCommand(
        command,
        args.map(String),
        withoutPwd,
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
        const projected = request.renderResult(collected.result);
        const result = bounded(
          command,
          {
            pid: 1,
            status: projected.exitCode,
            signal: null,
            stdout: Buffer.concat([
              Buffer.from(collected.stdout?.bytes ?? []),
              Buffer.from(projected.stdout),
            ]),
            stderr: Buffer.concat([
              Buffer.from(collected.stderr?.bytes ?? []),
              Buffer.from(projected.stderr),
            ]),
          },
          maxBuffer,
        );
        if (
          collected.bodyExecuted === false &&
          collected.result.tag === "err"
        ) {
          const error = collected.result.val;
          const code = {
            denied: "EACCES",
            "not-found": "ENOENT",
            "resource-exhausted": "ENOMEM",
          }[error.tag];
          if (code) {
            result.error ??= processError(code, command, error.tag);
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
            );
            result.status = null;
          } else if (collected[channel]?.failure && !result.error) {
            result.error = processError(
              "EIO",
              command,
              `${channel} stream failed`,
            );
            result.status = null;
          }
        }
        return result;
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
