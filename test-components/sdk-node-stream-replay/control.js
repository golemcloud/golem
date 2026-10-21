// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (http://license.golem.cloud/LICENSE).

import { getReflectedToolType } from "../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs";
import { PassThrough } from "node:stream";
import { finished } from "node:stream/promises";

export {
  getReflectedToolType,
  golemTool010Guest,
  ok,
  processExitStatusSchema,
  toolDefinition,
} from "../../sdks/ts/packages/golem-ts-sdk/dist/runtime/index.mjs";
export {
  guest as golemAgent200Guest,
  toolMiddlewareGuest,
  saveSnapshot,
  loadSnapshot,
} from "../../sdks/ts/packages/golem-ts-sdk/dist/runtime/emptyGuest.mjs";
export { z } from "../../sdks/ts/packages/golem-ts-sdk/node_modules/zod/v4/index.js";

// One ordinary SDK call and one Node stream, with no process adapter. Three
// chunks keep the producer blocked behind the harness's one-chunk buffer after
// the reader consumes its first chunk. Cancellation is causal, not a timer race.
export async function sdkOverflowControl() {
  const command = getReflectedToolType("streaming").client.command(["produce"]);
  const input = Object.fromEntries(
    command.arguments.map(({ name }) => {
      switch (name.replace(/[-_]/g, "").toLowerCase()) {
        case "chunkcount":
          return [name, 3];
        case "chunksize":
          return [name, 65536];
        default:
          throw new Error(`Unexpected producer argument: ${name}`);
      }
    }),
  );
  const call = command.startJson(input);
  const stdout = new PassThrough();
  let length = 0;
  stdout.on("data", (bytes) => {
    length += bytes.length;
  });
  const closed = finished(stdout);
  closed.catch(() => {});
  const result = call.result.then(
    (value) => ({ value }),
    (error) => ({ error }),
  );
  const reader = call.stdout.getReader();
  const first = await reader.read();
  if (first.done || first.value.length <= 4096)
    throw new Error("SDK producer did not exceed the output limit");
  stdout.write(first.value.subarray(0, 4096));
  call.cancel();
  try {
    await reader.cancel();
  } catch (error) {
    if (error?.failure?.tag !== "cancelled") throw error;
  } finally {
    reader.releaseLock();
  }
  const terminal = await result;
  if (
    terminal.error?.cause?.tag !== "rpc" ||
    terminal.error.cause.error.tag !== "cancelled"
  )
    throw new Error("SDK producer cancellation did not settle");
  stdout.end();
  await closed;
  return `overflow:${length}`;
}
