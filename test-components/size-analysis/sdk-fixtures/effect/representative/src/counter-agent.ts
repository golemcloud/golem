import { Effect, Ref, Schema, Stream } from 'effect';
import { defineAgent, defineConfig, Http, method } from '@golemcloud/effect-golem';
import { fromByteArray } from 'base64-js';
import { ChunkMEffectStreamingClient } from 'chunk-m-effect-streaming-tool-guest-client';

const payload = '__SIZE_PAYLOAD__';

class Settings extends defineConfig('SizeFixture.Config', { label: Schema.String }) {}

const SizeHelper = defineAgent({
  name: 'SizeHelper',
  id: { name: Schema.String },
  methods: {
    value: method({ input: {}, success: Schema.Number }),
    add: method({ input: { by: Schema.Number }, success: Schema.Number }),
  },
}).implement<Ref.Ref<number>>({
  init: () => Ref.make(0),
  methods: (state) => ({
    value: () => Ref.get(state),
    add: ({ by }) => Ref.updateAndGet(state, (value) => value + by),
  }),
});

export const CounterAgent = defineAgent({
  name: 'CounterAgent',
  id: { name: Schema.String },
  config: Settings,
  http: Http.mount('/counters/{name}'),
  methods: {
    value: method({ input: {}, success: Schema.Number, http: [Http.get('/value')] }),
    add: method({ input: { by: Schema.Number }, success: Schema.Number }),
    configured: method({ input: {}, success: Schema.String }),
    payloadLength: method({ input: {}, success: Schema.Number }),
    slice: method({ input: { start: Schema.Number, length: Schema.Number }, success: Schema.String }),
    dependency: method({ input: { start: Schema.Number, length: Schema.Number }, success: Schema.String }),
    rpcAdd: method({ input: { by: Schema.Number }, success: Schema.Number }),
    tool: method({ input: {}, success: Schema.String }),
  },
}).implement<{ name: string; count: Ref.Ref<number> }>({
  init: ({ name }) => Ref.make(0).pipe(Effect.map((count) => ({ name, count }))),
  methods: (state) => ({
    value: () => Ref.get(state.count),
    add: ({ by }) => Ref.updateAndGet(state.count, (value) => value + by),
    configured: () => Effect.gen(function* () { const config = yield* Settings; return yield* config.label; }),
    payloadLength: () => Effect.succeed(payload.length),
    slice: ({ start, length }) => Effect.succeed(payload.slice(start, start + length)),
    dependency: ({ start, length }) => Effect.succeed(fromByteArray(new TextEncoder().encode(payload.slice(start, start + length)))),
    rpcAdd: ({ by }) => Effect.gen(function* () {
      const helper = yield* SizeHelper.client.upsert({ name: state.name });
      return yield* helper.add({ by });
    }).pipe(Effect.scoped, Effect.orDie),
    tool: () => Effect.gen(function* () {
      const invocation = yield* ChunkMEffectStreamingClient.create().chunk_m_effect_streaming(
        'echo', Stream.succeed(new Uint8Array([65, 0, 255, 90])),
      );
      if (!invocation.stdout) return yield* Effect.die('missing tool stdout');
      const [result, chunks] = yield* Effect.all(
        [invocation.result, Stream.runCollect(invocation.stdout)], { concurrency: 'unbounded' },
      );
      const bytes = new Uint8Array(chunks.flatMap((chunk) => Array.from(chunk)));
      return `${result.bytesRead}:${result.outcome}:${fromByteArray(bytes)}`;
    }).pipe(Effect.scoped, Effect.orDie),
  }),
});
