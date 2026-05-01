/**
 * Effect-idiomatic wrapper around `wasi:blobstore/blobstore` +
 * `wasi:blobstore/container` + `wasi:blobstore/types`.
 *
 * **Example**
 *
 * ```ts
 * import { Effect, Schema, Stream } from "effect"
 * import { Blobstore, defineAgent, method } from "effect-golem"
 *
 * defineAgent({
 *   name: "Photos",
 *   constructorParams: { name: Schema.String },
 *   methods: {
 *     upload: method({ params: { key: Schema.String, body: Schema.Uint8Array }, success: Schema.Void }),
 *     download: method({ params: { key: Schema.String }, success: Schema.Uint8Array }),
 *     list: method({ params: {}, success: Schema.Array(Schema.String) }),
 *   },
 *   impl: ({ name }) =>
 *     Effect.gen(function* () {
 *       const photos = yield* Blobstore.getOrCreateContainer(name)
 *       return {
 *         upload: ({ key, body }) => photos.writeData(key, body),
 *         download: ({ key }) => photos.getData(key),
 *         list: () => Stream.runCollect(photos.listObjects).pipe(Effect.map((c) => Array.from(c))),
 *       }
 *     }),
 * })
 * ```
 *
 * @since 0.1.0
 */

import { Effect, Schema, Scope, Stream } from "effect"
import { BlobstoreClient, type HostContainer } from "./host/BlobstoreClient.js"

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

const messageOf = (e: unknown): string => {
  if (e !== null && typeof e === "object") {
    const obj = e as { message?: string }
    if (typeof obj.message === "string") return obj.message
  }
  if (typeof e === "string") return e
  return String(e)
}

/**
 * Raised when any host call into `wasi:blobstore/*` traps. Unlike
 * the keyvalue host the blobstore error type is a plain `string`
 * (per the WIT), so {@link trace} just carries that string verbatim.
 *
 * @since 0.1.0
 * @category errors
 */
export class BlobstoreHostError {
  readonly _tag = "BlobstoreHostError"
  readonly message: string
  readonly trace: string
  constructor(
    readonly cause: unknown,
    readonly operation: string,
  ) {
    const t = messageOf(cause)
    this.trace = t
    this.message = `BlobstoreHostError(${operation}): ${t}`
  }
}

/**
 * Raised when {@link SchemaContainer} cannot parse a stored object as JSON.
 *
 * @since 0.1.0
 * @category errors
 */
