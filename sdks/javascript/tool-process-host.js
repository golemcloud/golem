// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { Buffer } from "node:buffer";

// The native bridge owns the WIT resources. These objects are invocation-local
// adapters, not additional methods on golem:tool/host.
export function createToolProcessHost(native) {
  const resource = (value, release) => ({
    native: value,
    [Symbol.dispose]() {
      if (this.native === undefined) return;
      release?.(this.native);
      this.native = undefined;
    },
  });
  const input = (bytes) => native.createInput(bytes);
  const output = (limit) => {
    const value = native.createOutput(limit);
    return [
      Object.assign(
        resource(value, (value) => native.disposeOutput(value, true)),
        { limit },
      ),
      resource(value, (value) => native.disposeOutput(value, false)),
    ];
  };
  return {
    prepareCommand(name, args, variables) {
      const prepared = native.prepareCommand(name, args, variables);
      if (prepared?.tag !== "invoke") return prepared;
      const spec = prepared.val;
      return {
        tag: "invoke",
        val: {
          input: () => spec,
          renderResult: (terminal) => terminal.rendered,
          [Symbol.dispose]() {},
        },
      };
    },
    createStdinFromBytes(bytes) {
      return resource(input(bytes), (value) =>
        native.disposeInput(value, "source"),
      );
    },
    createStdin() {
      const value = input();
      return [
        Object.assign(
          resource(value, (value) => native.disposeInput(value, "writer")),
          {
            write: (bytes) => native.write(value, bytes),
            finish: () => native.finish(value, false),
            fail: () => native.finish(value, true),
          },
        ),
        resource(value, (value) => native.disposeInput(value, "source")),
        Object.assign(
          resource(value, (value) => native.disposeInput(value, "closed")),
          {
            wait: () => native.waitInputClosed(value),
          },
        ),
      ];
    },
    createCollectedOutput: (limit) => output(Number(limit)),
    createDiscardedOutput: () => output()[0],
    ToolRpc: class {
      [Symbol.dispose]() {}
      asyncInvokeAndAwait(_path, prepared, stdin, stdout, stderr) {
        let value;
        try {
          value = native.invoke(
            prepared,
            stdin?.native,
            stdout?.native,
            stderr?.native,
          );
        } catch (error) {
          for (const attachment of [stdin, stdout, stderr]) {
            try {
              attachment?.[Symbol.dispose]();
            } catch (cleanup) {
              if (error instanceof Error) error.cause ??= cleanup;
            }
          }
          throw error;
        }
        let stopped = false;
        const cancel = () => {
          try {
            native.cancel(value);
          } finally {
            stopped = true;
          }
        };
        const drain = async (target, channel, onOutput) => {
          const result = { bytes: [], overflow: false, forwarded: 0 };
          if (!target) return result;
          const chunks = [];
          let length = 0;
          try {
            while (!stopped) {
              const chunk = await native.readOutput(target.native);
              if (chunk === undefined) break;
              if (target.limit === undefined) continue;
              const retained = chunk.subarray(0, target.limit - length);
              length += retained.length;
              if (retained.length !== chunk.length) {
                result.overflow = true;
                cancel();
              }
              if (retained.length) {
                if (onOutput) {
                  result.forwarded += retained.length;
                  await onOutput(channel, retained);
                } else chunks.push(Buffer.from(retained));
              }
              if (result.overflow) break;
            }
          } catch (error) {
            result.failure = {
              tag: error?.tag === "cancelled" ? "cancelled" : "failed",
            };
            cancel();
          } finally {
            native.disposeOutput(target.native, false);
          }
          result.bytes = Buffer.concat(chunks);
          return result;
        };
        return Object.assign(
          resource(value, (value) => native.disposeProcess(value)),
          {
            cancel,
            collect: (_stdout, _stderr, timeout) =>
              native.collectSync(
                value,
                timeout === undefined ? undefined : Number(timeout),
              ),
            async collectAsync(_stdout, _stderr, _timeout, onOutput) {
              const terminal = Promise.resolve()
                .then(() => native.collectAsync(value))
                .catch((error) => {
                  cancel();
                  throw error;
                });
              const results = await Promise.allSettled([
                terminal,
                drain(stdout, "stdout", onOutput),
                drain(stderr, "stderr", onOutput),
              ]);
              const failure = results.find(
                (result) => result.status === "rejected",
              );
              if (failure) throw failure.reason;
              return {
                ...results[0].value,
                stdout: results[1].value,
                stderr: results[2].value,
              };
            },
          },
        );
      }
    },
  };
}
