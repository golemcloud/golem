/**
 * Lookup case — exercises the typed-error round-trip across an RPC
 * boundary inside the live Golem WASM runtime.
 *
 * `LookupCaller` (the driver agent) calls `Lookup` (the target agent)
 * over `Lookup.client.get(...)` and reports — as a plain `Schema.String`
 * — what its typed-E channel actually delivered. The harness then
 * regex-matches the printed string. The typed-E surface MUST be
 * `typed:NotFoundError(<resource>)` (NOT `transport:...` — the legacy
 * surface where typed user errors got silently downgraded into
 * `RpcCallError`).
 *
 * This case is the integration-level half of the
 * "RemoteMethod over-promises typed remote failures" fix and the only
 * test in the suite that proves the wasm-rquickjs marshalling of the
 * component-model `result<S, E>` envelope on the wire — unit tests
 * cannot exercise the real host bindings.
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const callerRef = `LookupCaller("lookup-${session.stamp}")`

  // ---- Schema.Number success / typed-error path ----

  // Success arm: `Lookup.fetch({ id: "alice" })` returns 7. The driver
  // unwraps the wire `Result.succeed(7)` to bare 7 and prints "ok:7".
  const ok = yield* liftCliError(cli.invoke(callerRef, "fetchAndReport", ['"alice"']))
  yield* expectMatch(
    ok.stdout,
    /\bok:7\b/,
    "Lookup.fetch success → wire Result.succeed(7) → unwrapped to bare 7",
  )

  // Failure arm: `Lookup.fetch({ id: "missing" })` typed-fails with
  // NotFoundError. The driver MUST observe the typed E in
  // `Effect.catchAll`, NOT a `RemoteCallError` wrapper.
  const typed = yield* liftCliError(cli.invoke(callerRef, "fetchAndReport", ['"missing"']))
  yield* expectMatch(
    typed.stdout,
    /typed:NotFoundError\(missing\)/,
    "Lookup.fetch typed failure → wire Result.fail(NotFoundError) → typed E channel",
  )

  // ---- Schema.Void success / typed-error path ----
  // (same pattern, but exercises the empty-record stand-in the SDK
  // substitutes for `Schema.Void` on the success arm of `result<_, E>`)

  const okVoid = yield* liftCliError(cli.invoke(callerRef, "cmdAndReport", ["false"]))
  yield* expectMatch(
    okVoid.stdout,
    /\bok:void\b/,
    "Lookup.cmd void-success → wire Result.succeed({}) → unwrapped to undefined",
  )

  const typedVoid = yield* liftCliError(cli.invoke(callerRef, "cmdAndReport", ["true"]))
  yield* expectMatch(
    typedVoid.stdout,
    /typed:NotFoundError\(always\)/,
    "Lookup.cmd void-success typed failure → typed E channel",
  )
})

export const case_ = defineCase(
  "lookup",
  "Lookup: typed-error round-trip via component-model result<S, E> over RPC",
  run,
)
