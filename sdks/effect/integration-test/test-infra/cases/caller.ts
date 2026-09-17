/**
 * Caller case — exercises the cross-agent typed-RPC client wired into
 * every `defineAgent` call.
 *
 *   - bump / peek drive the remote Counter via `Counter.client.get`
 *   - greetWithOverride proves the `{ overrides }` channel on the typed
 *     client actually replaces the `golem.yaml` default
 *   - abortInFlight forks `counter.slowValue(60s)`, sleeps 100ms, then
 *     interrupts the fiber. The SDK's
 *     `Effect.acquireUseRelease(asyncInvokeAndAwait, …, fut.cancel)`
 *     chain must propagate the interrupt to the host as
 *     `future-invoke-result.cancel()`.
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  // Caller's constructor param IS its counterName (the instance ID
  // doubles as the bound parameter), so `Caller("foo")` means
  // "a Caller pointing at Counter('foo')".
  const counterName = `caller-target-${session.stamp}`
  const counterRef = `Counter("${counterName}")`
  const callerRef = `Caller("${counterName}")`

  // Prime the target counter so we can assert state changes.
  yield* liftCliError(cli.invoke(counterRef, "value"))
  yield* liftCliError(cli.invoke(counterRef, "add", ["7"]))

  // Caller.peek reads the counter's current value via RPC.
  const peek = yield* liftCliError(cli.invoke(callerRef, "peek"))
  yield* expectMatch(
    peek.stdout,
    /\b7\b/,
    "Caller.peek reads remote Counter.value via typed RPC client",
  )

  // Caller.bump increments via RPC and returns the new value.
  const bump = yield* liftCliError(cli.invoke(callerRef, "bump"))
  yield* expectMatch(
    bump.stdout,
    /\b8\b/,
    "Caller.bump increments remote Counter and returns new value",
  )

  // Verify the increment was actually persisted on the remote counter.
  const remote = yield* liftCliError(cli.invoke(counterRef, "value"))
  yield* expectMatch(remote.stdout, /\b8\b/, "remote Counter.value reflects RPC increment")

  // greetWithOverride: must run via a Caller pointing at a FRESH
  // Counter (not the pre-primed one above), because RPC config
  // overrides are captured at remote-agent construction time. If
  // the target counter already exists with default config, the
  // override is silently ignored.
  const greetCounterName = `caller-greet-${session.stamp}`
  const greetCallerRef = `Caller("${greetCounterName}")`
  const overrideValue = `hi-from-rpc-${session.stamp}`
  const overridden = yield* liftCliError(
    cli.invoke(greetCallerRef, "greetWithOverride", [`"${overrideValue}"`]),
  )
  yield* expectMatch(
    overridden.stdout,
    new RegExp(overrideValue),
    "Caller.greetWithOverride RPC config override wins over golem.yaml default",
  )

  // abortInFlight: forks a slow remote invocation and interrupts it.
  // Returns true when the fiber's exit cause carried an interrupt
  // (i.e. host fut.cancel() propagated).
  const abort = yield* liftCliError(cli.invoke(callerRef, "abortInFlight", ["10"]))
  yield* expectMatch(
    abort.stdout,
    /true/,
    "Caller.abortInFlight Fiber.interrupt → future-invoke-result.cancel",
  )

  // Oplog should contain a future-invoke-result.cancel entry.
  const oplog = yield* liftCliError(cli.oplog(callerRef))
  yield* expectMatch(
    oplog.stdout,
    /cancel|interrupted/i,
    "Caller oplog records the cancelled future-invoke-result",
  )
})

export const case_ = defineCase(
  "caller",
  "Caller: typed-RPC client + config override + fiber-interrupt → host cancel",
  run,
)
