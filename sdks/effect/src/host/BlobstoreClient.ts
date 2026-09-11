/**
 * Host service for `wasi:blobstore/blobstore` +
 * `wasi:blobstore/container` + `wasi:blobstore/types` +
 * `wasi:io/streams@0.2.3`. Wraps the host's blobstore surface as
 * Effect-typed methods so SDK code can acquire / mutate object
 * containers via DI rather than reaching directly into the WIT
 * specifier imports.
 *
 * The 4096-byte chunking required by `wasi:io/streams.blocking-write
 * -and-flush` lives inside the Live `writeData` impl; consumers see
 * a single `(name, bytes)` Effect.
 *
 * Resource handles are *not* exposed; the service surface is purely
 * by-value (records of `Effect`-returning methods). The public
 * {@link import("../Blobstore.js").Container} type wraps a
 * {@link HostContainer} returned from `createContainer` /
 * `getContainer` and adds the schema-typed view + JS `Date`
 * decoding + whole-object read recovery.
 *
 * @internal — not re-exported from `src/index.ts`.
 */
import { Context, Effect, Layer, Option, Stream } from "effect"
import type * as Scope from "effect/Scope"
import { BlobstoreHostError } from "../Blobstore.js"
import * as Blob from "wasi:blobstore/blobstore"
import * as ContainerNS from "wasi:blobstore/container"
import * as Types from "wasi:blobstore/types"

/** `(containerName, objectName)` pair — mirrors the WIT `object-id`. */
export interface HostObjectId {
  readonly container: string
  readonly object: string
}

/**
 * Inclusive byte range for {@link HostContainer.getData}. Passed
 * through to the host verbatim — the SDK does NOT massage backend
 * divergence (see public `ByteRange` JSDoc in `src/blobstore.ts`).
 */
export interface HostByteRange {
  readonly start: bigint
  readonly end: bigint
}

/**
 * Container metadata as returned by the host — `createdAt` is the
 * raw `u64` Unix-millis bigint. The public `ContainerMetadata`
 * (in `src/blobstore.ts`) wraps this with a JS `Date`.
 */
export interface HostContainerMetadata {
  readonly name: string
  readonly createdAtMillis: bigint
}

/**
 * Object metadata as returned by the host — same caveat as
 * {@link HostContainerMetadata}.
 */
export interface HostObjectMetadata {
  readonly name: string
  readonly container: string
  readonly createdAtMillis: bigint
  readonly size: bigint
}

/**
 * Per-container surface. Mirrors the `wasi:blobstore/container`
 * resource methods, with the `wasi:io/streams` chunking folded into
 * `writeData` and the `stream-object-names` resource folded into
 * the {@link listObjects} stream.
 */
export interface HostContainer {
  /** The container's name (cached at acquire time). */
  readonly name: string
  readonly info: Effect.Effect<HostContainerMetadata, BlobstoreHostError>
  readonly clear: Effect.Effect<void, BlobstoreHostError>
  /**
   * Fetch a byte range. Inclusive end per the WIT spec; backend
   * divergence is the consumer's problem (see
   * `src/blobstore.ts` `ByteRange` JSDoc).
   */
  getData(name: string, range: HostByteRange): Effect.Effect<Uint8Array, BlobstoreHostError>
  /** Create or replace `name` with `data`. Chunked at 4096 bytes per write. */
  writeData(name: string, data: Uint8Array): Effect.Effect<void, BlobstoreHostError>
  hasObject(name: string): Effect.Effect<boolean, BlobstoreHostError>
  objectInfo(name: string): Effect.Effect<HostObjectMetadata, BlobstoreHostError>
  deleteObject(name: string): Effect.Effect<void, BlobstoreHostError>
  deleteObjects(names: ReadonlyArray<string>): Effect.Effect<void, BlobstoreHostError>
  /**
   * Stream of object names. Page size 256, eagerly fetched at
   * stream open (the host pins the snapshot to the oplog).
   */
  readonly listObjects: Stream.Stream<string, BlobstoreHostError>
}

