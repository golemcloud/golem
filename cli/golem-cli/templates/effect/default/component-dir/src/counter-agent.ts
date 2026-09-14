import { Effect, Ref, Schema } from "effect";
import { defineAgent, Http, method, Snapshot } from "@golemcloud/effect-golem";

export const Counter = defineAgent({
  name: "Counter",
  description: "A durable named counter",
  mode: "durable",
  id: { name: Schema.String },
  http: Http.mount("/counters/{name}", { cors: ["*"] }),
  snapshotting: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    value: method({
      input: {},
      success: Schema.Number,
      readOnly: true,
      description: "Returns the current value",
      http: [Http.get("/value")],
    }),
    increment: method({
      input: {},
      success: Schema.Number,
      description: "Increments the counter and returns the new value",
      http: [Http.post("/increment")],
    }),
  },
}).implement({
  init: ({ name }) =>
    Effect.gen(function* () {
      const state = yield* Ref.make({ count: 0 });
      yield* Effect.logInfo("Counter constructed").pipe(
        Effect.annotateLogs({ name }),
      );
      return state;
    }),
  methods: (state) => ({
    value: () => Ref.get(state).pipe(Effect.map(({ count }) => count)),
    increment: () =>
      Ref.updateAndGet(state, ({ count }) => ({ count: count + 1 })).pipe(
        Effect.map(({ count }) => count),
      ),
  }),
  snapshot: Snapshot.ref(),
});
