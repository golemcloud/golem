/**
 * Layer-based test fake for {@link BlobstoreClient}. Backed by an
 * in-memory `Map<containerName, ContainerEntry>` shared across the
 * fake's per-container handles, plus a `Ref<Option<...>>` for
 * one-shot error injection.
 *
 * The fake mirrors the bug in the Golem in-memory + filesystem
 * backends: ranged reads treat `end` as Rust-exclusive (i.e.
 * `bytes.subarray(start, end)`). The SDK's whole-object recovery
 * logic (in `src/blobstore.ts`) is what makes that observable.
 *
 * Use with `Effect.provide(eff, fake.layer)` once per test (NOT via
 * `it.layer(fake.layer)`, which would share state across the entire
 * `describe` block and cause cross-test interference).
 */

import { Effect, Layer, Option, Ref, Stream } from "effect"
import {
  BlobstoreClient,
  type HostByteRange,
  type HostContainer,
  type HostContainerMetadata,
  type HostObjectId,
  type HostObjectMetadata,
} from "../../src/host/BlobstoreClient.js"
import { BlobstoreHostError } from "../../src/Blobstore.js"

interface ObjectEntry {
  bytes: Uint8Array
  createdAtMillis: bigint
}

interface ContainerEntry {
  name: string
  createdAtMillis: bigint
  objects: Map<string, ObjectEntry>
}

export interface BlobFake {
  readonly layer: Layer.Layer<BlobstoreClient>
  /**
   * Queue a one-shot host failure for the *next* call into any
   * service method (top-level OR per-container). Consumed exactly
   * once on first match; subsequent calls fall through to the
   * in-memory backend.
   */
  readonly setNextError: (err: BlobstoreHostError) => Effect.Effect<void>
  /** Snapshot of currently-known container names (for assertions). */
  readonly containers: Effect.Effect<ReadonlyArray<string>>
  /**
   * Snapshot of object names inside a container (or `undefined` if
   * the container does not exist).
   */
  readonly objectsIn: (container: string) => Effect.Effect<ReadonlyArray<string> | undefined>
}

const consumeError = (
  ref: Ref.Ref<Option.Option<BlobstoreHostError>>,
): Effect.Effect<Option.Option<BlobstoreHostError>> =>
  Ref.modify(ref, (current) => [current, Option.none<BlobstoreHostError>()])

const guard = (
  ref: Ref.Ref<Option.Option<BlobstoreHostError>>,
  body: Effect.Effect<void, BlobstoreHostError>,
): Effect.Effect<void, BlobstoreHostError> =>
  Effect.gen(function* () {
    const queued = yield* consumeError(ref)
    if (Option.isSome(queued)) {
      return yield* Effect.fail(queued.value)
    }
    return yield* body
  })

const guardWith = <A>(
  ref: Ref.Ref<Option.Option<BlobstoreHostError>>,
  body: Effect.Effect<A, BlobstoreHostError>,
): Effect.Effect<A, BlobstoreHostError> =>
  Effect.gen(function* () {
    const queued = yield* consumeError(ref)
    if (Option.isSome(queued)) {
      return yield* Effect.fail(queued.value)
    }
    return yield* body
  })

const makeFakeContainer = (
  entry: ContainerEntry,
  nextError: Ref.Ref<Option.Option<BlobstoreHostError>>,
): HostContainer => {
  const info: HostContainer["info"] = guardWith(
    nextError,
    Effect.sync(
      (): HostContainerMetadata => ({
        name: entry.name,
        createdAtMillis: entry.createdAtMillis,
      }),
    ),
  )

  const clear: HostContainer["clear"] = guard(
    nextError,
    Effect.sync(() => {
      entry.objects.clear()
    }),
  )

  const getData: HostContainer["getData"] = (objectName: string, range: HostByteRange) =>
    guardWith(
      nextError,
      Effect.gen(function* () {
        const obj = entry.objects.get(objectName)
        if (obj === undefined) {
          return yield* Effect.fail(
            new BlobstoreHostError(
              new Error(`object ${objectName} not found`),
              "container.getData",
            ),
          )
        }
        const start = Number(range.start)
        // Mock follows the in-memory backend: `end` is exclusive.
        const end = Math.min(Number(range.end), obj.bytes.length)
        return new Uint8Array(obj.bytes.subarray(start, end))
      }),
    )

  const writeData: HostContainer["writeData"] = (objectName, data) =>
    guard(
      nextError,
      Effect.sync(() => {
        entry.objects.set(objectName, {
          bytes: new Uint8Array(data),
          createdAtMillis: BigInt(Date.now()),
        })
      }),
    )

  const hasObject: HostContainer["hasObject"] = (objectName) =>
    guardWith(
      nextError,
      Effect.sync(() => entry.objects.has(objectName)),
    )

  const objectInfo: HostContainer["objectInfo"] = (objectName) =>
    guardWith(
      nextError,
      Effect.gen(function* () {
        const obj = entry.objects.get(objectName)
        if (obj === undefined) {
          return yield* Effect.fail(
            new BlobstoreHostError(
              new Error(`object ${objectName} not found`),
              "container.objectInfo",
            ),
          )
        }
        const info: HostObjectMetadata = {
          name: objectName,
          container: entry.name,
          createdAtMillis: obj.createdAtMillis,
          size: BigInt(obj.bytes.length),
        }
        return info
      }),
    )

  const deleteObject: HostContainer["deleteObject"] = (objectName) =>
    guard(
      nextError,
      Effect.sync(() => {
        entry.objects.delete(objectName)
      }),
    )

  const deleteObjects: HostContainer["deleteObjects"] = (names) =>
    guard(
      nextError,
      Effect.sync(() => {
        for (const n of names) entry.objects.delete(n)
      }),
    )

  const listObjects: Stream.Stream<string, BlobstoreHostError> = Stream.unwrap(
    guardWith(
      nextError,
      Effect.sync(() => {
        // Snapshot at stream-open time, mirroring the host's eager
        // `list-objects` semantics (the host pins the snapshot to
        // the oplog). Order: pop from tail like the real mock.
        const snapshot = Array.from(entry.objects.keys())
        return Stream.fromIterable(snapshot.reverse())
      }),
    ),
  )

  return {
    name: entry.name,
    info,
    clear,
    getData,
    writeData,
    hasObject,
    objectInfo,
    deleteObject,
    deleteObjects,
    listObjects,
  }
}

