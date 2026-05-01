/**
 * Effect-idiomatic wrapper around the `wasi:keyvalue@0.1.0` host
 * interface.
 *
 * Only the `eventual` (single-key CRUD) and `eventual-batch`
 * (multi-key CRUD) interfaces are exposed. The `atomic` interface
 * (`increment` / `compare-and-swap`) and the entire `cache` interface
 * are NOT wrapped because they are currently `unimplemented!` in the
 * Golem host (calling them traps the worker). When the host
 * implements them, this module will be extended to cover them.
 *
 * **Example**
 *
 * ```ts
 * import { Effect, Schema } from "effect"
 * import { defineAgent, KeyValue, method } from "effect-golem"
 *
 * const User = Schema.Struct({ id: Schema.String, name: Schema.String })
 *
 * defineAgent({
 *   name: "Users",
 *   constructorParams: { name: Schema.String },
 *   methods: {
 *     put: method({ params: { id: Schema.String, name: Schema.String }, success: Schema.Void }),
 *     get: method({ params: { id: Schema.String }, success: Schema.Option(User) }),
 *   },
 *   impl: () =>
 *     Effect.gen(function* () {
 *       const bucket = yield* KeyValue.openBucket("users")
 *       const users = bucket.forSchema(User)
 *       return {
 *         put: ({ id, name }) => users.set(id, { id, name }),
 *         get: ({ id }) => users.get(id),
 *       }
 *     }),
 * })
 * ```
 *
 * @since 1.5.0
 */

import { Effect, Option, Scope, Schema } from "effect"
import { KeyValueClient, type HostBucket } from "./host/KeyValueClient.js"

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

const traceOf = (e: unknown): string => {
  if (e !== null && typeof e === "object") {
    const obj = e as { trace?: () => string; message?: string }
    if (typeof obj.trace === "function") {
      try {
        return obj.trace()
      } catch {
        /* fall through */
      }
    }
    if (typeof obj.message === "string") return obj.message
  }
  return String(e)
}

/**
 * Raised when any host call into `wasi:keyvalue@0.1.0` traps.
 *
 * The `trace` field is the verbatim driver-supplied string from the
 * host's `error.trace()` method (or, for non-Error host throws, the
 * stringified cause). Per the Golem source, this is opaque /
 * driver-specific (Redis / SQLite / Postgres / in-memory all produce
 * different formats) and should not be parsed.
 *
 * @since 1.5.0
 * @category errors
 */
export class KeyValueHostError {
  readonly _tag = "KeyValueHostError"
  readonly message: string
  readonly trace: string
  constructor(
    readonly cause: unknown,
    readonly operation: string,
  ) {
    const t = traceOf(cause)
    this.trace = t
    this.message = `KeyValueHostError(${operation}): ${t}`
  }
}

/**
 * Raised when {@link SchemaBucket} cannot parse a stored value as JSON.
 *
 * @since 1.5.0
 * @category errors
 */
export class KeyValueDecodeError {
  readonly _tag = "KeyValueDecodeError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `KeyValueDecodeError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

// ---------------------------------------------------------------------------
// TypeId
// ---------------------------------------------------------------------------

/**
 * `Symbol.for(...)` keyed identity stamp on every {@link Bucket}
 * instance. Lets cross-bundle code reliably check whether an unknown
 * value is a `Bucket` even when several copies of this module exist.
 *
 * @since 1.5.0
 * @category symbols
 */
export const BucketTypeId: unique symbol = Symbol.for(
  "effect-golem/keyvalue/Bucket",
) as BucketTypeId

/**
 * @since 1.5.0
 * @category symbols
 */
export type BucketTypeId = typeof BucketTypeId

/**
 * Type guard: true when `u` is a {@link Bucket}.
 *
 * @since 1.5.0
 * @category guards
 */
export const isBucket = (u: unknown): u is Bucket =>
  u !== null &&
  typeof u === "object" &&
  (u as { [k: symbol]: unknown })[BucketTypeId] === BucketTypeId

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * Handle on an open keyvalue bucket. Acquired via {@link openBucket}
 * inside an Effect `Scope`.
 *
 * The underlying WIT `bucket` resource has no explicit close method;
 * dropping the JS handle is enough.
 *
 * @since 1.5.0
 * @category models
 */
export interface Bucket {
  readonly [BucketTypeId]: BucketTypeId
  readonly name: string

