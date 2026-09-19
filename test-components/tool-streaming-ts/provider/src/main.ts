import {
  ok,
  s,
  toolDefinition,
  ToolStreamError,
} from "@golemcloud/golem-ts-sdk";
import { z } from "zod/v4";

toolDefinition("ts-streaming")
  .body((body) =>
    body
      .positional("mode", z.string())
      .stdin({ required: true })
      .stdout({ required: true })
      .returns(s.u64()),
  )
  .implement({
    "ts-streaming": async ({ mode }, context) => {
      const reader = context.stdin.getReader();
      const writer = context.stdout.getWriter();
      let bytesRead = 0n;

      if (mode === "resource-exhausted") {
        await writer.abort(new ToolStreamError({ tag: "resource-exhausted" }));
        return ok(bytesRead);
      }

      if (mode === "marker-echo") {
        await writer.write(new Uint8Array());
        await writer.write(new TextEncoder().encode("ts-marker:"));
      }

      while (true) {
        const item = await reader.read();
        if (item.done) break;
        bytesRead += BigInt(item.value.byteLength);
        await writer.write(item.value);
      }

      return ok(bytesRead);
    },
  });
