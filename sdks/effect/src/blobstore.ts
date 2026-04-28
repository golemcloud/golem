/**
 * Effect-idiomatic wrapper around `wasi:blobstore/blobstore` +
 * `wasi:blobstore/container` + `wasi:blobstore/types`.
 *
 * Authoring example:
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
 */

import { Effect, Option, Schema, Scope, Stream } from "effect"
import * as Blob from "wasi:blobstore/blobstore"
import * as ContainerNS from "wasi:blobstore/container"
import * as Types from "wasi:blobstore/types"
import * as Streams from "wasi:io/streams@0.2.3"

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

/** Raised when {@link SchemaContainer} cannot parse a stored object as JSON. */
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

export const ContainerTypeId: unique symbol = Symbol.for(
  "effect-golem/blobstore/Container",
) as ContainerTypeId
export type ContainerTypeId = typeof ContainerTypeId

export const isContainer = (u: unknown): u is Container =>
  u !== null &&
  typeof u === "object" &&
  (u as { [k: symbol]: unknown })[ContainerTypeId] === ContainerTypeId

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/** Object identifier — `(containerName, objectName)` pair. */
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
 */
export interface ByteRange {
  readonly start: bigint
  readonly end: bigint
}

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

/** Schema-typed view returned by {@link Container.forSchema}. */
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

const consumeIncomingSync = (
  iv: Types.IncomingValue,
): Effect.Effect<Uint8Array, BlobstoreHostError> =>
  Effect.try({
    try: () => iv.incomingValueConsumeSync(),
    catch: (cause) => new BlobstoreHostError(cause, "incomingValueConsumeSync"),
  })

/**
 * Push `bytes` into a wasi:io output-stream in 4096-byte chunks.
 * Mirrors the pseudo-code from the wasi:io spec for
 * `blocking-write-and-flush`.
 */
const writeAllToOutputStream = (
  stream: Streams.OutputStream,
  bytes: Uint8Array,
): Effect.Effect<void, BlobstoreHostError> =>
  Effect.try({
    try: () => {
      const total = bytes.length
      let offset = 0
      while (offset < total) {
        const remaining = total - offset
        const chunkLen = remaining > 4096 ? 4096 : remaining
        const chunk = bytes.subarray(offset, offset + chunkLen)
        stream.blockingWriteAndFlush(chunk)
        offset += chunkLen
      }
    },
    catch: (cause) => new BlobstoreHostError(cause, "outputStream.blockingWriteAndFlush"),
  })

const buildOutgoingValue = (
  bytes: Uint8Array,
): Effect.Effect<Types.OutgoingValue, BlobstoreHostError> =>
  Effect.gen(function* () {
    const ov = yield* Effect.try({
      try: () => Types.OutgoingValue.newOutgoingValue(),
      catch: (cause) => new BlobstoreHostError(cause, "newOutgoingValue"),
    })
    const stream = yield* Effect.try({
      try: () => ov.outgoingValueWriteBody(),
      catch: (cause) => new BlobstoreHostError(cause, "outgoingValueWriteBody"),
    })
    yield* writeAllToOutputStream(stream, bytes)
    return ov
  })

const decodeMetadata = (
  m: ContainerNS.ContainerMetadata | Types.ContainerMetadata,
): ContainerMetadata => {
  const ms = m.createdAt
  return {
    name: m.name,
    createdAt: new Date(Number(ms)),
    createdAtMillis: ms,
  }
}

