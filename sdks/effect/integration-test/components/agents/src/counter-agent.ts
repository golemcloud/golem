/**
 * Counter — a simple durable, named integer counter built declaratively
 * with effect-golem.
 *
 * Also exercises Effect-Context-based config: the agent declares a
 * `CounterConfig` Schema with one regular field (`greeting`) and one
 * `Schema.Redacted` secret (`apiKey`), reads the greeting at construction
 * time AND inside a method handler, and exposes the secret's tail via
 * `keyTail` so the integration harness can verify the wiring end to end.
 */
import { Effect, Redacted, Ref, Schema } from "effect"
import { defineAgent, defineConfig, Http, method, Principal } from "effect-golem"

export class CounterConfig extends defineConfig("Counter.Config", {
  greeting: Schema.String,
  apiKey: Schema.Redacted(Schema.String),
}) {}

export const Counter = defineAgent({
  name: "Counter",
  description: "A named integer counter (durable)",
  mode: "durable",
  config: CounterConfig,
  constructorParams: { name: Schema.String },
  http: Http.mount("/counters/{name}", { cors: ["*"] }),
  methods: {
    value: method({
      params: {},
      success: Schema.Number,
      description: "Returns the current value of the counter without modifying it.",
      promptHint: "Read the counter; never modifies state.",
      http: [Http.get("/value")],
    }),
    increment: method({
      params: {},
      success: Schema.Number,
      description: "Increments the counter by 1 and returns the new value.",
      promptHint: "Bump the counter by one.",
      http: [Http.post("/increment")],
    }),
    add: method({
      params: { by: Schema.Number },
      success: Schema.Number,
      description: "Adds `by` to the counter and returns the new value.",
      promptHint: "Add an arbitrary integer to the counter.",
      http: [
        Http.post("/add"), // by ← JSON body
        Http.get("/add?by={by}"), // by ← query parameter
      ],
    }),
    reset: method({
      params: {},
      success: Schema.Void,
      description: "Resets the counter back to zero.",
      http: [Http.post("/reset")],
    }),
    /** Returns the principal that originally created this Counter. */
    owner: method({
      params: {},
      success: Schema.String,
      http: [Http.get("/owner")],
    }),
    /** Returns the principal that issued THIS call. */
    caller: method({
      params: {},
      success: Schema.String,
      http: [Http.get("/caller")],
    }),
    /** Greeting fetched fresh from config every invocation. */
    currentGreeting: method({
      params: {},
      success: Schema.String,
      http: [Http.get("/greeting")],
    }),
    /** Last 4 chars of the secret, proving the secret pipeline works. */
    keyTail: method({
      params: {},
      success: Schema.String,
      http: [Http.get("/key-tail")],
    }),
  },
  impl: ({ name: _name }) =>
    Effect.gen(function* () {
      const ref = yield* Ref.make(0)
      const ownerPrincipal = yield* Principal
      const ownerTag =
        ownerPrincipal.tag === "oidc" ? `oidc:${ownerPrincipal.val.sub}` : ownerPrincipal.tag
      return {
        value: () => Ref.get(ref),
        increment: () => Ref.updateAndGet(ref, (n) => n + 1),
        add: ({ by }) => Ref.updateAndGet(ref, (n) => n + by),
        reset: () => Ref.set(ref, 0),
        owner: () => Effect.succeed(ownerTag),
        caller: () =>
          Effect.gen(function* () {
            const p = yield* Principal
            return p.tag === "oidc" ? `oidc:${p.val.sub}` : p.tag
          }),
        currentGreeting: () =>
          Effect.gen(function* () {
            const cfg = yield* CounterConfig
            return yield* cfg.greeting
          }),
        keyTail: () =>
          Effect.gen(function* () {
            const cfg = yield* CounterConfig
            const r = yield* cfg.apiKey.get
            return Redacted.value(r).slice(-4)
          }),
      }
    }),
})