/**
 * Build a fresh fake. Use one per test — the in-memory state is
 * owned by this instance.
 */
export const make: Effect.Effect<BlobFake> = Effect.gen(function* () {
  const containers = new Map<string, ContainerEntry>()
  const nextError = yield* Ref.make<Option.Option<BlobstoreHostError>>(Option.none())

  const ensure = (name: string): ContainerEntry => {
    let entry = containers.get(name)
    if (!entry) {
      entry = { name, createdAtMillis: BigInt(Date.now()), objects: new Map() }
      containers.set(name, entry)
    }
    return entry
  }

  const acquire = (entry: ContainerEntry) =>
    Effect.acquireRelease(Effect.succeed(makeFakeContainer(entry, nextError)), () => Effect.void)

  const layer = Layer.succeed(
    BlobstoreClient,
    BlobstoreClient.of({
      createContainer: (name) =>
        Effect.gen(function* () {
          const queued = yield* consumeError(nextError)
          if (Option.isSome(queued)) {
            return yield* Effect.fail(queued.value)
          }
          if (containers.has(name)) {
            return yield* Effect.fail(
              new BlobstoreHostError(
                new Error(`container ${name} already exists`),
                "createContainer",
              ),
            )
          }
          return yield* acquire(ensure(name))
        }),
      getContainer: (name) =>
        Effect.gen(function* () {
          const queued = yield* consumeError(nextError)
          if (Option.isSome(queued)) {
            return yield* Effect.fail(queued.value)
          }
          const entry = containers.get(name)
          if (!entry) {
            return yield* Effect.fail(
              new BlobstoreHostError(new Error(`container ${name} does not exist`), "getContainer"),
            )
          }
          return yield* acquire(entry)
        }),
      getOrCreateContainer: (name) =>
        Effect.gen(function* () {
          const queued = yield* consumeError(nextError)
          if (Option.isSome(queued)) {
            return yield* Effect.fail(queued.value)
          }
          return yield* acquire(ensure(name))
        }),
      containerExists: (name) =>
        guardWith(
          nextError,
          Effect.sync(() => containers.has(name)),
        ),
      deleteContainer: (name) =>
        guard(
          nextError,
          Effect.sync(() => {
            containers.delete(name)
          }),
        ),
      copyObject: (src: HostObjectId, dest: HostObjectId) =>
        guard(
          nextError,
          Effect.gen(function* () {
            const srcEntry = containers.get(src.container)
            if (!srcEntry) {
              return yield* Effect.fail(
                new BlobstoreHostError(
                  new Error(`source container ${src.container} not found`),
                  "copyObject",
                ),
              )
            }
            const obj = srcEntry.objects.get(src.object)
            if (!obj) {
              return yield* Effect.fail(
                new BlobstoreHostError(
                  new Error(`source object ${src.object} not found`),
                  "copyObject",
                ),
              )
            }
            const destEntry = containers.get(dest.container)
            if (!destEntry) {
              return yield* Effect.fail(
                new BlobstoreHostError(
                  new Error(`destination container ${dest.container} not found`),
                  "copyObject",
                ),
              )
            }
            destEntry.objects.set(dest.object, {
              bytes: new Uint8Array(obj.bytes),
              createdAtMillis: BigInt(Date.now()),
            })
          }),
        ),
      moveObject: (src: HostObjectId, dest: HostObjectId) =>
        guard(
          nextError,
          Effect.gen(function* () {
            const srcEntry = containers.get(src.container)
            if (!srcEntry) {
              return yield* Effect.fail(
                new BlobstoreHostError(
                  new Error(`source container ${src.container} not found`),
                  "moveObject",
                ),
              )
            }
            const obj = srcEntry.objects.get(src.object)
            if (!obj) {
              return yield* Effect.fail(
                new BlobstoreHostError(
                  new Error(`source object ${src.object} not found`),
                  "moveObject",
                ),
              )
            }
            const destEntry = containers.get(dest.container)
            if (!destEntry) {
              return yield* Effect.fail(
                new BlobstoreHostError(
                  new Error(`destination container ${dest.container} not found`),
                  "moveObject",
                ),
              )
            }
            destEntry.objects.set(dest.object, {
              bytes: new Uint8Array(obj.bytes),
              createdAtMillis: BigInt(Date.now()),
            })
            srcEntry.objects.delete(src.object)
          }),
        ),
    }),
  )

  return {
    layer,
    setNextError: (err) => Ref.set(nextError, Option.some(err)),
    containers: Effect.sync(() => Array.from(containers.keys())),
    objectsIn: (name) =>
      Effect.sync(() => {
        const entry = containers.get(name)
        return entry ? Array.from(entry.objects.keys()) : undefined
      }),
  }
})