const decodeObjectMetadata = (m: Types.ObjectMetadata): ObjectMetadata => {
  const ms = m.createdAt
  return {
    name: m.name,
    container: m.container,
    createdAt: new Date(Number(ms)),
    createdAtMillis: ms,
    size: m.size,
  }
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
// surfaces as the SDK-internal {@link BlobstoreDecodeError}.

const encoder = new TextEncoder()
const strictDecoder = new TextDecoder("utf-8", { fatal: true })

const decodeUtf8 = (bytes: Uint8Array): Effect.Effect<string, BlobstoreDecodeError> =>
  Effect.try({
    try: () => strictDecoder.decode(bytes),
    catch: (cause) => new BlobstoreDecodeError(cause),
  })

// ---------------------------------------------------------------------------
// listObjects — Stream over the host's stream-object-names resource
// ---------------------------------------------------------------------------

const LIST_PAGE_SIZE = 256n

const listObjectsStream = (
  handle: ContainerNS.Container,
): Stream.Stream<string, BlobstoreHostError> =>
  Stream.unwrap(
    Effect.gen(function* () {
      const iter = yield* Effect.try({
        try: () => handle.listObjects(),
        catch: (cause) => new BlobstoreHostError(cause, "container.listObjects"),
      })
      return Stream.paginate(false as boolean, (done) => {
        if (done) {
          return Effect.succeed([[] as ReadonlyArray<string>, Option.none<boolean>()] as const)
        }
        return Effect.try({
          try: () => iter.readStreamObjectNames(LIST_PAGE_SIZE),
          catch: (cause) => new BlobstoreHostError(cause, "streamObjectNames.read"),
        }).pipe(
          Effect.map(
            ([names, end]) =>
              [
                names as ReadonlyArray<string>,
                end ? Option.none<boolean>() : Option.some(false),
              ] as const,
          ),
        )
      })
    }),
  )

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
// Container factory
// ---------------------------------------------------------------------------

const makeContainer = (name: string, handle: ContainerNS.Container): Container => {
  const info: Container["info"] = Effect.gen(function* () {
    const m = yield* Effect.try({
      try: () => handle.info(),
      catch: (cause) => new BlobstoreHostError(cause, "container.info"),
    })
    return decodeMetadata(m)
  })

  const clear: Container["clear"] = Effect.try({
    try: () => handle.clear(),
    catch: (cause) => new BlobstoreHostError(cause, "container.clear"),
  })

  const readRange = (
    name: string,
    start: bigint,
    end: bigint,
  ): Effect.Effect<Uint8Array, BlobstoreHostError> =>
    Effect.gen(function* () {
      const iv = yield* Effect.try({
        try: () => handle.getData(name, start, end),
        catch: (cause) => new BlobstoreHostError(cause, "container.getData"),
      })
      return yield* consumeIncomingSync(iv)
    })

  const getData: Container["getData"] = (name: string, range?: ByteRange) =>
    Effect.gen(function* () {
      // Explicit user range — pass through verbatim. Backend semantics
      // diverge (see ByteRange JSDoc); the SDK does NOT massage them.
      if (range !== undefined) {
        return yield* readRange(name, range.start, range.end)
      }

      // Whole-object read. The WIT spec says `end` is inclusive,
      // but the Golem in-memory and filesystem backends implement
      // it as Rust-exclusive. We tolerate both:
      //   - empty object       → return Uint8Array(0) without a host call
      //   - try inclusive  end = size - 1
      //   - if backend returned size - 1 bytes (the in-mem/fs bug),
      //     retry with end = size to recover the last byte.
      const meta = yield* Effect.try({
        try: () => handle.objectInfo(name),
        catch: (cause) => new BlobstoreHostError(cause, "container.objectInfo"),
      })
      const size = meta.size
      if (size === 0n) {
        return new Uint8Array(0)
      }
      const firstAttempt = yield* readRange(name, 0n, size - 1n)
      if (BigInt(firstAttempt.length) === size) {
        return firstAttempt
      }
      // Backend treats `end` as exclusive — replay with end = size.
      return yield* readRange(name, 0n, size)
    })

  const writeData: Container["writeData"] = (name, data) =>
    Effect.gen(function* () {
      const ov = yield* buildOutgoingValue(data)
      yield* Effect.try({
        try: () => handle.writeData(name, ov),
        catch: (cause) => new BlobstoreHostError(cause, "container.writeData"),
      })
    })

  const hasObject: Container["hasObject"] = (name) =>
    Effect.try({
      try: () => handle.hasObject(name),
      catch: (cause) => new BlobstoreHostError(cause, "container.hasObject"),
    })

  const objectInfo: Container["objectInfo"] = (name) =>
    Effect.gen(function* () {
      const m = yield* Effect.try({
        try: () => handle.objectInfo(name),
        catch: (cause) => new BlobstoreHostError(cause, "container.objectInfo"),
      })
      return decodeObjectMetadata(m)
    })

  const deleteObject: Container["deleteObject"] = (name) =>
    Effect.try({
      try: () => handle.deleteObject(name),
      catch: (cause) => new BlobstoreHostError(cause, "container.deleteObject"),
    })

  const deleteObjects: Container["deleteObjects"] = (names) =>
    Effect.try({
      try: () => handle.deleteObjects([...names]),
      catch: (cause) => new BlobstoreHostError(cause, "container.deleteObjects"),
    })

  const self: Container = {
    [ContainerTypeId]: ContainerTypeId,
    name,
    info,
    clear,
    getData: getData as Container["getData"],
    writeData,
    hasObject,
    objectInfo,
    deleteObject,
    deleteObjects,
    get listObjects() {
      return listObjectsStream(handle)
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

const acquireContainer = (
  name: string,
  handle: ContainerNS.Container,
): Effect.Effect<Container, never, Scope.Scope> =>
  Effect.acquireRelease(Effect.succeed(makeContainer(name, handle)), () => Effect.void)

/**
 * Create a new empty container. Fails with {@link BlobstoreHostError}
 * if a container with the same name already exists.
 */
export const createContainer = (
  name: string,
): Effect.Effect<Container, BlobstoreHostError, Scope.Scope> =>
  Effect.gen(function* () {
    const handle = yield* Effect.try({
      try: () => Blob.createContainer(name),
      catch: (cause) => new BlobstoreHostError(cause, "createContainer"),
    })
    return yield* acquireContainer(name, handle)
  })

/**
 * Open an existing container by name. Fails with {@link BlobstoreHostError}
 * if the container does not exist.
 */
export const getContainer = (
  name: string,
): Effect.Effect<Container, BlobstoreHostError, Scope.Scope> =>
  Effect.gen(function* () {
    const handle = yield* Effect.try({
      try: () => Blob.getContainer(name),
      catch: (cause) => new BlobstoreHostError(cause, "getContainer"),
    })
    return yield* acquireContainer(name, handle)
  })

/**
 * Convenience: open an existing container by name, falling back to
 * `createContainer(name)` if it does not exist.
 *
 * Idempotent — safe to call from `defineAgent` `impl` after a
 * snapshot load.
 */
export const getOrCreateContainer = (
  name: string,
): Effect.Effect<Container, BlobstoreHostError, Scope.Scope> =>
  Effect.gen(function* () {
    const exists = yield* containerExists(name)
    if (exists) return yield* getContainer(name)
    return yield* createContainer(name)
  })

/** True if a container with the given name exists. */
export const containerExists = (name: string): Effect.Effect<boolean, BlobstoreHostError> =>
  Effect.try({
    try: () => Blob.containerExists(name),
    catch: (cause) => new BlobstoreHostError(cause, "containerExists"),
  })

/** Delete a container and all of its objects. */
export const deleteContainer = (name: string): Effect.Effect<void, BlobstoreHostError> =>
  Effect.try({
    try: () => Blob.deleteContainer(name),
    catch: (cause) => new BlobstoreHostError(cause, "deleteContainer"),
  })

/** Copy an object to the same or a different container. Overwrites the destination. */
export const copyObject = (
  src: ObjectId,
  dest: ObjectId,
): Effect.Effect<void, BlobstoreHostError> =>
  Effect.try({
    try: () => Blob.copyObject(src, dest),
    catch: (cause) => new BlobstoreHostError(cause, "copyObject"),
  })

/** Move (rename) an object to the same or a different container. Overwrites the destination. */
export const moveObject = (
  src: ObjectId,
  dest: ObjectId,
): Effect.Effect<void, BlobstoreHostError> =>
  Effect.try({
    try: () => Blob.moveObject(src, dest),
    catch: (cause) => new BlobstoreHostError(cause, "moveObject"),
  })
