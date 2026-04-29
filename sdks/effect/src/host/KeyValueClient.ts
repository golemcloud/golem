/**
 * Host service for `wasi:keyvalue/types@0.1.0` +
 * `wasi:keyvalue/eventual@0.1.0` +
 * `wasi:keyvalue/eventual-batch@0.1.0`. Wraps the host's keyvalue
 * surface as Effect-typed methods so SDK code can acquire / mutate
 * buckets via DI rather than reaching directly into the WIT
 * specifier imports.
 *
 * The `IncomingValue` / `OutgoingValue` resource plumbing
 * (`incomingValueConsumeSync` / `outgoingValueWriteBodySync`) lives
 * inside the Live impl; consumers see a plain `Uint8Array`.
 *
 * Resource handles are *not* exposed; the service surface is purely
 * by-value (records of `Effect`-returning methods). The public
 * {@link import("../keyvalue.js").Bucket} type wraps a
 * {@link HostBucket} returned from `openBucket` and adds the
 * `BucketTypeId` stamp + the schema-typed `forSchema` view.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Effect, Layer, Option } from "effect"
import type * as Scope from "effect/Scope"
import { KeyValueHostError } from "../keyvalue.js"
import * as KvBatch from "wasi:keyvalue/eventual-batch@0.1.0"
import * as KvEventual from "wasi:keyvalue/eventual@0.1.0"
import * as KvTypes from "wasi:keyvalue/types@0.1.0"

/**
 * Per-bucket surface. Mirrors the eventual + eventual-batch host
 * functions, with the `IncomingValue` / `OutgoingValue` chunking
 * folded in.
 */
export interface HostBucket {
  /** The bucket's name (cached at acquire time). */
  readonly name: string

  get(key: string): Effect.Effect<Option.Option<Uint8Array>, KeyValueHostError>
  set(key: string, value: Uint8Array): Effect.Effect<void, KeyValueHostError>
  delete(key: string): Effect.Effect<void, KeyValueHostError>
  exists(key: string): Effect.Effect<boolean, KeyValueHostError>

  getMany(
    keys: ReadonlyArray<string>,
  ): Effect.Effect<ReadonlyArray<Option.Option<Uint8Array>>, KeyValueHostError>
  setMany(
    entries: ReadonlyArray<readonly [string, Uint8Array]>,
  ): Effect.Effect<void, KeyValueHostError>
  deleteMany(keys: ReadonlyArray<string>): Effect.Effect<void, KeyValueHostError>
  readonly keys: Effect.Effect<ReadonlyArray<string>, KeyValueHostError>
}

export interface KeyValueClientShape {
  /**
   * Open a bucket by name. Scoped: dropping the surrounding scope
   * releases the JS handle (the WIT bucket resource has no explicit
   * close method).
   */
  openBucket(name: string): Effect.Effect<HostBucket, KeyValueHostError, Scope.Scope>
}

export class KeyValueClient extends Context.Service<KeyValueClient, KeyValueClientShape>()(
  "effect-golem/host/KeyValue",
) {}

// ---------------------------------------------------------------------------
// Live impl — calls into wasi:keyvalue/* verbatim.
// ---------------------------------------------------------------------------

const writeOutgoing = (
  bytes: Uint8Array,
): Effect.Effect<KvTypes.OutgoingValue, KeyValueHostError> =>
  Effect.try({
    try: () => {
      const ov = KvTypes.OutgoingValue.newOutgoingValue()
      ov.outgoingValueWriteBodySync(bytes)
      return ov
    },
    catch: (cause) => new KeyValueHostError(cause, "outgoingValueWriteBodySync"),
  })

const consumeIncoming = (iv: KvTypes.IncomingValue): Effect.Effect<Uint8Array, KeyValueHostError> =>
  Effect.try({
    try: () => iv.incomingValueConsumeSync(),
    catch: (cause) => new KeyValueHostError(cause, "incomingValueConsumeSync"),
  })