  /** Get the bytes stored under `key`, or `Option.none()` if absent. */
  get(key: string): Effect.Effect<Option.Option<Uint8Array>, KeyValueHostError>
  /** Overwrite `key` with `value`, or insert if absent. */
  set(key: string, value: Uint8Array): Effect.Effect<void, KeyValueHostError>
  /** Remove `key`. No-op if absent. */
  delete(key: string): Effect.Effect<void, KeyValueHostError>
  /** Test whether `key` is present. */
  exists(key: string): Effect.Effect<boolean, KeyValueHostError>

  /**
   * Batch-get. Returned array length === `keys.length`; missing keys
   * surface as `Option.none()`. The host promises positional
   * alignment with the request.
   *
   * The host is all-or-nothing: a storage-layer error fails the whole
   * call, not individual entries.
   */
  getMany(
    keys: ReadonlyArray<string>,
  ): Effect.Effect<ReadonlyArray<Option.Option<Uint8Array>>, KeyValueHostError>
  /** Batch-set. Per-entry order is not guaranteed; failure is partial. */
  setMany(
    entries: ReadonlyArray<readonly [string, Uint8Array]>,
  ): Effect.Effect<void, KeyValueHostError>
  /** Batch-delete. Per-entry order is not guaranteed; failure is partial. */
  deleteMany(keys: ReadonlyArray<string>): Effect.Effect<void, KeyValueHostError>

  /** All keys currently present in the bucket. Order undefined. */
  readonly keys: Effect.Effect<ReadonlyArray<string>, KeyValueHostError>

  /**
   * Build a schema-typed view of this bucket. Values are JSON-encoded
   * via the supplied schema (`Schema.encodeUnknownEffect` →
   * `JSON.stringify` → UTF-8) on writes and reversed on reads.
   *
   * Decoding failures surface as either {@link KeyValueDecodeError}
   * (when the stored bytes are not valid JSON) or
   * `Schema.SchemaError` (when JSON parses but does not match the
   * schema). Encode failures surface as `Schema.SchemaError`.
   */
  forSchema<S extends Schema.Top>(schema: S): SchemaBucket<S>
}

/**
 * Schema-typed view returned by {@link Bucket.forSchema}. Mirrors
 * `effect/platform/KeyValueStore.SchemaStore` so users coming from
 * `@effect/platform` see the same shape.
 *
 * @since 1.5.0
 * @category models
 */
export interface SchemaBucket<S extends Schema.Top> {
  get(
    key: string,
  ): Effect.Effect<
    Option.Option<S["Type"]>,
    KeyValueHostError | KeyValueDecodeError | Schema.SchemaError,
    S["DecodingServices"]
  >
  set(
    key: string,
    value: S["Type"],
  ): Effect.Effect<
    void,
    KeyValueHostError | KeyValueDecodeError | Schema.SchemaError,
    S["EncodingServices"]
  >
  delete(key: string): Effect.Effect<void, KeyValueHostError>
  exists(key: string): Effect.Effect<boolean, KeyValueHostError>

  getMany(
    keys: ReadonlyArray<string>,
  ): Effect.Effect<
    ReadonlyArray<Option.Option<S["Type"]>>,
    KeyValueHostError | KeyValueDecodeError | Schema.SchemaError,
    S["DecodingServices"]
  >
  setMany(
    entries: ReadonlyArray<readonly [string, S["Type"]]>,
  ): Effect.Effect<
    void,
    KeyValueHostError | KeyValueDecodeError | Schema.SchemaError,
    S["EncodingServices"]
  >
  deleteMany(keys: ReadonlyArray<string>): Effect.Effect<void, KeyValueHostError>

