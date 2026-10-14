/**
 * KvAgent case — exercises the `KeyValue.openBucket` surface against
 * the live `wasi:keyvalue/{eventual,eventual-batch}` host.
 *
 *   - bytes round-trip (putBytes / getBytes / exists / deleteKey)
 *   - schema-typed round-trip (putUser / getUser via forSchema(User))
 *   - batch ops (putBatch / getBatch)
 *   - keys listing
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const name = `kv-${session.stamp}`
  const r = `KvAgent("${name}")`

  // 1. Bytes round-trip.
  yield* liftCliError(cli.invoke(r, "putBytes", [`"alpha"`, `"value-alpha"`]))
  const got = yield* liftCliError(cli.invoke(r, "getBytes", [`"alpha"`]))
  yield* expectMatch(got.stdout, /value-alpha/, "getBytes returns the previously-set value")

  const exists = yield* liftCliError(cli.invoke(r, "exists", [`"alpha"`]))
  yield* expectMatch(exists.stdout, /true/, "exists(alpha) = true")

  const missing = yield* liftCliError(cli.invoke(r, "exists", [`"does-not-exist"`]))
  yield* expectMatch(missing.stdout, /false/, "exists(does-not-exist) = false")

  yield* liftCliError(cli.invoke(r, "deleteKey", [`"alpha"`]))
  const afterDelete = yield* liftCliError(cli.invoke(r, "exists", [`"alpha"`]))
  yield* expectMatch(afterDelete.stdout, /false/, "exists(alpha) = false after deleteKey")

  // 2. Schema-typed round-trip.
  yield* liftCliError(cli.invoke(r, "putUser", [`"u-1"`, `"Alice"`]))
  const user = yield* liftCliError(cli.invoke(r, "getUser", [`"u-1"`]))
  yield* expectMatch(user.stdout, /Alice/, "getUser returns the schema-decoded user")
  yield* expectMatch(user.stdout, /u-1/, "getUser returns the user id")

  // 3. Batch ops.
  yield* liftCliError(
    cli.invoke(r, "putBatch", [`[["batch-1","one"],["batch-2","two"],["batch-3","three"]]`]),
  )
  const batch = yield* liftCliError(
    cli.invoke(r, "getBatch", [`["batch-1","batch-2","batch-3","missing"]`]),
  )
  yield* expectMatch(batch.stdout, /one/, "getBatch positional[0] = one")
  yield* expectMatch(batch.stdout, /two/, "getBatch positional[1] = two")
  yield* expectMatch(batch.stdout, /three/, "getBatch positional[2] = three")
  // Schema.NullOr → WIT option<string>, which the CLI prints as
  // `undefined` (TypeScript-style) for the None case.
  yield* expectMatch(batch.stdout, /undefined|null/, "getBatch missing key → undefined / none")

  // 4. Keys listing — at minimum batch-* + u-1 should be present.
  const keys = yield* liftCliError(cli.invoke(r, "keys"))
  yield* expectMatch(keys.stdout, /batch-1/, "keys contains batch-1")
  yield* expectMatch(keys.stdout, /batch-2/, "keys contains batch-2")
  yield* expectMatch(keys.stdout, /batch-3/, "keys contains batch-3")
  yield* expectMatch(keys.stdout, /u-1/, "keys contains u-1")
})

export const case_ = defineCase(
  "kv",
  "KvAgent: wasi:keyvalue eventual + eventual-batch (bytes / schema / batch / keys)",
  run,
)
