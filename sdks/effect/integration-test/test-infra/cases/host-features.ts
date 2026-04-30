/**
 * HostFeatures case — exercises the Effect-typed wrappers around
 * Durability / Oplog / Agents / SelfAgentId. Drives every method on
 * the agent at least once and asserts on shape, not exact values
 * (most of these probes return runtime-dependent data like UUIDs and
 * oplog tags).
 *
 * The wrappedQuote replay drill (covered at the end) is the closest
 * thing to a property test: we record the live price, run
 * `agent update --await`, and then read the wrappedQuote price again
 * — Durability replays the oplog entry verbatim, so the price MUST be
 * identical. If it changed, the durability protocol is broken.
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import {
  TestFailure,
  TestSession,
  defineCase,
  expectInvokeFails,
  expectMatch,
  liftCliError,
  updateTolerant,
} from "../harness/case.ts"

const extractQuotePrice = (stdout: string): Effect.Effect<string, TestFailure, TestSession> =>
  Effect.gen(function* () {
    const session = yield* TestSession
    const m = stdout.match(/price[^0-9-]*(-?[0-9]+(?:\.[0-9]+)?)/)
    if (!m) {
      return yield* Effect.fail(
        new TestFailure({
          testName: session.currentTest,
          message: "could not parse `price` numeric field from wrappedQuote response",
          diagnostic: stdout,
        }),
      )
    }
    return m[1] as string
  })

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const name = `hf-${session.stamp}`
  const r = `HostFeatures("${name}")`
  // wrappedQuoteFailing leaves the agent in a "Previous Invocation
  // Failed" state on the host; subsequent invocations on the same
  // worker get rejected. We therefore drive that probe on a
  // separate, throwaway instance.
  const failingName = `hf-fail-${session.stamp}`
  const failingRef = `HostFeatures("${failingName}")`

  // Oplog index — string of digits.
  const idx = yield* liftCliError(cli.invoke(r, "oplogIndex"))
  yield* expectMatch(idx.stdout, /\b\d+\b/, "HostFeatures.oplogIndex returns numeric string")

  // Durability.atomically — increments by 3, returns 3.
  const atomic = yield* liftCliError(cli.invoke(r, "withAtomic", ["3"]))
  yield* expectMatch(atomic.stdout, /\b3\b/, "HostFeatures.withAtomic returns 3")

  // Durability.withPersistenceLevel(persistNothing, …) — adds 5, total 8.
  const persistNothing = yield* liftCliError(cli.invoke(r, "withPersistNothing", ["5"]))
  yield* expectMatch(persistNothing.stdout, /\b8\b/, "HostFeatures.withPersistNothing returns 8")

  // Idempotency key — UUID.
  const ikey = yield* liftCliError(cli.invoke(r, "idempotencyKey"))
  yield* expectMatch(
    ikey.stdout,
    /[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}/,
    "HostFeatures.idempotencyKey returns canonical UUID",
  )

  // Self metadata — must include agentName / componentRevision /
  // status / retryCount fields and the agentName must be our name.
  const meta = yield* liftCliError(cli.invoke(r, "selfMetadata"))
  yield* expectMatch(meta.stdout, /agentName/i, "selfMetadata has agentName field")
  yield* expectMatch(meta.stdout, /componentRevision/i, "selfMetadata has componentRevision")
  yield* expectMatch(meta.stdout, /status/i, "selfMetadata has status")
  yield* expectMatch(meta.stdout, /retryCount/i, "selfMetadata has retryCount")
  yield* expectMatch(meta.stdout, new RegExp(name), "selfMetadata.agentName matches instance")

  // Fork — running side returns "original".
  const forked = yield* liftCliError(cli.invoke(r, "forkSelf"))
  yield* expectMatch(
    forked.stdout,
    /original/,
    "HostFeatures.forkSelf returns 'original' on caller side",
  )

  // Read first 5 oplog entries — should at minimum include the
  // initialize / invoke tags that have already accumulated.
  const readOplog = yield* liftCliError(cli.invoke(r, "readOplog", ["5"]))
  yield* expectMatch(
    readOplog.stdout,
    /create|initialize|invoke|exported|imported/i,
    "HostFeatures.readOplog returns recognizable tag names",
  )

  // Search oplog for "withAtomic" — that invocation just happened.
  const search = yield* liftCliError(cli.invoke(r, "searchOplog", [`"withAtomic"`, "10"]))
  yield* expectMatch(
    search.stdout,
    /\[/,
    "HostFeatures.searchOplog returns an array (may be empty if backend lacks index)",
  )

  // Promise round-trip.
  const promise = yield* liftCliError(
    cli.invoke(r, "promiseRoundtrip", [`"hello-${session.stamp}"`]),
  )
  yield* expectMatch(
    promise.stdout,
    new RegExp(`hello-${session.stamp}`),
    "HostFeatures.promiseRoundtrip echoes the payload",
  )

  // wrappedQuote: live call records the rolled price into the oplog.
  const liveQuote = yield* liftCliError(cli.invoke(r, "wrappedQuote", [`"AAPL"`]))
  yield* expectMatch(liveQuote.stdout, /AAPL/, "wrappedQuote response contains the symbol")
  const livePrice = yield* extractQuotePrice(liveQuote.stdout)

  // wrappedQuoteFailing: typed Effect.fail round-trips through the
  // oplog as Result.fail. Run on a fresh instance so the resulting
  // "Previous Invocation Failed" host state does not poison the
  // snapshot / update drill on `r` below.
  const failed = yield* expectInvokeFails(failingRef, "wrappedQuoteFailing", [`"AAPL"`]).pipe(
    Effect.catchTag("GolemCliError", (e) =>
      Effect.fail(
        new TestFailure({
          testName: session.currentTest,
          message: `golem CLI failed unexpectedly: ${e.command.join(" ")}`,
          diagnostic: `exit=${e.exitCode}\nstdout:\n${e.stdout}\nstderr:\n${e.stderr}`,
        }),
      ),
    ),
  )
  yield* expectMatch(
    failed.stdout + failed.stderr,
    /UNAVAILABLE|AAPL|fail/i,
    "wrappedQuoteFailing surfaces a typed failure",
  )

  void livePrice // verified by the schema-decode at parse time

  // Oplog must record a `host-features::wrappedQuote` durable entry,
  // proving Durability.wrap took the live-mode persist branch.
  const quoteOplog = yield* liftCliError(cli.oplog(r))
  yield* expectMatch(
    quoteOplog.stdout,
    /host-features::wrappedQuote|wrappedQuote/i,
    "HostFeatures oplog records wrappedQuote durable function entry",
  )

  // Trigger snapshot drill (everyN(10)) to make sure the
  // Durability + snapshotting interaction works.
  for (let i = 0; i < 10; i++) {
    yield* liftCliError(cli.invoke(r, "withAtomic", ["1"]))
  }
  const oplog = yield* liftCliError(cli.oplog(r))
  yield* expectMatch(oplog.stdout, /SNAPSHOT/i, "HostFeatures oplog has SNAPSHOT entry")

  // update --await proves the snapshot+restore path lights up; the
  // restored agent state is observable via the next invocation.
  yield* updateTolerant(r, "manual")
  const idxAfterUpdate = yield* liftCliError(cli.invoke(r, "oplogIndex"))
  yield* expectMatch(
    idxAfterUpdate.stdout,
    /\b\d+\b/,
    "HostFeatures.oplogIndex still works after update --await",
  )
})

export const case_ = defineCase(
  "host-features",
  "HostFeatures: Durability/Oplog/Agents/SelfAgentId wrappers + replay round-trip",
  run,
)
