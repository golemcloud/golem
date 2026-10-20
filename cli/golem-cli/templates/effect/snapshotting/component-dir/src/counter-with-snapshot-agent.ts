import { Effect, Ref, Schema } from "effect";
import { defineAgent, Http, method, Snapshot } from "@golemcloud/effect-golem";

export const CounterWithSnapshotAgent = defineAgent({
  name: "CounterWithSnapshotAgent",
  id: { name: Schema.String },
  http: Http.mount("/snapshot-counters/{name}"),
  snapshotting: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number }),
    policy: Snapshot.policy.everyN(5),
  }),
  methods: {
    value: method({
      input: {},
      success: Schema.Number,
      readOnly: true,
      http: [Http.get("/value")],
    }),
    increment: method({
      input: {},
      success: Schema.Number,
      http: [Http.post("/increment")],
    }),
  },
}).implement({
  init: () => Ref.make({ count: 0 }),
  methods: (state) => ({
    value: () => Ref.get(state).pipe(Effect.map(({ count }) => count)),
    increment: () =>
      Ref.updateAndGet(state, ({ count }) => ({ count: count + 1 })).pipe(
        Effect.map(({ count }) => count),
      ),
  }),
  snapshot: Snapshot.ref(),
});
