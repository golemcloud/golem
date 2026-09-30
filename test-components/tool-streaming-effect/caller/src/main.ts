import { Effect, Schema } from "effect";
import { defineAgent, method, WitTypes } from "@golemcloud/effect-golem";
import { client as matrixCore } from "matrix-core-tool-guest-client";

const MatrixObservation = Schema.Struct({
  provider: Schema.String,
  command: Schema.String,
  normalizedSource: Schema.String,
  weightedSize: WitTypes.Int64,
  labelSummary: Schema.String,
  principal: Schema.String,
  ownerAgentId: Schema.String,
  errorField: Schema.String,
  errorReason: Schema.String,
  errorRetryable: Schema.Boolean,
});

const successRequest = {
  source: "matrix.sample",
  dimensions: { width: 3, height: 5 },
  labels: ["north", "east", "south"],
};

const rejectedRequest = {
  source: "reject.me",
  dimensions: { width: 2, height: 11 },
  labels: ["not", "used"],
};

defineAgent({
  name: "EffectToolStreamingCaller",
  id: { name: Schema.String },
  methods: {
    matrix_core_observation: method({ input: {}, success: MatrixObservation }),
  },
}).implement({
  init: () => Effect.void,
  methods: () => ({
    matrix_core_observation: () =>
      Effect.gen(function* () {
        const artifact = matrixCore.artifact();
        const success = yield* artifact.inspect(successRequest, 7n);
        const failure = yield* artifact
          .inspect(rejectedRequest, 13n)
          .pipe(Effect.flip);
        if (failure.tag !== "tool" || failure.error.tag !== "Rejected") {
          return yield* Effect.die(
            `expected rejected matrix-core error, got ${failure.tag}`,
          );
        }
        return {
          ...success,
          errorField: failure.error.value.field,
          errorReason: failure.error.value.reason,
          errorRetryable: failure.error.value.retryable,
        };
      }).pipe(Effect.scoped, Effect.orDie),
  }),
});
