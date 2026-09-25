import {
  defineAgent,
  method,
  s,
  ToolStreamError,
} from "@golemcloud/golem-ts-sdk";
import { TsStreamingClient } from "ts-streaming-tool-guest-client";
import { z } from "zod/v4";

const Evidence = z.object({
  output: s.bytes(),
  bytesRead: s.u64(),
});

const CompletionEvidence = z.object({
  output: s.bytes(),
  stdoutTerminal: z.string(),
  resultTerminal: z.string(),
});

const Caller = defineAgent({
  name: "TsToolStreamingCaller",
  id: { name: z.string() },
  methods: {
    markerBeforeEof: method({
      input: { payload: s.bytes() },
      returns: Evidence,
    }),
    typedStdoutFailure: method({
      input: {},
      returns: z.string(),
    }),
    declaredErrorCompletion: method({
      input: {},
      returns: CompletionEvidence,
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
          controller.enqueue(new Uint8Array());
          if (payload.byteLength > 0) controller.enqueue(payload);
          controller.close();
        },
      });
      const invocation = TsStreamingClient.newClient().ts_streaming(
        "marker-echo",
        stdin,
      );
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
    async typedStdoutFailure() {
      const stdin = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.close();
        },
      });
      const invocation = TsStreamingClient.newClient().ts_streaming(
        "resource-exhausted",
        stdin,
      );
      const [result, stdout] = await Promise.allSettled([
        invocation.result,
        invocation.stdout.getReader().read(),
      ]);
      if (result.status === "rejected") throw result.reason;
      if (result.value !== 0n) {
        throw new Error(`Unexpected structured result ${result.value}`);
      }
      if (stdout.status === "fulfilled") {
        throw new Error("Expected typed stdout failure");
      }
      if (!(stdout.reason instanceof ToolStreamError)) throw stdout.reason;
      return stdout.reason.failure.tag;
    },
    async declaredErrorCompletion() {
      const stdin = new ReadableStream<Uint8Array>({
        start(controller) {
          controller.close();
        },
      });
      const invocation = TsStreamingClient.newClient().ts_streaming(
        "declared-error",
        stdin,
      );
      const chunks: Uint8Array[] = [];
      const reader = invocation.stdout.getReader();
      const [result, stdout] = await Promise.allSettled([
        invocation.result,
        (async () => {
          while (true) {
            const item = await reader.read();
            if (item.done) return "finished";
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
      return {
        output,
        stdoutTerminal: stdout.status === "fulfilled" ? stdout.value : "failed",
        resultTerminal: result.status === "rejected" ? "declared-error" : "ok",
      };
    },
  },
});