export interface BlobstoreClientShape {
  /** Create a new empty container. Fails if a container with the same name already exists. */
  createContainer(name: string): Effect.Effect<HostContainer, BlobstoreHostError, Scope.Scope>
  /** Open an existing container by name. Fails if the container does not exist. */
  getContainer(name: string): Effect.Effect<HostContainer, BlobstoreHostError, Scope.Scope>
  /**
   * Idempotent: open the named container, creating it first if it
   * does not exist. `wasi:blobstore` has no atomic get-or-create
   * primitive, so the default Live impl optimistically calls
   * `createContainer` and, on failure, replays `getContainer` if
   * `containerExists` reports the container is now present
   * (collapses the TOCTOU window vs. a raw exists-then-create).
   * Test fakes can override with a single-call semantic.
   */
  getOrCreateContainer(name: string): Effect.Effect<HostContainer, BlobstoreHostError, Scope.Scope>
  containerExists(name: string): Effect.Effect<boolean, BlobstoreHostError>
  deleteContainer(name: string): Effect.Effect<void, BlobstoreHostError>
  copyObject(src: HostObjectId, dest: HostObjectId): Effect.Effect<void, BlobstoreHostError>
  moveObject(src: HostObjectId, dest: HostObjectId): Effect.Effect<void, BlobstoreHostError>
}

export class BlobstoreClient extends Context.Service<BlobstoreClient, BlobstoreClientShape>()(
  "effect-golem/host/Blobstore",
) {}

// ---------------------------------------------------------------------------
// Live impl — calls into wasi:blobstore/* + wasi:io/streams verbatim.
// ---------------------------------------------------------------------------