export class BlobstoreDecodeError {
  readonly _tag = "BlobstoreDecodeError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `BlobstoreDecodeError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

// ---------------------------------------------------------------------------
// TypeId
// ---------------------------------------------------------------------------

/**
 * `Symbol.for(...)` keyed identity stamp on every {@link Container}
 * instance. Lets cross-bundle code reliably check whether an unknown
 * value is a `Container` even when several copies of this module exist.
 *
 * @since 0.1.0
 * @category symbols
 */
export const ContainerTypeId: unique symbol = Symbol.for(
  "effect-golem/blobstore/Container",
) as ContainerTypeId

/**
 * @since 0.1.0
 * @category symbols
 */
export type ContainerTypeId = typeof ContainerTypeId

/**
 * Type guard: true when `u` is a {@link Container}.
 *
 * @since 0.1.0
 * @category guards
 */
export const isContainer = (u: unknown): u is Container =>
  u !== null &&
  typeof u === "object" &&
  (u as { [k: symbol]: unknown })[ContainerTypeId] === ContainerTypeId

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * Object identifier — `(containerName, objectName)` pair.
 *
 * @since 0.1.0
 * @category models
 */
export interface ObjectId {
  readonly container: string
  readonly object: string
}

/**
 * Container metadata. The WIT `created-at` field is a `u64` whose
 * unit is undocumented in the spec; the Golem host populates it as
 * Unix milliseconds (verified in `golem-service-base/src/storage/blob/`),
 * so we expose it here as both a JS `Date` (`createdAt`) and the raw
 * bigint (`createdAtMillis`).
 *
 * Note: in the host, this field is actually `last_modified_at` —
 * there is no separate creation time on object storage backends.
 *
 * @since 0.1.0
 * @category models
 */
export interface ContainerMetadata {
  readonly name: string
  readonly createdAt: Date
  readonly createdAtMillis: bigint
}

/**
 * Object metadata. Same caveats as {@link ContainerMetadata}: the
 * `created-at` field is populated from `last_modified_at` in the
 * Golem host (S3 `LastModified` / filesystem `mtime`), in Unix
 * milliseconds — there is no separate creation time on object
 * storage backends.
 *
 * @since 0.1.0
 * @category models
 */
export interface ObjectMetadata {
  readonly name: string
  readonly container: string
  readonly createdAt: Date
  readonly createdAtMillis: bigint
  readonly size: bigint
}

/**
 * Optional inclusive byte range for {@link Container.getData}.
 *
 * **Backend caveat.** The Golem host implementation diverges across
 * backends:
 * - in-memory + filesystem: `{start..end}` is treated as a Rust
 *   half-open range (end exclusive);
 * - S3: `Range: bytes=start-end` is sent (end inclusive).
 *
 * The WIT spec says "Start and end offsets are inclusive". Until the
 * host fixes the in-memory/fs backends, ranged reads are not
 * portable — prefer reading whole objects.
 *
 * @since 0.1.0
 * @category models
 */
export interface ByteRange {
  readonly start: bigint
  readonly end: bigint
}

/**
 * Handle on an open blob container. Acquired via {@link createContainer},
 * {@link getContainer} or {@link getOrCreateContainer} inside an Effect
 * `Scope`.
 *
 * @since 0.1.0
 * @category models
 */
export interface Container {
  readonly [ContainerTypeId]: ContainerTypeId
  /** The container's name (cached at acquire time). */
  readonly name: string

  readonly info: Effect.Effect<ContainerMetadata, BlobstoreHostError>
  readonly clear: Effect.Effect<void, BlobstoreHostError>

  /** Read the entire object's bytes. */
  getData(name: string): Effect.Effect<Uint8Array, BlobstoreHostError>
  /** Read a byte range (see caveat on {@link ByteRange}). */
  getData(name: string, range: ByteRange): Effect.Effect<Uint8Array, BlobstoreHostError>

  /** Create or replace `name` with `data`. */
  writeData(name: string, data: Uint8Array): Effect.Effect<void, BlobstoreHostError>

  /** True if the named object exists in this container. */
  hasObject(name: string): Effect.Effect<boolean, BlobstoreHostError>
  /** Metadata for the named object. Fails if the object does not exist. */
  objectInfo(name: string): Effect.Effect<ObjectMetadata, BlobstoreHostError>
  /** Delete the named object. Does NOT fail if it does not exist. */
  deleteObject(name: string): Effect.Effect<void, BlobstoreHostError>
  /** Delete multiple objects. */
  deleteObjects(names: ReadonlyArray<string>): Effect.Effect<void, BlobstoreHostError>

  /**
   * Stream of object names in this container. NOTE: the Golem host
   * eagerly fetches the full name list at the moment `list-objects`
   * is called and pins it to the oplog — paging through the
   * `Stream` only consumes from the in-memory snapshot. Order is
   * not guaranteed (the host pops from the tail of its internal
   * `Vec`). Page size: 256.
   */
  readonly listObjects: Stream.Stream<string, BlobstoreHostError>

