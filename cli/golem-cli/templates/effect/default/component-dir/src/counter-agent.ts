import { Effect, Ref, Schema } from "effect";
import {
  defineAgent,
  Http,
  method,
  Snapshot,
} from "@golemcloud/effect-golem";

export const Counter = defineAgent({
  name: "Counter",
  description: "A durable named counter",
  mode: "durable",
  constructorParams: { name: Schema.String },
  http: Http.mount("/counters/{name}", { cors: ["*"] }),
  snapshot: Snapshot.define({
    schema: Schema.Struct({ count: Schema.Number }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    value: method({
      params: {},
      success: Schema.Number,
      description: "Returns the current value",
      http: [Http.get("/value")],
    }),
    increment: method({
      params: {},
      success: Schema.Number,
      description: "Increments the counter and returns the new value",
      http: [Http.post("/increment")],
    }),
  },
}).implement(({ name }, snapshot) =>
  Effect.gen(function* () {
    const state = yield* snapshot.init({ count: 0 });
    yield* Effect.logInfo("Counter constructed").pipe(
      Effect.annotateLogs({ name }),
    );

    return {
      value: () => Ref.get(state).pipe(Effect.map(({ count }) => count)),
      increment: () =>
        Ref.updateAndGet(state, ({ count }) => ({ count: count + 1 })).pipe(
          Effect.map(({ count }) => count),
        ),
    };
  }),
);