const consumeIncomingSync = (
  iv: Types.IncomingValue,
): Effect.Effect<Uint8Array, BlobstoreHostError> =>
  Effect.try({
    try: () => iv.incomingValueConsumeSync(),
    catch: (cause) => new BlobstoreHostError(cause, "incomingValueConsumeSync"),
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
    yield* Effect.try({
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
    return ov
  })

const decodeContainerMetadata = (
  m: ContainerNS.ContainerMetadata | Types.ContainerMetadata,
): HostContainerMetadata => ({
  name: m.name,
  createdAtMillis: m.createdAt,
})

const decodeObjectMetadata = (m: Types.ObjectMetadata): HostObjectMetadata => ({
  name: m.name,
  container: m.container,
  createdAtMillis: m.createdAt,
  size: m.size,
})

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

const makeHostContainer = (name: string, handle: ContainerNS.Container): HostContainer => {
  const info: HostContainer["info"] = Effect.gen(function* () {
    const m = yield* Effect.try({
      try: () => handle.info(),
      catch: (cause) => new BlobstoreHostError(cause, "container.info"),
    })
    return decodeContainerMetadata(m)
  })

  const clear: HostContainer["clear"] = Effect.try({
    try: () => handle.clear(),
    catch: (cause) => new BlobstoreHostError(cause, "container.clear"),
  })

  const getData: HostContainer["getData"] = (objectName, range) =>
    Effect.gen(function* () {
      const iv = yield* Effect.try({
        try: () => handle.getData(objectName, range.start, range.end),
        catch: (cause) => new BlobstoreHostError(cause, "container.getData"),
      })
      return yield* consumeIncomingSync(iv)
    })

  const writeData: HostContainer["writeData"] = (objectName, data) =>
    Effect.gen(function* () {
      const ov = yield* buildOutgoingValue(data)
      yield* Effect.try({
        try: () => handle.writeData(objectName, ov),
        catch: (cause) => new BlobstoreHostError(cause, "container.writeData"),
      })
    })

  const hasObject: HostContainer["hasObject"] = (objectName) =>
    Effect.try({
      try: () => handle.hasObject(objectName),
      catch: (cause) => new BlobstoreHostError(cause, "container.hasObject"),
    })

  const objectInfo: HostContainer["objectInfo"] = (objectName) =>
    Effect.gen(function* () {
      const m = yield* Effect.try({
        try: () => handle.objectInfo(objectName),
        catch: (cause) => new BlobstoreHostError(cause, "container.objectInfo"),
      })
      return decodeObjectMetadata(m)
    })

  const deleteObject: HostContainer["deleteObject"] = (objectName) =>
    Effect.try({
      try: () => handle.deleteObject(objectName),
      catch: (cause) => new BlobstoreHostError(cause, "container.deleteObject"),
    })

  const deleteObjects: HostContainer["deleteObjects"] = (names) =>
    Effect.try({
      try: () => handle.deleteObjects([...names]),
      catch: (cause) => new BlobstoreHostError(cause, "container.deleteObjects"),
    })

  return {
    name,
    info,
    clear,
    getData,
    writeData,
    hasObject,
    objectInfo,
    deleteObject,
    deleteObjects,
    get listObjects() {
      return listObjectsStream(handle)
    },
  }
}

const acquireHostContainer = (
  name: string,
  handle: ContainerNS.Container,
): Effect.Effect<HostContainer, never, Scope.Scope> =>
  Effect.acquireRelease(Effect.succeed(makeHostContainer(name, handle)), () => Effect.void)

const liveCreateContainer = (
  name: string,
): Effect.Effect<HostContainer, BlobstoreHostError, Scope.Scope> =>
  Effect.gen(function* () {
    const handle = yield* Effect.try({
      try: () => Blob.createContainer(name),
      catch: (cause) => new BlobstoreHostError(cause, "createContainer"),
    })
    return yield* acquireHostContainer(name, handle)
  })

const liveGetContainer = (
  name: string,
): Effect.Effect<HostContainer, BlobstoreHostError, Scope.Scope> =>
  Effect.gen(function* () {
    const handle = yield* Effect.try({
      try: () => Blob.getContainer(name),
      catch: (cause) => new BlobstoreHostError(cause, "getContainer"),
    })
    return yield* acquireHostContainer(name, handle)
  })

const liveContainerExists = (name: string): Effect.Effect<boolean, BlobstoreHostError> =>
  Effect.try({
    try: () => Blob.containerExists(name),
    catch: (cause) => new BlobstoreHostError(cause, "containerExists"),
  })

const liveDeleteContainer = (name: string): Effect.Effect<void, BlobstoreHostError> =>
  Effect.try({
    try: () => Blob.deleteContainer(name),
    catch: (cause) => new BlobstoreHostError(cause, "deleteContainer"),
  })

const liveCopyObject = (
  src: HostObjectId,
  dest: HostObjectId,
): Effect.Effect<void, BlobstoreHostError> =>
  Effect.try({
    try: () => Blob.copyObject(src, dest),
    catch: (cause) => new BlobstoreHostError(cause, "copyObject"),
  })

const liveMoveObject = (
  src: HostObjectId,
  dest: HostObjectId,
): Effect.Effect<void, BlobstoreHostError> =>
  Effect.try({
    try: () => Blob.moveObject(src, dest),
    catch: (cause) => new BlobstoreHostError(cause, "moveObject"),
  })

export const BlobstoreLive: Layer.Layer<BlobstoreClient> = Layer.succeed(
  BlobstoreClient,
  BlobstoreClient.of({
    createContainer: liveCreateContainer,
    getContainer: liveGetContainer,
    getOrCreateContainer: (name) =>
      // `wasi:blobstore` exposes no atomic get-or-create primitive.
      // We minimise the TOCTOU window by attempting `createContainer`
      // first; if it fails AND the container now exists, treat the
      // failure as a benign race (another fiber / external actor won
      // the create) and replay `getContainer`. Otherwise propagate
      // the original create failure.
      Effect.matchEffect(liveCreateContainer(name), {
        onSuccess: Effect.succeed,
        onFailure: (createErr) =>
          Effect.gen(function* () {
            const exists = yield* Effect.matchEffect(liveContainerExists(name), {
              onSuccess: Effect.succeed,
              onFailure: () => Effect.succeed(false),
            })
            if (exists) return yield* liveGetContainer(name)
            return yield* Effect.fail(createErr)
          }),
      }),
    containerExists: liveContainerExists,
    deleteContainer: liveDeleteContainer,
    copyObject: liveCopyObject,
    moveObject: liveMoveObject,
  }),
)