  /**
   * Build a schema-typed view of this container. Object bodies are
   * JSON-encoded via the supplied schema.
   */
  forSchema<S extends Schema.Top>(schema: S): SchemaContainer<S>
}

/**
 * Schema-typed view returned by {@link Container.forSchema}.
 *
 * @since 0.1.0
 * @category models
 */
export interface SchemaContainer<S extends Schema.Top> {
  getData(
    name: string,
  ): Effect.Effect<
    S["Type"],
    BlobstoreHostError | BlobstoreDecodeError | Schema.SchemaError,
    S["DecodingServices"]
  >
  writeData(
    name: string,
    value: S["Type"],
  ): Effect.Effect<
    void,
    BlobstoreHostError | BlobstoreDecodeError | Schema.SchemaError,
    S["EncodingServices"]
  >
  hasObject(name: string): Effect.Effect<boolean, BlobstoreHostError>
  objectInfo(name: string): Effect.Effect<ObjectMetadata, BlobstoreHostError>
  deleteObject(name: string): Effect.Effect<void, BlobstoreHostError>
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

const decodeContainerMetadata = (m: {
  readonly name: string
  readonly createdAtMillis: bigint
}): ContainerMetadata => ({
  name: m.name,
  createdAt: new Date(Number(m.createdAtMillis)),
  createdAtMillis: m.createdAtMillis,
})

const decodeObjectMetadata = (m: {
  readonly name: string
  readonly container: string
  readonly createdAtMillis: bigint
  readonly size: bigint
}): ObjectMetadata => ({
  name: m.name,
  container: m.container,
  createdAt: new Date(Number(m.createdAtMillis)),
  createdAtMillis: m.createdAtMillis,
  size: m.size,
})

// ---------------------------------------------------------------------------
// Schema codec — UTF-8 JSON over `Schema`
// ---------------------------------------------------------------------------
//
// We compose `Schema.fromJsonString(schema)` (the canonical
// JSON-string transformer in Effect v4) with a strict UTF-8 codec
// at the bytes ↔ string boundary. JSON syntax errors and schema
// validation failures uniformly surface as `Schema.SchemaError`;
// only malformed UTF-8 (which the schema layer does not see)
// surfaces as the SDK-internal {@link BlobstoreDecodeError}.

const encoder = new TextEncoder()
const strictDecoder = new TextDecoder("utf-8", { fatal: true })

const decodeUtf8 = (bytes: Uint8Array): Effect.Effect<string, BlobstoreDecodeError> =>
  Effect.try({
    try: () => strictDecoder.decode(bytes),
    catch: (cause) => new BlobstoreDecodeError(cause),
  })

// ---------------------------------------------------------------------------
// SchemaContainer factory
// ---------------------------------------------------------------------------

const makeSchemaContainer = <S extends Schema.Top>(
  container: Container,
  schema: S,
): SchemaContainer<S> => {
  type A = S["Type"]
  const jsonSchema = Schema.fromJsonString(schema)
  const encodeJson = Schema.encodeUnknownEffect(jsonSchema)
  const decodeJson = Schema.decodeUnknownEffect(jsonSchema)

  const getData: SchemaContainer<S>["getData"] = (name) =>
    Effect.gen(function* () {
      const bytes = yield* container.getData(name)
      const jsonString = yield* decodeUtf8(bytes)
      return (yield* decodeJson(jsonString)) as A
    })

  const writeData: SchemaContainer<S>["writeData"] = (name, value) =>
    Effect.gen(function* () {
      const jsonString = yield* encodeJson(value)
      yield* container.writeData(name, encoder.encode(jsonString))
    })

  return {
    getData,
    writeData,
    hasObject: (name: string) => container.hasObject(name),
    objectInfo: (name: string) => container.objectInfo(name),
    deleteObject: (name: string) => container.deleteObject(name),
  }
}

// ---------------------------------------------------------------------------
// Container factory — wraps a HostContainer with the public surface.
// ---------------------------------------------------------------------------

const makeContainer = (host: HostContainer): Container => {
  const info: Container["info"] = host.info.pipe(Effect.map(decodeContainerMetadata))

  const objectInfo: Container["objectInfo"] = (name) =>
    host.objectInfo(name).pipe(Effect.map(decodeObjectMetadata))

  const getData: Container["getData"] = (name: string, range?: ByteRange) =>
    Effect.gen(function* () {
      // Explicit user range — pass through verbatim. Backend semantics
      // diverge (see ByteRange JSDoc); the SDK does NOT massage them.
      if (range !== undefined) {
        return yield* host.getData(name, range)
      }

      // Whole-object read. The WIT spec says `end` is inclusive,
      // but the Golem in-memory and filesystem backends implement
      // it as Rust-exclusive. We tolerate both:
      //   - empty object       → return Uint8Array(0) without a host call
      //   - try inclusive  end = size - 1
      //   - if backend returned size - 1 bytes (the in-mem/fs bug),
      //     retry with end = size to recover the last byte.
      const meta = yield* host.objectInfo(name)
      const size = meta.size
      if (size === 0n) {
        return new Uint8Array(0)
      }
      const firstAttempt = yield* host.getData(name, { start: 0n, end: size - 1n })
      if (BigInt(firstAttempt.length) === size) {
        return firstAttempt
      }
      // Backend treats `end` as exclusive — replay with end = size.
      return yield* host.getData(name, { start: 0n, end: size })
    })

  const self: Container = {
    [ContainerTypeId]: ContainerTypeId,
    name: host.name,
    info,
    clear: host.clear,
    getData: getData as Container["getData"],
    writeData: (name, data) => host.writeData(name, data),
    hasObject: (name) => host.hasObject(name),
    objectInfo,
    deleteObject: (name) => host.deleteObject(name),
    deleteObjects: (names) => host.deleteObjects(names),
    get listObjects() {
      return host.listObjects
    },
    forSchema<S extends Schema.Top>(schema: S) {
      return makeSchemaContainer(self, schema)
    },
  }
  return self
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Create a new empty container. Fails with {@link BlobstoreHostError}
 * if a container with the same name already exists.
 *
 * @since 0.1.0
 * @category constructors
 */
export const createContainer = (
  name: string,
): Effect.Effect<Container, BlobstoreHostError, Scope.Scope | BlobstoreClient> =>
  Effect.gen(function* () {
    const client = yield* BlobstoreClient
    const host = yield* client.createContainer(name)
    return makeContainer(host)
  })

/**
 * Open an existing container by name. Fails with {@link BlobstoreHostError}
 * if the container does not exist.
 *
 * @since 0.1.0
 * @category constructors
 */
export const getContainer = (
  name: string,
): Effect.Effect<Container, BlobstoreHostError, Scope.Scope | BlobstoreClient> =>
  Effect.gen(function* () {
    const client = yield* BlobstoreClient
    const host = yield* client.getContainer(name)
    return makeContainer(host)
  })

/**
 * Convenience: open an existing container by name, falling back to
 * `createContainer(name)` if it does not exist.
 *
 * Idempotent — safe to call from `defineAgent` `impl` after a
 * snapshot load.
 *
 * @since 0.1.0
 * @category constructors
 */
export const getOrCreateContainer = (
  name: string,
): Effect.Effect<Container, BlobstoreHostError, Scope.Scope | BlobstoreClient> =>
  Effect.gen(function* () {
    const client = yield* BlobstoreClient
    const host = yield* client.getOrCreateContainer(name)
    return makeContainer(host)
  })

/**
 * True if a container with the given name exists.
 *
 * @since 0.1.0
 * @category operations
 */
export const containerExists = (
  name: string,
): Effect.Effect<boolean, BlobstoreHostError, BlobstoreClient> =>
  Effect.gen(function* () {
    const client = yield* BlobstoreClient
    return yield* client.containerExists(name)
  })

/**
 * Delete a container and all of its objects.
 *
 * @since 0.1.0
 * @category operations
 */
export const deleteContainer = (
  name: string,
): Effect.Effect<void, BlobstoreHostError, BlobstoreClient> =>
  Effect.gen(function* () {
    const client = yield* BlobstoreClient
    return yield* client.deleteContainer(name)
  })

/**
 * Copy an object to the same or a different container. Overwrites the destination.
 *
 * @since 0.1.0
 * @category operations
 */
export const copyObject = (
  src: ObjectId,
  dest: ObjectId,
): Effect.Effect<void, BlobstoreHostError, BlobstoreClient> =>
  Effect.gen(function* () {
    const client = yield* BlobstoreClient
    return yield* client.copyObject(src, dest)
  })

/**
 * Move (rename) an object to the same or a different container. Overwrites the destination.
 *
 * @since 0.1.0
 * @category operations
 */
export const moveObject = (
  src: ObjectId,
  dest: ObjectId,
): Effect.Effect<void, BlobstoreHostError, BlobstoreClient> =>
  Effect.gen(function* () {
    const client = yield* BlobstoreClient
    return yield* client.moveObject(src, dest)
  })
