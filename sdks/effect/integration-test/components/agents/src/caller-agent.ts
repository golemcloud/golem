/**
 * Caller — a durable agent that drives the Counter agent over the
 * Golem typed-RPC client. Because both agents live in the same
 * component, the Counter definition (and therefore its statically-typed
 * client) is reused directly — no separate "stub" needed.
 */
import { Effect, Schema } from "effect"
import { Counter } from "./counter-agent.js"
import { defineAgent, method } from "effect-golem"

export const Caller = defineAgent({
  name: "Caller",
  description: "Coordinator that drives a remote Counter via wasm-rpc",
  mode: "durable",
  constructorParams: { counterName: Schema.String },
  methods: {
    /** Increment the remote counter once and return its new value. */
    bump: method({ params: {}, success: Schema.Number }),
    /** Read the current value of the remote counter. */
    peek: method({ params: {}, success: Schema.Number }),
  },
  impl: ({ counterName }) =>
    Effect.succeed({
      bump: () =>
        Effect.gen(function* () {
          const counter = yield* Counter.client.get({ name: counterName })
          return yield* counter.increment({})
        }).pipe(Effect.catch(() => Effect.succeed(-1))),
      peek: () =>
        Effect.gen(function* () {
          const counter = yield* Counter.client.get({ name: counterName })
          return yield* counter.value({})
        }).pipe(Effect.catch(() => Effect.succeed(-1))),
    }),
})
