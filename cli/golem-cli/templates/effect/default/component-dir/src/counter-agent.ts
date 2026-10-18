import { Effect, Ref, Schema } from "effect";
import { defineAgent, Http, method } from "@golemcloud/effect-golem";

export const CounterAgent = defineAgent({
  name: "CounterAgent",
  description: "A durable named counter",
  mode: "durable",
  id: { name: Schema.String },
  http: Http.mount("/counters/{name}", { cors: ["*"] }),
  methods: {
    add: method({
      input: { by: Schema.Number },
      success: Schema.Number,
      http: [Http.post("/add")],
    }),
    reset: method({
      input: {},
      success: Schema.Void,
      http: [Http.post("/reset")],
    }),
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
}).implement<Ref.Ref<{ count: number }>>({
  init: ({ name }) =>
    Effect.gen(function* () {
      const state = yield* Ref.make({ count: 0 });
      yield* Effect.logInfo("Counter constructed").pipe(
        Effect.annotateLogs({ name }),
      );
      return state;
    }),
  methods: (state) => ({
    add: ({ by }) =>
      Ref.updateAndGet(state, ({ count }) => ({ count: count + by })).pipe(
        Effect.map(({ count }) => count),
      ),
    reset: () => Ref.set(state, { count: 0 }),
    value: () => Ref.get(state).pipe(Effect.map(({ count }) => count)),
    increment: () =>
      Ref.updateAndGet(state, ({ count }) => ({ count: count + 1 })).pipe(
        Effect.map(({ count }) => count),
      ),
  }),
});
