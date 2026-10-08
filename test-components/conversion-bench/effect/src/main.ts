import { defineAgent, method, WitTypes } from '@golemcloud/effect-golem';
import { Effect, Schema } from 'effect';

defineAgent({
  name: 'ConversionBenchEffect',
  id: { name: Schema.String },
  methods: {
    checksum: method({ input: { input: Schema.Array(WitTypes.Uint8) }, success: WitTypes.Uint32 }),
    produce: method({ input: { length: WitTypes.Uint32 }, success: Schema.Array(WitTypes.Uint8) }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    checksum: ({ input }) => Effect.sync(() => input.reduce((sum, byte) => (sum + byte) >>> 0, 0)),
    produce: ({ length }) => Effect.sync(() => Array.from({ length }, (_, i) => i % 251)),
  }),
});
