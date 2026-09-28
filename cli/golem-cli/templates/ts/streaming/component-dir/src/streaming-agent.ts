import { z } from "zod";
import {
  AgentStream,
  createDurableJsonWriter,
  defineAgent,
  http,
  method,
  readDurableJsonStream,
  Result,
  s,
} from "@golemcloud/golem-ts-sdk";

export const StreamingAgent = defineAgent({
  name: "StreamingAgent",
  id: { name: z.string() },
  config: { externalAuth: s.secret(z.string()) },
  http: http.mount("/durable-stream-agents/{name}"),
  methods: {
    sum: method({
      input: { input: s.stream(z.number()) },
      returns: z.number(),
    }),
    produce: method({ input: {}, returns: s.stream(z.number()) }),
    transform: method({
      input: { prefix: z.string(), input: s.stream(z.number()) },
      returns: s.stream(z.string()),
    }),
    nested: method({ input: {}, returns: s.stream(s.stream(z.number())) }),
    recoverable: method({
      input: {},
      returns: s.stream(s.result(z.number(), z.string())),
    }),
    status: method({ input: {}, returns: z.string() }),
    durableEcho: method({
      input: { input: s.stream(z.string()) },
      returns: s.stream(z.string()),
      http: http.put("/echo", {
        durableStreams: {
          slots: [
            { source: "input", slot: "input" },
            { source: "output", slot: "$result" },
          ],
          allowExternalWrites: true,
        },
      }),
    }),
    appendExternal: method({
      input: {
        url: z.string(),
        producerId: z.string(),
        values: z.array(z.string()),
        close: z.boolean(),
      },
      returns: z.string().optional(),
    }),
    readExternal: method({
      input: { url: z.string() },
      returns: z.array(z.string()),
    }),
  },
});

function stream<T>(values: Iterable<T>, onCancel: () => void): AgentStream<T> {
  return AgentStream.from(
    (async function* () {
      let completed = false;
      try {
        for (const value of values) yield value;
        completed = true;
      } finally {
        // A remote consumer that stops early closes this iterator.
        if (!completed) onCancel();
      }
    })(),
  );
}

export const StreamingAgentImpl = StreamingAgent.implement({
  init: () => ({ cancelledProducers: 0 }),
  methods: {
    async sum({ input }) {
      let total = 0;
      for await (const value of input) total += value;
      return total;
    },
    produce() {
      return stream([1, 2, 3], () => this.cancelledProducers++);
    },
    transform({ prefix, input }) {
      const state = this;
      return AgentStream.from(
        (async function* () {
          let completed = false;
          try {
            for await (const value of input) yield `${prefix}:${value}`;
            completed = true;
          } finally {
            // Exiting early also closes `input`, propagating cancellation.
            if (!completed) state.cancelledProducers++;
          }
        })(),
      );
    },
    nested() {
      return stream(
        [
          stream([10, 20], () => this.cancelledProducers++),
          stream([30, 40], () => this.cancelledProducers++),
        ],
        () => this.cancelledProducers++,
      );
    },
    recoverable() {
      return stream<Result<number, string>>(
        [
          Result.ok(1),
          Result.err("this item could not be produced"),
          Result.ok(2),
        ],
        () => this.cancelledProducers++,
      );
    },
    status() {
      return `ready (${this.cancelledProducers} cancelled producers)`;
    },
    durableEcho({ input }) {
      return AgentStream.from(
        (async function* () {
          for await (const value of input) yield `echo:${value}`;
        })(),
      );
    },
    async appendExternal({ url, producerId, values, close }) {
      const writer = createDurableJsonWriter(z.string(), {
        url,
        producerId,
        auth: this.config.externalAuth,
      });
      try {
        return (await writer.append(values, { close })).nextOffset;
      } finally {
        await writer.dispose();
      }
    },
    async readExternal({ url }) {
      const values: string[] = [];
      for await (const value of readDurableJsonStream(z.string(), {
        url,
        auth: this.config.externalAuth,
      })) {
        values.push(value);
      }
      return values;
    },
  },
});
