/**
 * Layer-based test fake for {@link KeyValueClient}. Backed by an
 * in-memory `Map<bucketName, Map<key, Uint8Array>>` shared across
 * the fake's per-bucket handles, plus `Ref`-backed slots for
 * one-shot error injection on `openBucket`, `eventual.*`, and
 * `eventual-batch.*` calls.
 *
 * Bypasses the production {@link KeyValueLive} layer entirely — the
 * `wasi:keyvalue/*` WIT specifiers are NOT loaded along this path.
 *
 * Use with `Effect.provide(eff, fake.layer)` once per test (NOT via
 * `it.layer(fake.layer)`, which would share state across the entire
 * `describe` block and cause cross-test interference).
 */

import { Effect, Layer, Option, Ref } from "effect"
import { KeyValueClient, type HostBucket } from "../../src/host/KeyValueClient.js"
import { KeyValueHostError } from "../../src/keyvalue.js"

type EventualOp = "get" | "set" | "delete" | "exists"
type BatchOp = "get-many" | "set-many" | "delete-many" | "keys"

export interface KeyValueFake {
  readonly layer: Layer.Layer<KeyValueClient>
  /**
   * Queue a one-shot `openBucket` failure for the next call matching
   * `name`. Consumed exactly once on first match.
   */
  readonly setOpenError: (name: string, message: string) => Effect.Effect<void>
  /**
   * Queue a one-shot per-key error for the next `eventual.*` call.
   * Consumed exactly once.
   */
  readonly setNextError: (op: EventualOp, message: string) => Effect.Effect<void>
  /**
   * Queue a one-shot per-batch error for the next `eventual-batch.*`
   * call. Consumed exactly once.
   */
  readonly setNextBatchError: (op: BatchOp, message: string) => Effect.Effect<void>
  /** Snapshot of currently-known bucket names. */
  readonly buckets: Effect.Effect<ReadonlyArray<string>>
}

export const make = (): Effect.Effect<KeyValueFake> =>
  Effect.gen(function* () {
    const store = yield* Ref.make<Map<string, Map<string, Uint8Array>>>(new Map())
    const openErr = yield* Ref.make<Option.Option<{ name: string; message: string }>>(Option.none())
    const evErr = yield* Ref.make<Option.Option<{ op: EventualOp; message: string }>>(Option.none())
    const batchErr = yield* Ref.make<Option.Option<{ op: BatchOp; message: string }>>(Option.none())

    const consumeEvErr = (op: EventualOp): Effect.Effect<Option.Option<KeyValueHostError>> =>
      Ref.modify(evErr, (current) => {
        if (Option.isSome(current) && current.value.op === op) {
          return [
            Option.some(new KeyValueHostError(current.value.message, `eventual.${op}`)),
            Option.none<{ op: EventualOp; message: string }>(),
          ]
        }
        return [Option.none<KeyValueHostError>(), current]
      })

    const consumeBatchErr = (op: BatchOp): Effect.Effect<Option.Option<KeyValueHostError>> =>
      Ref.modify(batchErr, (current) => {
        if (Option.isSome(current) && current.value.op === op) {
          return [
            Option.some(new KeyValueHostError(current.value.message, `eventual-batch.${op}`)),
            Option.none<{ op: BatchOp; message: string }>(),
          ]
        }
        return [Option.none<KeyValueHostError>(), current]
      })

    const getOrCreateBucket = (name: string): Effect.Effect<Map<string, Uint8Array>> =>
      Ref.modify(store, (current) => {
        const existing = current.get(name)
        if (existing !== undefined) return [existing, current]
        const fresh = new Map<string, Uint8Array>()
        const next = new Map(current)
        next.set(name, fresh)
        return [fresh, next]
      })

    const makeBucket = (name: string, data: Map<string, Uint8Array>): HostBucket => ({
      name,
      get: (key) =>
        Effect.gen(function* () {
          const err = yield* consumeEvErr("get")
          if (Option.isSome(err)) return yield* Effect.fail(err.value)
          const v = data.get(key)
          return v === undefined ? Option.none<Uint8Array>() : Option.some(v)
        }),
      set: (key, value) =>
        Effect.gen(function* () {
          const err = yield* consumeEvErr("set")
          if (Option.isSome(err)) return yield* Effect.fail(err.value)
          data.set(key, value)
        }),
      delete: (key) =>
        Effect.gen(function* () {
          const err = yield* consumeEvErr("delete")
          if (Option.isSome(err)) return yield* Effect.fail(err.value)
          data.delete(key)
        }),
      exists: (key) =>
        Effect.gen(function* () {
          const err = yield* consumeEvErr("exists")
          if (Option.isSome(err)) return yield* Effect.fail(err.value)
          return data.has(key)
        }),
      getMany: (keys) =>
        Effect.gen(function* () {
          const err = yield* consumeBatchErr("get-many")
          if (Option.isSome(err)) return yield* Effect.fail(err.value)
          const out: Array<Option.Option<Uint8Array>> = []
          for (const k of keys) {
            const v = data.get(k)
            out.push(v === undefined ? Option.none<Uint8Array>() : Option.some(v))
          }
          return out as ReadonlyArray<Option.Option<Uint8Array>>
        }),
      setMany: (entries) =>
        Effect.gen(function* () {
          const err = yield* consumeBatchErr("set-many")
          if (Option.isSome(err)) return yield* Effect.fail(err.value)
          for (const [k, v] of entries) data.set(k, v)
        }),
      deleteMany: (keys) =>
        Effect.gen(function* () {
          const err = yield* consumeBatchErr("delete-many")
          if (Option.isSome(err)) return yield* Effect.fail(err.value)
          for (const k of keys) data.delete(k)
        }),
      get keys() {
        return Effect.gen(function* () {
          const err = yield* consumeBatchErr("keys")
          if (Option.isSome(err)) return yield* Effect.fail(err.value)
          return Array.from(data.keys()) as ReadonlyArray<string>
        })
      },
    })

    const layer = Layer.succeed(
      KeyValueClient,
      KeyValueClient.of({
        openBucket: (name) =>
          Effect.gen(function* () {
            const queued = yield* Ref.modify(openErr, (current) => {
              if (Option.isSome(current) && current.value.name === name) {
                return [
                  Option.some(new KeyValueHostError(current.value.message, "openBucket")),
                  Option.none<{ name: string; message: string }>(),
                ]
              }
              return [Option.none<KeyValueHostError>(), current]
            })
            if (Option.isSome(queued)) return yield* Effect.fail(queued.value)
            const data = yield* getOrCreateBucket(name)
            return makeBucket(name, data)
          }),
      }),
    )

    return {
      layer,
      setOpenError: (name, message) => Ref.set(openErr, Option.some({ name, message })),
      setNextError: (op, message) => Ref.set(evErr, Option.some({ op, message })),
      setNextBatchError: (op, message) => Ref.set(batchErr, Option.some({ op, message })),
      buckets: Ref.get(store).pipe(
        Effect.map((m) => Array.from(m.keys()) as ReadonlyArray<string>),
      ),
    }
  })
