import { Effect, Schema } from "effect";
import { defineAgent, method, WitTypes } from "@golemcloud/effect-golem";

export const EffectBenchmarkAgent = defineAgent({
  name: "EffectBenchmarkAgent",
  id: { name: Schema.String },
  methods: {
    largeInput: method({
      input: { input: Schema.Array(WitTypes.Uint8) },
      success: WitTypes.Uint32,
    }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    largeInput: ({ input }) => Effect.sync(() => input.length),
  }),
});