const makeHostBucket = (name: string, handle: KvTypes.Bucket): HostBucket => {
  const get: HostBucket["get"] = (key) =>
    Effect.gen(function* () {
      const iv = yield* Effect.try({
        try: () => KvEventual.get(handle, key),
        catch: (cause) => new KeyValueHostError(cause, "eventual.get"),
      })
      if (iv === undefined || iv === null) return Option.none<Uint8Array>()
      const bytes = yield* consumeIncoming(iv)
      return Option.some(bytes)
    })

  const set: HostBucket["set"] = (key, value) =>
    Effect.gen(function* () {
      const ov = yield* writeOutgoing(value)
      yield* Effect.try({
        try: () => KvEventual.set(handle, key, ov),
        catch: (cause) => new KeyValueHostError(cause, "eventual.set"),
      })
    })

  const del: HostBucket["delete"] = (key) =>
    Effect.try({
      try: () => KvEventual.delete_(handle, key),
      catch: (cause) => new KeyValueHostError(cause, "eventual.delete"),
    })

  const exists: HostBucket["exists"] = (key) =>
    Effect.try({
      try: () => KvEventual.exists(handle, key),
      catch: (cause) => new KeyValueHostError(cause, "eventual.exists"),
    })

  const getMany: HostBucket["getMany"] = (keys) =>
    Effect.gen(function* () {
      const raw = yield* Effect.try({
        try: () =>
          KvBatch.getMany(handle, [...keys]) as ReadonlyArray<KvTypes.IncomingValue | undefined>,
        catch: (cause) => new KeyValueHostError(cause, "eventual-batch.get-many"),
      })
      const out: Array<Option.Option<Uint8Array>> = []
      for (const iv of raw) {
        if (iv === undefined || iv === null) {
          out.push(Option.none<Uint8Array>())
        } else {
          const bytes = yield* consumeIncoming(iv)
          out.push(Option.some(bytes))
        }
      }
      return out as ReadonlyArray<Option.Option<Uint8Array>>
    })

  const setMany: HostBucket["setMany"] = (entries) =>
    Effect.gen(function* () {
      const pairs: Array<[string, KvTypes.OutgoingValue]> = []
      for (const [k, v] of entries) {
        const ov = yield* writeOutgoing(v)
        pairs.push([k, ov])
      }
      yield* Effect.try({
        try: () => KvBatch.setMany(handle, pairs),
        catch: (cause) => new KeyValueHostError(cause, "eventual-batch.set-many"),
      })
    })

  const deleteMany: HostBucket["deleteMany"] = (keys) =>
    Effect.try({
      try: () => KvBatch.deleteMany(handle, [...keys]),
      catch: (cause) => new KeyValueHostError(cause, "eventual-batch.delete-many"),
    })

  return {
    name,
    get,
    set,
    delete: del,
    exists,
    getMany,
    setMany,
    deleteMany,
    get keys() {
      return Effect.try({
        try: () => KvBatch.keys(handle),
        catch: (cause) => new KeyValueHostError(cause, "eventual-batch.keys"),
      })
    },
  }
}

const liveOpenBucket = (name: string): Effect.Effect<HostBucket, KeyValueHostError, Scope.Scope> =>
  Effect.gen(function* () {
    const handle = yield* Effect.acquireRelease(
      Effect.try({
        try: () => KvTypes.Bucket.openBucket(name),
        catch: (cause) => new KeyValueHostError(cause, "openBucket"),
      }),
      // The WIT bucket resource has no .close() method; rely on JS GC
      // when the scope's reference is dropped.
      () => Effect.void,
    )
    return makeHostBucket(name, handle)
  })

export const KeyValueLive: Layer.Layer<KeyValueClient> = Layer.succeed(
  KeyValueClient,
  KeyValueClient.of({
    openBucket: liveOpenBucket,
  }),
)
