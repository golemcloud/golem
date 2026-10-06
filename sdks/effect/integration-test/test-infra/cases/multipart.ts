import { Effect } from "effect"
import {
  defineCase,
  expectContains,
  expectMatch,
  liftCliError,
  TestSession,
} from "../harness/case.ts"
import { GolemCli } from "../harness/golem-cli.ts"

export const case_ = defineCase(
  "multipart",
  "Multipart bytes survive snapshot recovery and suffix replay without initialization",
  Effect.gen(function* () {
    const cli = yield* GolemCli
    const session = yield* TestSession
    const ref = `MultipartIndex("multipart-${session.stamp}")`
    for (let i = 0; i < 10; i++) yield* liftCliError(cli.invoke(ref, "append", [String(i)]))
    const oplog = yield* liftCliError(cli.oplog(ref))
    yield* expectMatch(oplog.stdout, /SNAPSHOT/i, "multipart snapshot was saved")
    // These writes must be reconstructed by suffix replay, not present in the snapshot.
    for (const byte of [255, 13, 10]) yield* liftCliError(cli.invoke(ref, "append", [String(byte)]))
    yield* liftCliError(cli.run(["--local", "--yes", "agent", "simulate-crash", ref]))
    const result = yield* liftCliError(cli.invoke(ref, "inspect"))
    const expected = [
      ...Array.from({ length: 256 }, (_, i) => i),
      ...Array.from({ length: 10 }, (_, i) => i),
      255,
      13,
      10,
    ]
    // The restored flag is only set in restore: a full init+replay fallback fails this assertion.
    yield* expectContains(
      result.stdout,
      `13|true|${expected.join(",")}`,
      "snapshot load plus suffix replay preserves every byte",
    )
  }),
)
