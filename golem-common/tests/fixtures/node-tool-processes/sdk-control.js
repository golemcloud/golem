// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { getReflectedToolType } from "../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs";
import { PassThrough } from "node:stream";
import { finished } from "node:stream/promises";
import { nextTick } from "node:process";

export {
  getReflectedToolType,
  golemTool010Guest,
  ok,
  processExitStatusSchema,
  toolDefinition,
} from "../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs";
export {
  guest as golemAgent200Guest,
  toolMiddlewareGuest,
  saveSnapshot,
  loadSnapshot,
} from "../../../../sdks/ts/packages/golem-ts-sdk/dist/runtime/emptyGuest.mjs";
export { z } from "../../../../sdks/ts/packages/golem-ts-sdk/node_modules/zod/v4/index.js";

export async function sdkOverflowControl() {
  const command = getReflectedToolType("streaming")?.client.command([
    "produce",
  ]);
  if (!command) throw new Error("SDK control tool not found");
  const input = Object.fromEntries(
    command.arguments.map(({ name }) => {
      switch (name.replace(/[-_]/g, "").toLowerCase()) {
        case "chunkcount":
          return [name, 512];
        case "chunksize":
          return [name, 65536];
        default:
          throw new Error(`Unexpected producer argument: ${name}`);
      }
    }),
  );
  const invocation = command.startJson(input);
  const stdout = new PassThrough();
  const stderr = new PassThrough();
  let length = 0;
  stdout.on("data", (bytes) => {
    length += bytes.length;
  });
  stderr.resume();
  const startup = new Promise((resolve) => nextTick(resolve));
  const outputClosed = Promise.all([finished(stdout), finished(stderr)]);
  outputClosed.catch(() => {});
  let writing;
  const result = invocation.result.then(
    (value) => ({ value }),
    (error) => ({ error }),
  );
  const reader = invocation.stdout.getReader();
  const stop = async () => {
    try {
      await reader.cancel();
    } catch (error) {
      if (error?.failure?.tag !== "cancelled") throw error;
    }
  };
  try {
    const first = await reader.read();
    if (first.done || first.value.length <= 4096)
      throw new Error("SDK producer did not exceed the output limit");
    await startup;
    writing = new Promise((resolve, reject) => {
      stdout.write(first.value.subarray(0, 4096), (error) =>
        error ? reject(error) : resolve(),
      );
    });
    writing.catch(() => {});
    invocation.cancel();
    await stop();
    const terminal = await result;
    if (
      terminal.error?.cause?.tag !== "rpc" ||
      terminal.error.cause.error.tag !== "cancelled"
    )
      throw new Error("SDK producer cancellation did not settle");
    stdout.end();
    stderr.end();
    stdout.resume();
    await Promise.all([writing, outputClosed]);
    return `overflow:${length}`;
  } finally {
    invocation.cancel();
    try {
      await stop();
    } finally {
      reader.releaseLock();
      await result;
      stdout.destroy();
      stderr.destroy();
      await Promise.allSettled([writing, outputClosed]);
    }
  }
}
