import {
  client,
  defineAgent,
  method,
  s,
  toolDefinition,
} from "@golemcloud/golem-ts-sdk";
import { z } from "zod/v4";

const streamingTool = toolDefinition("ts-streaming").body((body) =>
  body
    .positional("mode", z.string())
    .stdin({ required: true })
    .stdout({ required: true })
    .returns(s.u64()),
);

const Evidence = z.object({
  output: s.bytes(),
  bytesRead: s.u64(),
});

const Caller = defineAgent({
  name: "TsToolStreamingCaller",
  id: { name: z.string() },
  methods: {
    markerBeforeEof: method({
      input: { payload: s.bytes() },
      returns: Evidence,
    }),
  },
});

Caller.implement({
  init: () => ({}),
  methods: {
    async markerBeforeEof({ payload }) {
      let releaseInput!: () => void;
      const inputGate = new Promise<void>((resolve) => {
        releaseInput = resolve;
      });
      const stdin = new ReadableStream<Uint8Array>({
        async start(controller) {
          await inputGate;
          if (payload.byteLength > 0) controller.enqueue(payload);
          controller.close();
        },
      });
      const invocation = client(streamingTool)["ts-streaming"]({
        mode: "marker-echo",
        stdin,
      });
      const reader = invocation.stdout.getReader();
      const marker = await reader.read();
      if (
        marker.done ||
        new TextDecoder().decode(marker.value) !== "ts-marker:"
      ) {
        throw new Error(
          "TypeScript tool stdout marker was not live before stdin EOF",
        );
      }

      releaseInput();
      const chunks = [marker.value];
      const [bytesRead] = await Promise.all([
        invocation.result,
        (async () => {
          while (true) {
            const item = await reader.read();
            if (item.done) break;
            chunks.push(item.value);
          }
        })(),
      ]);
      const output = new Uint8Array(
        chunks.reduce((size, chunk) => size + chunk.byteLength, 0),
      );
      let offset = 0;
      for (const chunk of chunks) {
        output.set(chunk, offset);
        offset += chunk.byteLength;
      }
      return { output, bytesRead };
    },
  },
});
