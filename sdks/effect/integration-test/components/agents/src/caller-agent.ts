/**
 * Caller — a durable agent that drives the Counter agent over the
 * Golem typed-RPC client. Because both agents live in the same
 * component, the Counter definition (and therefore its statically-typed
 * client) is reused directly — no separate "stub" needed.
 */
import { Cause, Effect, Exit, Fiber, Schema } from "effect"
import { Counter } from "./counter-agent.js"
import { defineAgent, InvocationId, method } from "@golemcloud/effect-golem"

export const Caller = defineAgent({
  name: "Caller",
  description: "Coordinator that drives a remote Counter via wasm-rpc",
  mode: "durable",
  id: { counterName: Schema.String },
  methods: {
    /** Increment the remote counter once and return its new value. */
    bump: method({ input: {}, success: Schema.Number }),
    /** Read the current value of the remote counter. */
    peek: method({ input: {}, success: Schema.Number }),
    /**
     * Create a new Counter with an RPC config override for the
     * `greeting` field, then read it back via `currentGreeting`. Proves
     * the `config` option of `create` actually drives the `agent-config`
     * of `golem:agent/host.wasm-rpc.create` and that the override wins
     * over the `golem.yaml` default.
     */
    greetWithOverride: method({ input: { override: Schema.String }, success: Schema.String }),
    /**
     * Forks a long-running remote `slowValue` invocation, sleeps
     * briefly, then `Fiber.interrupt`s the forked fiber. Returns
     * `true` when the fiber's exit cause carries an interrupt. The
     * interrupt aborts the signal of the awaited `invoke-and-await`
     * import, so the runtime drops the import and the host withdraws
     * the invocation when it has not started yet.
     */
    abortInFlight: method({ input: { seconds: Schema.Number }, success: Schema.Boolean }),
    /**
     * Exercises the invocation handles of `trigger` and `schedule` on the
     * remote Counter: status, await, cancel, rebuilding a handle from its ID,
     * withdrawing a scheduled invocation, and the typed failure of cancelling
     * a running one. Returns the observations joined with `:`.
     */
    invocationHandles: method({ input: {}, success: Schema.String }),
  },
}).implement({
  init: ({ counterName }) => Effect.succeed(counterName),
  methods: (counterName) => ({
    bump: () =>
      Effect.gen(function* () {
        const counter = yield* Counter.client.upsert({ name: counterName })
        return yield* counter.increment({})
      }).pipe(
        Effect.catch(() => Effect.succeed(-1)),
        Effect.scoped,
      ),
    peek: () =>
      Effect.gen(function* () {
        const counter = yield* Counter.client.upsert({ name: counterName })
        return yield* counter.value({})
      }).pipe(
        Effect.catch(() => Effect.succeed(-1)),
        Effect.scoped,
      ),
    greetWithOverride: ({ override }) =>
      Effect.gen(function* () {
        const counter = yield* Counter.client.create(
          { name: counterName },
          { config: { greeting: override } },
        )
        return yield* counter.currentGreeting({})
      }).pipe(
        Effect.catch((e) => Effect.succeed(`error: ${String(e)}`)),
        Effect.scoped,
      ),
    abortInFlight: ({ seconds }) =>
      Effect.gen(function* () {
        const counter = yield* Counter.client.upsert({ name: counterName })
        // Fork the remote invoke; let it wait for the awaited
        // import; then interrupt.
        const fiber = yield* Effect.forkChild(counter.slowValue({ seconds }))
        yield* Effect.sleep("100 millis")
        yield* Fiber.interrupt(fiber)
        const exit = yield* Fiber.await(fiber)
        // Either we're truly interrupted, or the slow remote already
        // started executing and the result was discarded — both are
        // valid "host cancel was issued" outcomes. Test passes if
        // we did NOT successfully receive the slow value.
        return Exit.isFailure(exit) && Cause.hasInterrupts(exit.cause)
      }).pipe(
        Effect.catch(() => Effect.succeed(false)),
        Effect.scoped,
      ),
    invocationHandles: () =>
      Effect.gen(function* () {
        const counter = yield* Counter.client.upsert({ name: counterName })

        const triggered = yield* counter.increment.trigger({})
        const incremented = yield* triggered.awaitResult
        const triggeredStatus = yield* triggered.status
        const cancelCompleted = yield* triggered.cancel

        const read = yield* counter.value.trigger({})
        const rebuilt = yield* InvocationId.get(read.id, Counter.methods.value)
        const rebuiltValue = yield* rebuilt.awaitResult

        const scheduled = yield* counter.increment.schedule(Date.now() + 3_600_000, {})
        const scheduledStatus = yield* scheduled.status
        const cancelScheduled = yield* scheduled.cancel
        const cancelledStatus = yield* scheduled.status
        const awaitCancelled = yield* scheduled.awaitResult.pipe(
          Effect.match({ onFailure: errorTag, onSuccess: () => "unexpected:completed" }),
        )

        const slow = yield* counter.slowValue.trigger({ seconds: 5 })
        let runningStatus = yield* slow.status
        for (let i = 0; i < 100 && runningStatus === "pending"; i++) {
          yield* Effect.sleep("100 millis")
          runningStatus = yield* slow.status
        }
        const cancelRunning = yield* slow.cancel.pipe(
          Effect.match({ onFailure: errorTag, onSuccess: (b) => `unexpected:${String(b)}` }),
        )
        const slowValue = yield* slow.awaitResult

        return [
          triggeredStatus,
          cancelCompleted,
          incremented,
          rebuiltValue,
          scheduledStatus,
          cancelScheduled,
          cancelledStatus,
          awaitCancelled,
          runningStatus,
          cancelRunning,
          slowValue,
        ].join(":")
      }).pipe(
        Effect.catch((e) => Effect.succeed(`error: ${errorTag(e)}`)),
        Effect.scoped,
      ),
  }),
})

const errorTag = (error: unknown): string =>
  typeof error === "object" && error !== null && "_tag" in error
    ? String(error._tag)
    : String(error)
