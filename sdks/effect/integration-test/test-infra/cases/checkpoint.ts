import { createServer } from "node:http"
import { Effect } from "effect"
import { GolemCli } from "../harness/golem-cli.ts"
import { TestSession, defineCase, expectEqual, expectMatch, liftCliError } from "../harness/case.ts"

export const case_ = defineCase(
  "checkpoint",
  "Checkpoint rollback restores state and replays without repeating retained HTTP effects",
  Effect.scoped(
    Effect.gen(function* () {
      const cli = yield* GolemCli
      const session = yield* TestSession
      let requests = 0
      const server = yield* Effect.acquireRelease(
        Effect.promise(
          () =>
            new Promise<ReturnType<typeof createServer>>((resolve, reject) => {
              const server = createServer((_request, response) => {
                requests += 1
                response.end(requests === 1 ? "retry" : "ok")
              })
              server.once("error", reject)
              server.listen(0, "127.0.0.1", () => resolve(server))
            }),
        ),
        (server) =>
          Effect.promise(() => new Promise<void>((resolve) => server.close(() => resolve()))),
      )
      const address = server.address()
      if (address === null || typeof address === "string")
        return yield* Effect.die("Missing probe port")
      const agent = `HostFeatures("checkpoint-${session.stamp}")`
      const result = yield* liftCliError(
        cli.invoke(agent, "checkpointRollback", [
          JSON.stringify(`http://127.0.0.1:${address.port}/attempt`),
        ]),
      )
      yield* expectMatch(result.stdout, /\b20\b/, "rollback restores counter before retry")
      yield* expectEqual(requests, 2, "discarded HTTP call executes once again")
      yield* liftCliError(cli.run(["--local", "agent", "simulate-crash", agent]))
      const restored = yield* liftCliError(cli.invoke(agent, "withAtomic", ["0"]))
      yield* expectMatch(restored.stdout, /\b20\b/, "counter survives reconstruction")
      yield* expectEqual(
        requests,
        2,
        "retained HTTP completion is replayed without another request",
      )
      const oplog = yield* liftCliError(cli.oplog(agent))
      yield* expectMatch(oplog.stdout, /JUMP/, "checkpoint uses Jump")
      yield* expectEqual(
        /golem::api::revert_worker|\bREVERT\b/.test(oplog.stdout),
        false,
        "no management revert",
      )
    }),
  ),
)
