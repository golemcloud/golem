/**
 * InventorySaga — exercises `Saga.infallibleTransaction` end-to-end
 * inside a real Golem runtime.
 *
 * The agent simulates a "reserve → commit → settle" workflow on top
 * of the infallible saga combinator. Because the host's oplog
 * machinery makes `Effect.fail` deterministic across replays, an
 * agent body that fails on the first attempt would also fail on
 * every subsequent replay — yielding an infinite loop. That is the
 * **correct** infallible-saga semantic ("retry until the failure
 * stops"), but to keep the integration drill bounded we expose two
 * methods:
 *
 * - `runOnceOk()` — happy-path workflow. Verifies the wire layout:
 *   each step opens its own atomic region, no `JUMP` entries are
 *   emitted, and the body returns its value.
 * - `runOnceAndJump()` — drives a deliberate first-attempt failure.
 *   Observable via `golem agent oplog`: each step emits a balanced
 *   `BEGIN ATOMIC REGION` / `END ATOMIC REGION` pair, compensations
 *   run in reverse order, and a `JUMP` entry from the current oplog
 *   tip back to the captured checkpoint is recorded — exactly what
 *   `set-oplog-index` produces. The replayed body fails again and
 *   issues another JUMP, so the agent loops forever; **interrupt
 *   the agent manually after inspecting the oplog**:
 *
 *   ```
 *   golem -L agent invoke -n 'InventorySaga("demo")' runOnceAndJump &
 *   sleep 2 && golem -L agent oplog 'InventorySaga("demo")'
 *   golem -L agent interrupt 'InventorySaga("demo")'
 *   golem -L -Y agent delete 'InventorySaga("demo")'
 *   ```
 *
 * The unit test suite (`test/saga.test.ts`) covers the rewind+park
 * protocol comprehensively against host mocks; this integration agent
 * is here to confirm that the wire-level oplog layout is bit-compatible
 * with the official `golem-ts-sdk`.
 */
import { Effect, Ref, Schema } from "effect"
import { defineAgent, method, Saga, Snapshot } from "effect-golem"

const RunResult = Schema.Struct({
  reserved: Schema.Number,
  committed: Schema.Number,
  totalAttempts: Schema.Number,
})

export const InventorySaga = defineAgent({
  name: "InventorySaga",
  description: "Inventory saga that exercises Saga.infallibleTransaction (rewind-on-failure)",
  mode: "durable",
  constructorParams: { name: Schema.String },
  snapshot: Snapshot.define({
    schema: Schema.Struct({ totalAttempts: Schema.Number }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    runOnceOk: method({
      params: {},
      success: RunResult,
      description: "Happy-path infallible saga; returns the body's value.",
    }),
    runOnceAndJump: method({
      params: {},
      success: Schema.Void,
      description:
        "Drives a deliberate first-attempt failure to emit a JUMP oplog entry. Loops forever; interrupt manually.",
    }),
    totalAttempts: method({
      params: {},
      success: Schema.Number,
      description: "Total method invocations (incl. host-driven replays after a JUMP).",
    }),
  },
  impl: ({ name }, snap) =>
    Effect.gen(function* () {
      const state = yield* snap.init({ totalAttempts: 0 })

      const reserveStep = Saga.operation({
        execute: ({ qty }: { qty: number }) =>
          Effect.gen(function* () {
            yield* Effect.logInfo("reserve.exec").pipe(Effect.annotateLogs({ qty, agent: name }))
            return { reserved: qty }
          }),
        compensate: (_in: { qty: number }, _out: { reserved: number }) =>
          Effect.logInfo("reserve.comp").pipe(Effect.annotateLogs({ agent: name })),
      })

      const commitStep = Saga.operation({
        execute: ({ qty }: { qty: number }) =>
          Effect.gen(function* () {
            yield* Effect.logInfo("commit.exec").pipe(Effect.annotateLogs({ qty, agent: name }))
            return { committed: qty }
          }),
        compensate: (_in: { qty: number }, _out: { committed: number }) =>
          Effect.logInfo("commit.comp").pipe(Effect.annotateLogs({ agent: name })),
      })

      const okSettleStep = Saga.withCompensation(
        Effect.gen(function* () {
          yield* Effect.logInfo("settle.exec").pipe(Effect.annotateLogs({ agent: name }))
          return { settled: true }
        }),
        (_out: { settled: boolean }) =>
          Effect.logInfo("settle.comp").pipe(Effect.annotateLogs({ agent: name })),
      )

      // Step 3 always fails. The surrounding infallibleTransaction
      // drains comps and issues setOplogIndex(checkpoint), causing the
      // host to rewind. On replay the same path runs and fails again
      // (the failure is deterministic), so this method loops forever
      // by design — it exists purely to exercise the wire protocol.
      const failingSettleStep = Saga.withCompensation(
        Effect.gen(function* () {
          yield* Effect.logInfo("settle.exec.fail").pipe(Effect.annotateLogs({ agent: name }))
          return yield* Effect.fail("settle-failed-by-design" as const)
        }),
        (_out: { settled: boolean }) =>
          Effect.logInfo("settle.comp").pipe(Effect.annotateLogs({ agent: name })),
      )

      return {
        runOnceOk: () =>
          Effect.gen(function* () {
            yield* Ref.update(state, (s) => ({ totalAttempts: s.totalAttempts + 1 }))
            const result = yield* Saga.infallibleTransaction(
              Effect.gen(function* () {
                const r = yield* reserveStep({ qty: 7 })
                const c = yield* commitStep({ qty: r.reserved })
                yield* okSettleStep
                return { reserved: r.reserved, committed: c.committed }
              }) as Effect.Effect<{ reserved: number; committed: number }, never, never>,
            )
            const totals = yield* Ref.get(state)
            return {
              reserved: result.reserved,
              committed: result.committed,
              totalAttempts: totals.totalAttempts,
            }
          }).pipe(Effect.withSpan("InventorySaga.runOnceOk")),

        runOnceAndJump: () =>
          Effect.gen(function* () {
            yield* Ref.update(state, (s) => ({ totalAttempts: s.totalAttempts + 1 }))
            yield* Saga.infallibleTransaction(
              Effect.gen(function* () {
                const r = yield* reserveStep({ qty: 3 })
                yield* commitStep({ qty: r.reserved })
                yield* failingSettleStep
              }) as Effect.Effect<void, never, never>,
            )
          }).pipe(Effect.withSpan("InventorySaga.runOnceAndJump")),

        totalAttempts: () => Ref.get(state).pipe(Effect.map((s) => s.totalAttempts)),
      }
    }),
})
