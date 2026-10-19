/**
 * InventorySaga — exercises `Saga.infallibleTransaction` end-to-end
 * inside a real Golem runtime.
 *
 * `runOnceOk` exercises success; `runWithRetries` compensates two failed
 * attempts before succeeding. All attempts remain in the oplog.
 */
import { Effect, Ref, Schema } from "effect"
import { defineAgent, method, Saga, Snapshot } from "@golemcloud/effect-golem"

const RunResult = Schema.Struct({
  reserved: Schema.Number,
  committed: Schema.Number,
  totalAttempts: Schema.Number,
})

const InventorySagaSpec = defineAgent({
  name: "InventorySaga",
  description: "Inventory saga with compensation and guest-side retries",
  mode: "durable",
  id: { name: Schema.String },
  snapshotting: Snapshot.define({
    schema: Schema.Struct({ totalAttempts: Schema.Number }),
    policy: Snapshot.policy.everyN(10),
  }),
  methods: {
    runOnceOk: method({
      input: {},
      success: RunResult,
      description: "Happy-path infallible saga; returns the body's value.",
    }),
    runWithRetries: method({
      input: {},
      success: RunResult,
      description: "Compensates two failed attempts and succeeds on the third.",
    }),
    totalAttempts: method({
      input: {},
      success: Schema.Number,
      description: "Total saga attempts.",
    }),
  },
})

interface InventoryState {
  readonly name: string
  readonly attempts: Ref.Ref<{ totalAttempts: number }>
}

const inventoryMethods = ({ name, attempts: state }: InventoryState) => {
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

    runWithRetries: () =>
      Effect.gen(function* () {
        let attempts = 0
        return yield* Saga.infallibleTransaction(
          Effect.gen(function* () {
            attempts++
            yield* Ref.update(state, (s) => ({ totalAttempts: s.totalAttempts + 1 }))
            const r = yield* reserveStep({ qty: 3 })
            const c = yield* commitStep({ qty: r.reserved })
            if (attempts < 3) return yield* Effect.fail("settle-failed-by-design")
            return { reserved: r.reserved, committed: c.committed, totalAttempts: attempts }
          }),
        )
      }).pipe(Effect.withSpan("InventorySaga.runWithRetries")),

    totalAttempts: () => Ref.get(state).pipe(Effect.map((s) => s.totalAttempts)),
  }
}

export const InventorySaga = InventorySagaSpec.implement<InventoryState>({
  init: ({ name }) =>
    Ref.make({ totalAttempts: 0 }).pipe(Effect.map((attempts) => ({ name, attempts }))),
  methods: inventoryMethods,
  snapshot: {
    save: (state) => Ref.get(state.attempts),
    restore: (saved, context) =>
      Ref.make(saved).pipe(Effect.map((attempts) => ({ name: context.id.name, attempts }))),
  },
})
