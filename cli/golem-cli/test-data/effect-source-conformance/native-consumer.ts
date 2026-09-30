import { Effect, Schema, Stream } from "effect";
import { defineAgent, method } from "@golemcloud/effect-golem";
import { client as native } from "native-conformance-tool-guest-client";

const collect = (
  stream: Stream.Stream<Uint8Array, unknown, never> | undefined,
) =>
  stream
    ? stream.pipe(
        Stream.runCollect,
        Effect.map((chunks) => [...chunks].flatMap((chunk) => [...chunk])),
      )
    : Effect.succeed([] as number[]);

const stringify = (value: unknown): string =>
  JSON.stringify(value, (_key, entry) =>
    typeof entry === "bigint" ? Number(entry) : entry,
  );

defineAgent({
  name: "EffectNativeConsumer",
  id: { name: Schema.String },
  methods: { run: method({ input: {}, success: Schema.String }) },
}).implement({
  init: ({ name }) => Effect.succeed(name),
  methods: () => ({
    run: (_input) =>
      Effect.scoped(
        Effect.gen(function* () {
          const structured = yield* native.structured("alpha", 7n);
          const supportedError = yield* native
            .supported_error("expected")
            .pipe(Effect.flip);
          const finiteStream = yield* native.finite_stream("payload");
          const [finiteResult, stdout] = yield* Effect.all(
            [finiteStream.result, collect(finiteStream.stdout)],
            { concurrency: "unbounded" },
          );
          const middleware = yield* native.middleware("input");

          return stringify({
            structured,
            supportedError,
            finiteResult,
            stdout,
            middleware,
          });
        }),
      ).pipe(Effect.orDie),
  }),
});

defineAgent({
  name: "UnauthorizedEffectNativeConsumer",
  id: { name: Schema.String },
  methods: { run: method({ input: {}, success: Schema.String }) },
}).implement({
  init: ({ name }) => Effect.succeed(name),
  methods: () => ({
    run: (_input) =>
      Effect.scoped(native.structured("denied", 0n)).pipe(
        Effect.flip,
        Effect.map(stringify),
        Effect.orDie,
      ),
  }),
});
