/**
 * BlobAgent case — exercises the `Blobstore.getOrCreateContainer`
 * surface against the live `wasi:blobstore` host.
 *
 *   - small object: write / read / has / size
 *   - schema-typed object: putPhoto / getPhoto
 *   - large object (10 000 bytes) to exercise the 4096-byte
 *     `blocking-write-and-flush` chunking
 *   - listObjects
 *   - deleteOne
 *
 * NOTE on `clear`: per AGENTS.md, the filesystem-backed blobstore has
 * an upstream bug where `clear()` deletes the underlying directory
 * and subsequent `listObjects()` traps. We do NOT exercise `clear` in
 * the harness for that reason.
 */
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestFailure, TestSession, defineCase, expectMatch, liftCliError } from "../harness/case.ts"

const run: Effect.Effect<void, TestFailure, GolemCli | TestSession> = Effect.gen(function* () {
  const cli = yield* GolemCli
  const session = yield* TestSession
  const name = `blob-${session.stamp}`
  const r = `BlobAgent("${name}")`

  // Small write → read.
  yield* liftCliError(cli.invoke(r, "write", [`"hello.txt"`, `"hello, blob"`]))
  const read = yield* liftCliError(cli.invoke(r, "read", [`"hello.txt"`]))
  yield* expectMatch(read.stdout, /hello, blob/, "read returns the previously-written value")

  const has = yield* liftCliError(cli.invoke(r, "has", [`"hello.txt"`]))
  yield* expectMatch(has.stdout, /true/, "has(hello.txt) = true")

  const missing = yield* liftCliError(cli.invoke(r, "has", [`"missing.txt"`]))
  yield* expectMatch(missing.stdout, /false/, "has(missing.txt) = false")

  const size = yield* liftCliError(cli.invoke(r, "size", [`"hello.txt"`]))
  yield* expectMatch(size.stdout, /\b11\b/, "size(hello.txt) = 11 bytes (UTF-8)")

  // Schema-typed photo round-trip.
  yield* liftCliError(
    cli.invoke(r, "putPhoto", [`"sunrise.jpg"`, `"sunrise.jpg"`, "1700000000000"]),
  )
  const photo = yield* liftCliError(cli.invoke(r, "getPhoto", [`"sunrise.jpg"`]))
  yield* expectMatch(photo.stdout, /sunrise\.jpg/, "getPhoto returns the filename")
  yield* expectMatch(photo.stdout, /1700000000000/, "getPhoto returns takenAtMillis")

  // Large object to exercise 4096-byte chunking inside the SDK.
  yield* liftCliError(cli.invoke(r, "writeBig", [`"big.bin"`, "10000"]))
  const readSize = yield* liftCliError(cli.invoke(r, "readSize", [`"big.bin"`]))
  yield* expectMatch(
    readSize.stdout,
    /\b10000\b/,
    "readSize(big.bin) = 10000 bytes (whole-object read across chunks)",
  )

  // listObjects must include all three keys.
  const list = yield* liftCliError(cli.invoke(r, "list"))
  yield* expectMatch(list.stdout, /hello\.txt/, "list contains hello.txt")
  yield* expectMatch(list.stdout, /sunrise\.jpg/, "list contains sunrise.jpg")
  yield* expectMatch(list.stdout, /big\.bin/, "list contains big.bin")

  // Delete one and verify it disappears.
  yield* liftCliError(cli.invoke(r, "deleteOne", [`"hello.txt"`]))
  const afterDelete = yield* liftCliError(cli.invoke(r, "has", [`"hello.txt"`]))
  yield* expectMatch(afterDelete.stdout, /false/, "has(hello.txt) = false after deleteOne")
})

export const case_ = defineCase(
  "blob",
  "BlobAgent: wasi:blobstore container + object I/O (small / schema / 10 000-byte / list / delete)",
  run,
)
