import { Effect, Option, Ref, Result, Schema, Stream } from "effect";
import {
  defineAgent,
  defineConfig,
  DurableStreams,
  Http,
  method,
  WitTypes,
} from "@golemcloud/effect-golem";

class StreamingConfig extends defineConfig("StreamingAgent.Config", {
  externalAuth: Schema.Redacted(Schema.String),
}) {}

export const StreamingAgent = defineAgent({
  name: "StreamingAgent",
  id: { name: Schema.String },
  config: StreamingConfig,
  http: Http.mount("/durable-stream-agents/{name}"),
  methods: {
    sum: method({
      input: { input: WitTypes.AgentStream(Schema.Number) },
      success: Schema.Number,
    }),
    produce: method({ input: {}, success: WitTypes.AgentStream(Schema.Number) }),
    transform: method({
      input: {
        prefix: Schema.String,
        input: WitTypes.AgentStream(Schema.Number),
      },
      success: WitTypes.AgentStream(Schema.String),
    }),
    nested: method({
      input: {},
      success: WitTypes.AgentStream(WitTypes.AgentStream(Schema.Number)),
    }),
    recoverable: method({
      input: {},
      success: WitTypes.AgentStream(Schema.Result(Schema.Number, Schema.String)),
    }),
    status: method({ input: {}, success: Schema.String }),
    durableEcho: method({
      input: { input: WitTypes.AgentStream(Schema.String) },
      success: WitTypes.AgentStream(Schema.String),
      http: [
        Http.put("/echo", {
          durableStreams: {
            slots: [
              { source: "input", slot: "input" },
              { source: "output", slot: "$result" },
            ],
            allowExternalWrites: true,
          },
        }),
      ],
    }),
    appendExternal: method({
      input: {
        url: Schema.String,
        producerId: Schema.String,
        values: Schema.Array(Schema.String),
        close: Schema.Boolean,
      },
      success: Schema.Option(Schema.String),
    }),
    readExternal: method({
      input: { url: Schema.String },
      success: Schema.Array(Schema.String),
    }),
  },
}).implement({
  init: () => Ref.make(0),
  methods: (cancelledProducers) => ({
    sum: ({ input }) =>
      input.pipe(
        Stream.runFold(
          () => 0,
          (total, value) => total + value,
        ),
        Effect.orDie,
      ),
    produce: () => Effect.succeed(Stream.make(1, 2, 3)),
    transform: ({ prefix, input }) =>
      Effect.succeed(input.pipe(Stream.map((value) => `${prefix}:${value}`))),
    nested: () =>
      Effect.succeed(Stream.make(Stream.make(10, 20), Stream.make(30, 40))),
    recoverable: () =>
      Effect.succeed(
        Stream.make(
          Result.succeed(1),
          Result.fail("this item could not be produced"),
          Result.succeed(2),
        ),
      ),
    status: () =>
      Ref.get(cancelledProducers).pipe(
        Effect.map((count) => `ready (${count} cancelled producers)`),
      ),
    durableEcho: ({ input }) =>
      Effect.succeed(input.pipe(Stream.map((value) => `echo:${value}`))),
    appendExternal: ({ url, producerId, values, close }) =>
      Effect.gen(function* () {
        const config = yield* StreamingConfig;
        const auth = yield* config.externalAuth.borrow;
        const writer = yield* DurableStreams.makeJsonWriter(Schema.String, {
          url,
          producerId,
          auth,
        });
        const receipt = yield* writer.append(values, { close });
        return Option.fromUndefinedOr(receipt.nextOffset);
      }).pipe(Effect.orDie, Effect.scoped),
    readExternal: ({ url }) =>
      Effect.gen(function* () {
        const config = yield* StreamingConfig;
        const auth = yield* config.externalAuth.borrow;
        const values = yield* DurableStreams.readJson(Schema.String, {
          url,
          auth,
        }).pipe(Stream.runCollect);
        return Array.from(values);
      }).pipe(Effect.orDie, Effect.scoped),
  }),
});