  readonly keys: Effect.Effect<ReadonlyArray<string>, KeyValueHostError>
}

// ---------------------------------------------------------------------------
// Schema codec — UTF-8 JSON over `Schema`
// ---------------------------------------------------------------------------
//
// We compose `Schema.fromJsonString(schema)` (the canonical
// JSON-string transformer in Effect v4) with a strict UTF-8 codec
// at the bytes ↔ string boundary. JSON syntax errors and schema
// validation failures uniformly surface as `Schema.SchemaError`;
// only malformed UTF-8 (which the schema layer does not see)
// surfaces as the SDK-internal {@link KeyValueDecodeError}.

const encoder = new TextEncoder()
const strictDecoder = new TextDecoder("utf-8", { fatal: true })

const decodeUtf8 = (bytes: Uint8Array): Effect.Effect<string, KeyValueDecodeError> =>
  Effect.try({
    try: () => strictDecoder.decode(bytes),
    catch: (cause) => new KeyValueDecodeError(cause),
  })

// ---------------------------------------------------------------------------
// Bucket factory
// ---------------------------------------------------------------------------

const makeSchemaBucket = <S extends Schema.Top>(bucket: Bucket, schema: S): SchemaBucket<S> => {
  type A = S["Type"]
  const jsonSchema = Schema.fromJsonString(schema)
  const encodeJson = Schema.encodeUnknownEffect(jsonSchema)
  const decodeJson = Schema.decodeUnknownEffect(jsonSchema)

  const enc = (value: A) =>
    Effect.map(encodeJson(value), (jsonString) => encoder.encode(jsonString))

  const dec = (bytes: Uint8Array) =>
    Effect.gen(function* () {
      const jsonString = yield* decodeUtf8(bytes)
      return (yield* decodeJson(jsonString)) as A
    })

  const get: SchemaBucket<S>["get"] = (key) =>
    Effect.gen(function* () {
      const opt = yield* bucket.get(key)
      if (Option.isNone(opt)) return Option.none<A>()
      const value = yield* dec(opt.value)
      return Option.some(value)
    })

  const set: SchemaBucket<S>["set"] = (key, value) =>
    Effect.gen(function* () {
      const bytes = yield* enc(value)
      yield* bucket.set(key, bytes)
    })

  const getMany: SchemaBucket<S>["getMany"] = (keys) =>
    Effect.gen(function* () {
      const raws = yield* bucket.getMany(keys)
      const out: Array<Option.Option<A>> = []
      for (const raw of raws) {
        if (Option.isNone(raw)) {
          out.push(Option.none<A>())
        } else {
          const value = yield* dec(raw.value)
          out.push(Option.some(value))
        }
      }
      return out as ReadonlyArray<Option.Option<A>>
    })

  const setMany: SchemaBucket<S>["setMany"] = (entries) =>
    Effect.gen(function* () {
      const encoded: Array<readonly [string, Uint8Array]> = []
      for (const [k, v] of entries) {
        const bytes = yield* enc(v)
        encoded.push([k, bytes])
      }
      yield* bucket.setMany(encoded)
    })

  return {
    get,
    set,
    delete: (key: string) => bucket.delete(key),
    exists: (key: string) => bucket.exists(key),
    getMany,
    setMany,
    deleteMany: (keys: ReadonlyArray<string>) => bucket.deleteMany(keys),
    get keys() {
      return bucket.keys
    },
  }
}

const makeBucket = (host: HostBucket): Bucket => {
  const self: Bucket = {
    [BucketTypeId]: BucketTypeId,
    name: host.name,
    get: (key) => host.get(key),
    set: (key, value) => host.set(key, value),
    delete: (key) => host.delete(key),
    exists: (key) => host.exists(key),
    getMany: (keys) => host.getMany(keys),
    setMany: (entries) => host.setMany(entries),
    deleteMany: (keys) => host.deleteMany(keys),
    get keys() {
      return host.keys
    },
    forSchema<S extends Schema.Top>(schema: S) {
      return makeSchemaBucket(self, schema)
    },
  }
  return self
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Open a bucket by name. Scoped: dropping the surrounding scope
 * releases the JS handle (the WIT resource has no explicit close).
 *
 * Failures from the host's `bucket.open-bucket` (for example, a
 * malformed name or backend rejection) surface as
 * {@link KeyValueHostError}.
 *
 * @since 1.5.0
 * @category constructors
 */
export const openBucket = (
  name: string,
): Effect.Effect<Bucket, KeyValueHostError, Scope.Scope | KeyValueClient> =>
  Effect.gen(function* () {
    const client = yield* KeyValueClient
    const host = yield* client.openBucket(name)
    return makeBucket(host)
  })
