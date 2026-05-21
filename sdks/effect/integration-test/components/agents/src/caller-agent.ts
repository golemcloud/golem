/**
 * Caller — a durable agent that drives the Counter agent over the
 * Golem typed-RPC client. Because both agents live in the same
 * component, the Counter definition (and therefore its statically-typed
 * client) is reused directly — no separate "stub" needed.
 */
import { Cause, Effect, Exit, Fiber, Schema } from "effect"
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
    /**
     * Construct a Counter client with an RPC config override for the
     * `greeting` field, then read it back via `currentGreeting`. Proves
     * the `{ overrides }` channel on `AgentClient.get` actually drives
     * `golem:agent/host.WasmRpc(agent-config: list<typed-agent-config-value>)`
     * and that the override wins over the `golem.yaml` default.
     */
    greetWithOverride: method({ params: { override: Schema.String }, success: Schema.String }),
    /**
     * Forks a long-running remote `slowValue` invocation, sleeps
     * briefly, then `Fiber.interrupt`s the forked fiber. Returns
     * `true` when the fiber's exit cause carries an interrupt — which
     * proves the SDK's `Effect.acquireUseRelease(asyncInvokeAndAwait,
     * pollable.abortablePromise, fut.cancel)` chain propagated the
     * fiber-interrupt to the host. Inspect the oplog with
     * `golem -L agent oplog 'Caller("...")'` to see the
     * `future-invoke-result.cancel()` entry.
     */
    abortInFlight: method({ params: { seconds: Schema.Number }, success: Schema.Boolean }),
  },
}).implement(({ counterName }) =>
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
    greetWithOverride: ({ override }) =>
      Effect.gen(function* () {
        const counter = yield* Counter.client.get(
          { name: counterName },
          { overrides: { greeting: override } },
        )
        return yield* counter.currentGreeting({})
      }).pipe(Effect.catch((e) => Effect.succeed(`error: ${String(e)}`))),
    abortInFlight: ({ seconds }) =>
      Effect.gen(function* () {
        const counter = yield* Counter.client.get({ name: counterName })
        // Fork the remote invoke; let it park inside the SDK's
        // pollable.abortablePromise; then interrupt.
        const fiber = yield* Effect.forkChild(counter.slowValue({ seconds }))
        yield* Effect.sleep("100 millis")
        yield* Fiber.interrupt(fiber)
        const exit = yield* Fiber.await(fiber)
        // Either we're truly interrupted, or the slow remote already
        // started executing and the result was discarded — both are
        // valid "host cancel was issued" outcomes. Test passes if
        // we did NOT successfully receive the slow value.
        return Exit.isFailure(exit) && Cause.hasInterrupts(exit.cause)
      }).pipe(Effect.catch(() => Effect.succeed(false))),
  }),
)
