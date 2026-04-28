/**
 * Counter — a simple durable, named integer counter built declaratively
 * with effect-golem.
 *
 * Also exercises Effect-Context-based config: the agent declares a
 * `CounterConfig` Schema with one regular field (`greeting`) and one
 * `Schema.Redacted` secret (`apiKey`), reads the greeting at construction
 * time AND inside a method handler, and exposes the secret's tail via
 * `keyTail` so the integration harness can verify the wiring end to end.
 *
 * Snapshotting: the counter state (the `count` integer + its `owner`)
 * is auto-snapshotted via `Snapshot.define`, with the host driving
 * save/load every 10 invocations.
 */
import { Effect, Redacted, Ref, Schema } from "effect"
import { defineAgent, defineConfig, Http, method, Principal, Snapshot } from "effect-golem"

export class CounterConfig extends defineConfig("Counter.Config", {
  greeting: Schema.String,
  apiKey: Schema.Redacted(Schema.String),
}) {}

export const Counter = defineAgent({
  name: "Counter",
  description: "A named integer counter (durable, snapshotted)",
  mode: "durable",
  config: CounterConfig,
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
    /**
     * Sleeps for `seconds` then returns the current value. Driver for
     * the abort-in-flight integration test: the caller forks a remote
     * `slowValue` invocation and interrupts it before the sleep
     * completes; the SDK must propagate the fiber-interrupt to
     * `future-invoke-result.cancel()` on the host.
     */
    slowValue: method({
      params: { seconds: Schema.Number },
      success: Schema.Number,
    }),
  },
  impl: ({ name }, snap) =>
    Effect.gen(function* () {
      yield* Effect.logInfo("Counter constructed").pipe(Effect.annotateLogs({ counter: name }))
      const state = yield* snap.init({ count: 0 })
      const ownerPrincipal = yield* Principal
      const ownerTag =
        ownerPrincipal.tag === "oidc" ? `oidc:${ownerPrincipal.val.sub}` : ownerPrincipal.tag
      return {
        value: () =>
          Ref.get(state).pipe(
            Effect.map((s) => s.count),
            Effect.withSpan("Counter.value"),
          ),
        increment: () =>
          Ref.updateAndGet(state, (s) => ({ count: s.count + 1 })).pipe(
            Effect.tap((s) =>
              Effect.logInfo("incremented").pipe(Effect.annotateLogs({ to: s.count })),
            ),
            Effect.map((s) => s.count),
            Effect.withSpan("Counter.increment"),
          ),
        add: ({ by }) =>
          Ref.updateAndGet(state, (s) => ({ count: s.count + by })).pipe(
            Effect.tap(() => Effect.logDebug("added").pipe(Effect.annotateLogs({ by }))),
            Effect.map((s) => s.count),
            Effect.withSpan("Counter.add", { attributes: { by } }),
          ),
        reset: () => Ref.set(state, { count: 0 }),
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
        slowValue: ({ seconds }) =>
          Effect.gen(function* () {
            yield* Effect.logInfo("slowValue: sleeping").pipe(Effect.annotateLogs({ seconds }))
            yield* Effect.sleep(`${seconds} seconds`)
            const s = yield* Ref.get(state)
            return s.count
          }).pipe(Effect.withSpan("Counter.slowValue", { attributes: { seconds } })),
      }
    }),
})
