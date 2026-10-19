import { Effect, Schema, Stream } from "effect";
import { defineAgent, method } from "@golemcloud/effect-golem";
import { client as catalog } from "catalog-lookup-tool-guest-client";

const collect = (
  stream: Stream.Stream<Uint8Array, unknown, never> | undefined,
) =>
  stream
    ? stream.pipe(
        Stream.runCollect,
        Effect.map((chunks) => [...chunks].flatMap((chunk) => [...chunk])),
      )
    : Effect.succeed([] as number[]);

const stringify = (value: unknown) =>
  JSON.stringify(value, (_key, item) =>
    typeof item === "bigint" ? Number(item) : item,
  );

const renderError = (error: unknown): string => {
  if (typeof error !== "object" || error === null) return String(error);
  const value = error as {
    tag?: string;
    error?: { name?: string; value?: unknown; tag?: string; val?: unknown };
  };
  return value.tag === "tool"
    ? `tool:mcp-tool-error:${String(value.error?.value)}`
    : `rpc:${value.error?.tag}:${stringify(value.error?.val)}`;
};

defineAgent({
  name: "EffectMcpConsumer",
  id: { name: Schema.String },
  methods: { run: method({ input: {}, success: Schema.String }) },
}).implement({
  init: ({ name }) => Effect.succeed(name),
  methods: () => ({
    run: (_input) =>
      Effect.scoped(
        Effect.gen(function* () {
          const streamed = yield* catalog.catalog_lookup(false, "streamed");
          const [streamedResult, stdout] = yield* Effect.all(
            [streamed.result, collect(streamed.stdout)],
            { concurrency: "unbounded" },
          );
          const blocks = yield* catalog.catalog_lookup(true, "blocks");
          const blocksResult = yield* blocks.result;
          const toolError = yield* catalog
            .catalog_lookup(false, "tool-error")
            .pipe(
              Effect.flatMap((invocation) => invocation.result),
              Effect.flip,
            );
          const invalid = yield* catalog.catalog_lookup(false, "invalid").pipe(
            Effect.flatMap((invocation) => invocation.result),
            Effect.flip,
          );
          return stringify({
            streamed: streamedResult,
            stdout,
            blocks: blocksResult,
            toolError: renderError(toolError),
            invalid: renderError(invalid),
          });
        }),
      ).pipe(Effect.orDie),
  }),
});
