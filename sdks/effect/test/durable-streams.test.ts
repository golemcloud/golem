import { describe, expect, it } from "@effect/vitest"
import { Cause, Deferred, Effect, Exit, Fiber, Layer, Schema, Scope, Stream } from "effect"
import { TestClock } from "effect/testing"
import { vi } from "vitest"
import type * as Host from "golem:agent/durable-streams@2.0.0"
import type { Secret } from "golem:core/types@2.0.0"
import * as DS from "../src/DurableStreams.js"
import {
  DurableStreamsClient,
  DurableStreamsLive,
  type DurableStreamsClientShape,
  type Reader,
  type Writer,
} from "../src/host/DurableStreamsClient.js"
import { agentStreamFromHandle, agentStreamToHandle } from "../src/internal/agentStream.js"
import { toWitCodec } from "../src/WitCodec.js"
import { Uint8, Uint64 } from "../src/WitTypes.js"

const wit = vi.hoisted(() => ({ DurableStreamWriter: vi.fn(), DurableStreamReader: vi.fn() }))
vi.mock("golem:agent/durable-streams@2.0.0", async (original) => ({
  ...(await original<object>()),
  ...wit,
}))

const options = { url: "https://streams.example/events", producerId: "producer", maxRetries: 0 }
const bytes = (s: string) => new TextEncoder().encode(s)
const batch = (
  payload: Uint8Array,
  overrides: Partial<Host.DurableStreamBatch> = {},
): Host.DurableStreamBatch => ({
  payload,
  next: { offset: "opaque/end", cursor: undefined },
  contentType: "application/json",
  upToDate: true,
  closed: true,
  ...overrides,
})
const receipt = (
  request: Host.DurableStreamAppendRequest,
  epoch = 0n,
): Host.DurableStreamAppendReceipt => ({
  nextOffset: undefined,
  epoch,
  sequence: request.sequence,
  closed: request.close,
})
const layer = (
  host: Partial<Reader & Writer>,
  constructors: Partial<DurableStreamsClientShape> = {},
) =>
  Layer.succeed(DurableStreamsClient, {
    makeReader: () => Effect.succeed({ read: host.read ?? (() => Effect.die("unexpected read")) }),
    makeWriter: () =>
      Effect.succeed({ append: host.append ?? (() => Effect.die("unexpected append")) }),
    ...constructors,
  })
const failure = (
  kind: Host.DurableStreamErrorKind,
  retryAfterMs?: bigint,
): Host.DurableStreamError => ({
  kind,
  message: "sanitized failure",
  retryAfterMs,
  producerEpoch: undefined,
  expectedSequence: undefined,
})

describe("Durable Streams", () => {
  it.effect("uses one live reader per run, compact requests, and drops on EOF or take", () =>
    Effect.gen(function* () {
      const instances: {
        requests: Host.DurableStreamReadRequest[]
        drop: ReturnType<typeof vi.fn>
      }[] = []
      const auth = Object.freeze({}) as Secret
      wit.DurableStreamReader.mockImplementation(function (
        descriptor: Host.DurableStreamReaderOptions,
        secret: Secret,
      ) {
        expect(descriptor).toEqual({ url: options.url, mode: "bytes", timeoutMs: 1234n })
        expect(secret).toBe(auth)
        const instance = { requests: [] as Host.DurableStreamReadRequest[], drop: vi.fn() }
        instances.push(instance)
        return {
          read: (request: Host.DurableStreamReadRequest) => {
            expect(instance.drop).not.toHaveBeenCalled()
            instance.requests.push(structuredClone(request))
            return Promise.resolve(
              batch(new Uint8Array([instance.requests.length === 1 ? 17 : 93]), {
                contentType: "application/octet-stream",
                next: { offset: "opaque:8", cursor: "cursor:2" },
                closed: instance.requests.length === 2,
              }),
            )
          },
          [Symbol.dispose]: instance.drop,
        }
      })
      const source = DS.readBytes({ ...options, auth, timeoutMs: 1234 })
      expect(instances).toHaveLength(0)
      expect(yield* Stream.runCollect(source).pipe(Effect.provide(DurableStreamsLive))).toEqual([
        17, 93,
      ])
      expect(
        yield* source.pipe(Stream.take(1), Stream.runCollect, Effect.provide(DurableStreamsLive)),
      ).toEqual([17])
      expect(instances).toHaveLength(2)
      expect(instances[0]!.requests).toEqual([
        {
          checkpoint: { offset: "-1", cursor: undefined },
          transport: "catch-up",
          contentType: undefined,
        },
        {
          checkpoint: { offset: "opaque:8", cursor: "cursor:2" },
          transport: "long-poll",
          contentType: "application/octet-stream",
        },
      ])
      expect(instances[1]!.requests).toHaveLength(1)
      for (const instance of instances) expect(instance.drop).toHaveBeenCalledTimes(1)
      wit.DurableStreamReader.mockReset()
    }),
  )

  it.effect("drops a live reader after a typed protocol failure", () =>
    Effect.gen(function* () {
      const drop = vi.fn()
      wit.DurableStreamReader.mockImplementation(function () {
        return { read: () => Promise.reject(failure("permission-denied")), [Symbol.dispose]: drop }
      })
      const error = yield* DS.readBytes(options).pipe(
        Stream.runDrain,
        Effect.flip,
        Effect.provide(DurableStreamsLive),
      )
      expect(error.kind).toBe("permission-denied")
      expect(wit.DurableStreamReader).toHaveBeenCalledTimes(1)
      expect(drop).toHaveBeenCalledTimes(1)
      wit.DurableStreamReader.mockReset()
    }),
  )

  it.each(["resolve", "reject"])(
    "defers reader drop after interruption until the WIT Promise %ss",
    async (settlement) => {
      let settle!: () => void
      let started!: () => void
      const pulling = new Promise<void>((resolve) => {
        started = resolve
      })
      const drop = vi.fn()
      wit.DurableStreamReader.mockImplementation(function () {
        return {
          read: () =>
            new Promise<Host.DurableStreamBatch>((resolve, reject) => {
              settle = () =>
                settlement === "resolve"
                  ? resolve(batch(new Uint8Array([17])))
                  : reject(failure("transport"))
              started()
            }),
          [Symbol.dispose]: drop,
        }
      })
      const fiber = Effect.runFork(
        DS.readBytes(options).pipe(Stream.runDrain, Effect.provide(DurableStreamsLive)),
      )
      await pulling
      await Effect.runPromise(Fiber.interrupt(fiber))
      expect(drop).not.toHaveBeenCalled()
      settle()
      await new Promise<void>((resolve) => setTimeout(resolve, 0))
      expect(drop).toHaveBeenCalledTimes(1)
      wit.DurableStreamReader.mockReset()
    },
  )

  it.effect(
    "captures distinct writer descriptors once and reuses each handle through retry and close",
    () =>
      Effect.gen(function* () {
        const instances: {
          descriptor: Host.DurableStreamWriterOptions
          requests: Host.DurableStreamAppendRequest[]
          drop: ReturnType<typeof vi.fn>
        }[] = []
        const auth = Object.freeze({}) as Secret
        wit.DurableStreamWriter.mockImplementation(function (
          descriptor: Host.DurableStreamWriterOptions,
          secret: Secret,
        ) {
          expect(secret).toBe(auth)
          const instance = {
            descriptor,
            requests: [] as Host.DurableStreamAppendRequest[],
            drop: vi.fn(),
          }
          instances.push(instance)
          return {
            append: (request: Host.DurableStreamAppendRequest) => {
              expect(instance.drop).not.toHaveBeenCalled()
              instance.requests.push(structuredClone(request))
              return instance.requests.length === 1
                ? Promise.reject(failure("transport"))
                : Promise.resolve(receipt(request, descriptor.producerEpoch))
            },
            [Symbol.dispose]: instance.drop,
          }
        })
        yield* Effect.gen(function* () {
          const mutable = {
            ...options,
            auth,
            epoch: 7n,
            timeoutMs: 912,
            contentType: "application/custom",
          }
          const first = yield* DS.makeByteWriter(mutable)
          mutable.url = "https://other.example/events"
          mutable.producerId = "other"
          mutable.epoch = 19n
          mutable.timeoutMs = 3456
          const second = yield* DS.makeByteWriter(mutable)
          mutable.contentType = "changed/after-construction"
          expect(instances).toHaveLength(2)
          expect(instances.every((instance) => instance.requests.length === 0)).toBe(true)
          expect(instances.map((instance) => instance.descriptor)).toEqual([
            {
              url: options.url,
              contentType: "application/custom",
              producerId: "producer",
              producerEpoch: 7n,
              timeoutMs: 912n,
            },
            {
              url: "https://other.example/events",
              contentType: "application/custom",
              producerId: "other",
              producerEpoch: 19n,
              timeoutMs: 3456n,
            },
          ])
          const payload = new Uint8Array([19, 251])
          expect((yield* first.append(payload).pipe(Effect.flip)).kind).toBe("transport")
          payload.fill(0)
          yield* first.retryPending
          expect((yield* first.close).nextOffset).toBeUndefined()
          expect((yield* second.close.pipe(Effect.flip)).kind).toBe("transport")
          yield* second.retryPending
          expect(instances[0]!.requests).toEqual([
            {
              payload: { tag: "bytes", val: new Uint8Array([19, 251]) },
              sequence: 0n,
              close: false,
            },
            {
              payload: { tag: "bytes", val: new Uint8Array([19, 251]) },
              sequence: 0n,
              close: false,
            },
            { payload: { tag: "bytes", val: new Uint8Array() }, sequence: 1n, close: true },
          ])
          expect(instances[1]!.requests).toEqual([
            { payload: { tag: "bytes", val: new Uint8Array() }, sequence: 0n, close: true },
            { payload: { tag: "bytes", val: new Uint8Array() }, sequence: 0n, close: true },
          ])
        }).pipe(Effect.scoped, Effect.provide(DurableStreamsLive))
        expect(wit.DurableStreamWriter).toHaveBeenCalledTimes(2)
        for (const instance of instances) expect(instance.drop).toHaveBeenCalledTimes(1)
        wit.DurableStreamWriter.mockReset()
      }),
  )

  it("ignores a late WIT Promise acknowledgement after native fiber interruption", async () => {
    let release!: (ack: Host.DurableStreamAppendReceipt) => void
    let started!: () => void
    const pulling = new Promise<void>((resolve) => {
      started = resolve
    })
    const requests: Host.DurableStreamAppendRequest[] = []
    const append = (request: Host.DurableStreamAppendRequest) => {
      requests.push(structuredClone(request))
      if (requests.length > 1) return Promise.resolve(receipt(request))
      started()
      return new Promise<Host.DurableStreamAppendReceipt>((resolve) => {
        release = resolve
      })
    }
    const drop = vi.fn()
    wit.DurableStreamWriter.mockImplementation(function () {
      return { append, [Symbol.dispose]: drop }
    })
    const scope = await Effect.runPromise(Scope.make())
    const writer = await Effect.runPromise(
      DS.makeByteWriter(options).pipe(Effect.provide(DurableStreamsLive), Scope.provide(scope)),
    )
    const fiber = Effect.runFork(writer.close.pipe(Effect.provide(DurableStreamsLive)))
    await pulling
    await Effect.runPromise(Fiber.interrupt(fiber))
    expect(await Effect.runPromise(writer.hasPending)).toBe(true)
    expect(await Effect.runPromise(writer.nextSequence)).toBe(0n)
    await Effect.runPromise(writer.retryPending.pipe(Effect.provide(DurableStreamsLive)))
    expect(requests).toHaveLength(2)
    expect(requests[1]).toEqual(requests[0])
    expect(await Effect.runPromise(writer.nextSequence)).toBe(1n)
    expect(wit.DurableStreamWriter).toHaveBeenCalledTimes(1)
    expect(drop).not.toHaveBeenCalled()
    await Effect.runPromise(Scope.close(scope, Exit.void))
    expect(drop).not.toHaveBeenCalled()
    release(receipt(requests[0]!))
    await new Promise<void>((resolve) => setTimeout(resolve, 0))
    expect(drop).toHaveBeenCalledTimes(1)
    wit.DurableStreamWriter.mockReset()
  })

  it("allocates a default producer once across forked effects and explicit retry", async () => {
    const uuid = vi
      .spyOn(crypto, "randomUUID")
      .mockReturnValue("11111111-2222-4333-8444-555555555555")
    const scope = await Effect.runPromise(Scope.make())
    try {
      const requests: Host.DurableStreamAppendRequest[] = []
      const host = layer({
        append: (request) =>
          Effect.suspend(() => {
            requests.push(structuredClone(request))
            return requests.length === 1
              ? Effect.fail(failure("transport"))
              : Effect.succeed(receipt(request))
          }),
      })
      const writer = await Effect.runPromise(
        DS.makeByteWriter({ url: options.url, maxRetries: 0 }).pipe(
          Effect.provide(host),
          Scope.provide(scope),
        ),
      )
      const first = Effect.runFork(writer.close.pipe(Effect.provide(host)))
      expect(Exit.isFailure(await Effect.runPromise(Fiber.await(first)))).toBe(true)
      const retry = Effect.runFork(writer.retryPending.pipe(Effect.provide(host)))
      await Effect.runPromise(Fiber.join(retry))
      expect(uuid).toHaveBeenCalledTimes(1)
      expect(requests[1]).toEqual(requests[0])
      expect(writer.producerId).toBe("11111111-2222-4333-8444-555555555555")
    } finally {
      await Effect.runPromise(Scope.close(scope, Exit.void))
      uuid.mockRestore()
    }
  })

  it.effect(
    "resolves now once, preserves opaque cursors and MIME, and emits final payload before EOF",
    () =>
      Effect.gen(function* () {
        const requests: Host.DurableStreamReadRequest[] = []
        const responses = [
          batch(bytes("[11,29]"), {
            closed: false,
            upToDate: false,
            next: { offset: "a/9", cursor: "cursor-a" },
          }),
          batch(bytes("[]"), { closed: false, next: { offset: "b:2", cursor: "cursor-b" } }),
          batch(bytes("[47]")),
        ]
        const stream = DS.readJson(Schema.Number, { ...options, offset: "now", live: "sse" })
        expect(requests).toHaveLength(0)
        const fiber = yield* Stream.runCollect(stream).pipe(
          Effect.provide(
            layer({
              read: (request) =>
                Effect.sync(() => {
                  requests.push(structuredClone(request))
                  return responses.shift()!
                }),
            }),
          ),
          Effect.forkChild,
        )
        yield* TestClock.adjust(100)
        expect(yield* Fiber.join(fiber)).toEqual([11, 29, 47])
        expect(requests.map((r) => [r.checkpoint, r.transport, r.contentType])).toEqual([
          [{ offset: "now", cursor: undefined }, "catch-up", undefined],
          [{ offset: "a/9", cursor: "cursor-a" }, "catch-up", "application/json"],
          [{ offset: "b:2", cursor: "cursor-b" }, "sse", "application/json"],
        ])
      }),
  )

  it.effect("is reusable and stops pulling after downstream take", () =>
    Effect.gen(function* () {
      let reads = 0
      const source = DS.readBytes(options).pipe(Stream.take(1))
      const run = Stream.runCollect(source).pipe(
        Effect.provide(
          layer({
            read: () =>
              Effect.sync(() => {
                reads++
                return batch(new Uint8Array([7, 251]), { closed: false })
              }),
          }),
        ),
      )
      expect(yield* run).toEqual([7])
      expect(yield* run).toEqual([7])
      expect(reads).toBe(2)
    }),
  )

  it.effect("forwards bytes through native agent streams with captured host services", () =>
    Effect.gen(function* () {
      const codec = (yield* toWitCodec(Uint8)).codec
      const handle = yield* Effect.map(Effect.context<DurableStreamsClient>(), (context) =>
        agentStreamToHandle(DS.readBytes(options), codec, context),
      ).pipe(
        Effect.provide(layer({ read: () => Effect.succeed(batch(new Uint8Array([3, 249, 17]))) })),
      )
      expect(yield* Stream.runCollect(agentStreamFromHandle(handle, codec))).toEqual([3, 249, 17])
    }),
  )

  it.effect("interrupts a pending read and joins the host effect finalizer", () =>
    Effect.gen(function* () {
      const started = yield* Deferred.make<void>()
      let finalized = false
      let dropped = false
      const fiber = yield* Stream.runDrain(DS.readBytes(options)).pipe(
        Effect.provide(
          layer(
            {},
            {
              makeReader: () =>
                Effect.acquireRelease(
                  Effect.succeed({
                    read: () =>
                      Deferred.succeed(started, undefined).pipe(
                        Effect.andThen(Effect.never),
                        Effect.ensuring(
                          Effect.sync(() => {
                            finalized = true
                          }),
                        ),
                      ),
                  }),
                  () =>
                    Effect.sync(() => {
                      expect(finalized).toBe(true)
                      dropped = true
                    }),
                ),
            },
          ),
        ),
        Effect.forkChild,
      )
      yield* Deferred.await(started)
      yield* Fiber.interrupt(fiber)
      expect(finalized).toBe(true)
      expect(dropped).toBe(true)
      const exit = yield* Fiber.await(fiber)
      expect(Exit.isFailure(exit) && Cause.hasInterrupts(exit.cause)).toBe(true)
    }),
  )

  it.effect(
    "retains committed-but-unacknowledged bytes, tuple and close through interruption",
    () =>
      Effect.gen(function* () {
        const started = yield* Deferred.make<void>()
        const requests: Host.DurableStreamAppendRequest[] = []
        let commits = 0
        const host = layer({
          append: (request) =>
            Effect.gen(function* () {
              requests.push(structuredClone(request))
              if (requests.length === 1) {
                commits++
                yield* Deferred.succeed(started, undefined)
                return yield* Effect.never
              }
              expect(request).toEqual(requests[0])
              return receipt(request, 7n)
            }),
        })
        const writer = yield* DS.makeByteWriter({ ...options, epoch: 7n }).pipe(
          Effect.provide(host),
        )
        const input = new Uint8Array([5, 201, 8])
        const fiber = yield* writer
          .append(input, { close: true })
          .pipe(Effect.provide(host), Effect.forkChild)
        yield* Deferred.await(started)
        input.fill(0)
        yield* Fiber.interrupt(fiber)
        expect(yield* writer.hasPending).toBe(true)
        expect(yield* writer.nextSequence).toBe(0n)
        const blocked = yield* writer
          .append(new Uint8Array([99]))
          .pipe(Effect.flip, Effect.provide(host))
        expect(blocked.kind).toBe("sequence-conflict")
        const ack = yield* writer.retryPending.pipe(Effect.provide(host))
        expect(ack.nextOffset).toBeUndefined()
        expect("duplicate" in ack).toBe(false)
        expect(requests).toHaveLength(2)
        expect(commits).toBe(1)
        expect(requests[1]!.sequence).toBe(0n)
        expect(requests[1]!.payload.val).toEqual(new Uint8Array([5, 201, 8]))
        expect(yield* writer.hasPending).toBe(false)
        expect(yield* writer.nextSequence).toBe(1n)
        expect((yield* writer.close.pipe(Effect.flip, Effect.provide(host))).kind).toBe("closed")
      }),
  )

  it.effect("serializes concurrent effects without allocating another producer", () =>
    Effect.gen(function* () {
      const requests: Host.DurableStreamAppendRequest[] = []
      const host = layer({
        append: (request) =>
          Effect.gen(function* () {
            requests.push(structuredClone(request))
            yield* Effect.yieldNow
            return receipt(request)
          }),
      })
      const writer = yield* DS.makeByteWriter(options).pipe(Effect.provide(host))
      yield* Effect.all([writer.append(new Uint8Array([13])), writer.close], {
        concurrency: 2,
      }).pipe(Effect.provide(host))
      expect(requests.map((r) => [r.sequence, r.close])).toEqual([
        [0n, false],
        [1n, true],
      ])
    }),
  )

  it.effect(
    "invalid, divergent, or missing-close acknowledgements never release pending state",
    () =>
      Effect.gen(function* () {
        for (const [override, kind] of [
          [{ epoch: 2n }, "protocol-error"],
          [{ sequence: 9n }, "producer-diverged"],
          [{ closed: false }, "protocol-error"],
        ] as const) {
          const writer = yield* DS.makeByteWriter(options).pipe(
            Effect.provide(
              layer({
                append: (request) => Effect.succeed({ ...receipt(request), ...override }),
              }),
            ),
          )
          const error = yield* writer.close.pipe(
            Effect.flip,
            Effect.provide(
              layer({
                append: (request) => Effect.succeed({ ...receipt(request), ...override }),
              }),
            ),
          )
          expect(error.kind).toBe(kind)
          expect(yield* writer.hasPending).toBe(true)
          expect(yield* writer.nextSequence).toBe(0n)
        }
      }),
  )

  it.effect("honors Retry-After and retries the same uncertain request", () =>
    Effect.gen(function* () {
      const requests: Host.DurableStreamAppendRequest[] = []
      const attempted = yield* Deferred.make<void>()
      const host = layer({
        append: (request) =>
          Effect.gen(function* () {
            requests.push(structuredClone(request))
            if (requests.length === 1) {
              yield* Deferred.succeed(attempted, undefined)
              return yield* Effect.fail(failure("rate-limited", 250n))
            }
            return receipt(request)
          }),
      })
      const writer = yield* DS.makeByteWriter({ ...options, maxRetries: 1, retryDelayMs: 10 }).pipe(
        Effect.provide(host),
      )
      const fiber = yield* writer.close.pipe(Effect.forkChild)
      yield* Deferred.await(attempted)
      yield* TestClock.adjust(249)
      expect(requests).toHaveLength(1)
      yield* TestClock.adjust(1)
      yield* Fiber.join(fiber)
      expect(requests).toHaveLength(2)
      expect(requests[1]).toEqual(requests[0])
    }),
  )

  it.effect("retains exhausted automatic retry budget until pending acknowledgement", () =>
    Effect.gen(function* () {
      const requests: Host.DurableStreamAppendRequest[] = []
      const host = layer({
        append: (request) =>
          Effect.suspend(() => {
            requests.push(structuredClone(request))
            return [1, 2, 3, 5].includes(requests.length)
              ? Effect.fail(failure("transport"))
              : Effect.succeed(receipt(request))
          }),
      })
      const writer = yield* DS.makeByteWriter({ ...options, maxRetries: 1, retryDelayMs: 10 }).pipe(
        Effect.provide(host),
      )
      const first = yield* writer
        .append(new Uint8Array([23]))
        .pipe(Effect.provide(host), Effect.forkChild)
      yield* TestClock.adjust(10)
      expect(Exit.isFailure(yield* Fiber.await(first))).toBe(true)
      expect(requests).toHaveLength(2)
      const retry = yield* writer.retryPending.pipe(Effect.provide(host), Effect.forkChild)
      yield* TestClock.adjust(1000)
      expect(Exit.isFailure(yield* Fiber.await(retry))).toBe(true)
      expect(requests).toHaveLength(3)
      expect(yield* writer.hasPending).toBe(true)
      yield* writer.retryPending.pipe(Effect.provide(host))
      expect(requests.slice(0, 4).every((request) => request.sequence === 0n)).toBe(true)
      const next = yield* writer.close.pipe(Effect.provide(host), Effect.forkChild)
      yield* TestClock.adjust(10)
      yield* Fiber.join(next)
      expect(requests).toHaveLength(6)
      expect(requests[4]!.sequence).toBe(1n)
      expect(yield* writer.hasPending).toBe(false)
    }),
  )

  it.effect("retains an interrupted backoff before explicitly retrying pending", () =>
    Effect.gen(function* () {
      let attempts = 0
      const host = layer({
        append: (request) =>
          Effect.suspend(() => {
            attempts++
            return attempts === 1
              ? Effect.fail(failure("transport"))
              : Effect.succeed(receipt(request))
          }),
      })
      const writer = yield* DS.makeByteWriter({
        ...options,
        maxRetries: 1,
        retryDelayMs: 100,
      }).pipe(Effect.provide(host))
      const first = yield* writer.close.pipe(Effect.provide(host), Effect.forkChild)
      yield* TestClock.adjust(25)
      yield* Fiber.interrupt(first)
      const retry = yield* writer.retryPending.pipe(Effect.provide(host), Effect.forkChild)
      yield* TestClock.adjust(99)
      expect(attempts).toBe(1)
      yield* TestClock.adjust(1)
      yield* Fiber.join(retry)
      expect(attempts).toBe(2)
    }),
  )

  it.effect(
    "decodes only demanded JSON messages and fails only when an invalid remainder is pulled",
    () =>
      Effect.gen(function* () {
        const decoded: string[] = []
        const source = DS.readJson(Schema.Number, {
          ...options,
          decode: (json) => {
            decoded.push(json)
            return JSON.parse(json)
          },
        })
        const host = layer({ read: () => Effect.succeed(batch(bytes('[37,"not-a-number"]'))) })
        expect(yield* source.pipe(Stream.take(1), Stream.runCollect, Effect.provide(host))).toEqual(
          [37],
        )
        expect(decoded).toEqual(["37"])
        const seen: number[] = []
        const exit = yield* source.pipe(
          Stream.tap((value) =>
            Effect.sync(() => {
              seen.push(value)
            }),
          ),
          Stream.runDrain,
          Effect.exit,
          Effect.provide(host),
        )
        expect(Exit.isFailure(exit)).toBe(true)
        expect(seen).toEqual([37])
      }),
  )

  it.effect("keeps schema arrays as messages and preserves exact unquoted u64 lexemes", () =>
    Effect.gen(function* () {
      const arrayValues = yield* Stream.runCollect(
        DS.readJson(Schema.Array(Schema.String), options),
      ).pipe(
        Effect.provide(
          layer({ read: () => Effect.succeed(batch(bytes('[["a,]","b\\"c"],["z"]]'))) }),
        ),
      )
      expect(arrayValues).toEqual([["a,]", 'b"c'], ["z"]])
      const big = 18446744073709551615n
      expect(
        yield* Stream.runCollect(DS.readJson(Uint64, { ...options, decode: BigInt })).pipe(
          Effect.provide(layer({ read: () => Effect.succeed(batch(bytes(`[${big}]`))) })),
        ),
      ).toEqual([big])
      const writer = yield* DS.makeJsonWriter(Uint64, { ...options, encode: String }).pipe(
        Effect.provide(
          layer({
            append: (request) =>
              Effect.sync(() => {
                expect(request.payload).toEqual({ tag: "json", val: ["18446744073709551615"] })
                return receipt(request)
              }),
          }),
        ),
      )
      yield* writer.append([big], { close: true })
      const failure = yield* Stream.runCollect(DS.readJson(Uint64, options)).pipe(
        Effect.flip,
        Effect.provide(layer({ read: () => Effect.succeed(batch(bytes(`[${big}]`))) })),
      )
      expect(failure._tag).toBe("DurableStreamError")
    }),
  )

  it.effect("rejects unsafe numeric literals symmetrically, including exponent notation", () =>
    Effect.gen(function* () {
      for (const literal of ["9007199254740993", "9007199254740993e0", "9007199254740993.0"]) {
        const error = yield* Stream.runCollect(DS.readJson(Schema.Number, options)).pipe(
          Effect.flip,
          Effect.provide(layer({ read: () => Effect.succeed(batch(bytes(`[${literal}]`))) })),
        )
        expect(error._tag).toBe("DurableStreamError")
      }
      const writer = yield* DS.makeJsonWriter(Schema.Number, options).pipe(
        Effect.provide(layer({})),
      )
      const error = yield* writer
        .append([9007199254740992])
        .pipe(Effect.flip, Effect.provide(layer({})))
      expect(error._tag).toBe("DurableStreamError")
      expect(yield* writer.hasPending).toBe(false)
      const safe = yield* Stream.runCollect(DS.readJson(Schema.Number, options)).pipe(
        Effect.provide(
          layer({ read: () => Effect.succeed(batch(bytes("[9007199254740991,1.25,-0.5]"))) }),
        ),
      )
      expect(safe).toEqual([9007199254740991, 1.25, -0.5])
    }),
  )

  it.effect(
    "rejects invalid JSON, schema violations and unresolved now rather than emitting EOF",
    () =>
      Effect.gen(function* () {
        for (const response of [
          batch(bytes("[1,]")),
          batch(bytes('["wrong"]')),
          batch(bytes("[]"), { next: { offset: "now", cursor: undefined } }),
        ]) {
          const exit = yield* Stream.runCollect(DS.readJson(Schema.Number, options)).pipe(
            Effect.exit,
            Effect.provide(layer({ read: () => Effect.succeed(response) })),
          )
          expect(Exit.isFailure(exit)).toBe(true)
        }
        const writer = yield* DS.makeJsonWriter(Uint64, options).pipe(Effect.provide(layer({})))
        yield* writer.append([18446744073709551615n]).pipe(Effect.flip, Effect.provide(layer({})))
        expect(yield* writer.hasPending).toBe(false)
        expect(yield* writer.nextSequence).toBe(0n)
      }),
  )

  it.effect(
    "reconstructs the same producer progress from recorded outcomes without live calls",
    () =>
      Effect.gen(function* () {
        // This is an SDK reconstruction assumption test, not an executor crash-recovery test.
        const descriptors: Host.DurableStreamWriterOptions[] = []
        const transcript: Host.DurableStreamAppendRequest[] = []
        const outcomes: Host.DurableStreamAppendReceipt[] = []
        const program = Effect.gen(function* () {
          const writer = yield* DS.makeByteWriter(options)
          yield* writer.append(new Uint8Array([17, 91]))
          yield* writer.close
          return yield* writer.nextSequence
        })
        expect(
          yield* program.pipe(
            Effect.provide(
              layer(
                {},
                {
                  makeWriter: (descriptor) =>
                    Effect.sync(() => {
                      descriptors.push(structuredClone(descriptor))
                      return {
                        append: (request) =>
                          Effect.sync(() => {
                            transcript.push(structuredClone(request))
                            const ack = receipt(request)
                            outcomes.push(ack)
                            return ack
                          }),
                      }
                    }),
                },
              ),
            ),
          ),
        ).toBe(2n)
        expect(descriptors).toEqual([
          {
            url: options.url,
            contentType: "application/octet-stream",
            producerId: "producer",
            producerEpoch: 0n,
            timeoutMs: 30000n,
          },
        ])
        let replay = 0
        let constructions = 0
        expect(
          yield* program.pipe(
            Effect.provide(
              layer(
                {},
                {
                  makeWriter: (descriptor) =>
                    Effect.sync(() => {
                      expect(descriptor).toEqual(descriptors[constructions++])
                      return {
                        append: (request) =>
                          Effect.sync(() => {
                            expect(request).toEqual(transcript[replay])
                            return outcomes[replay++]!
                          }),
                      }
                    }),
                },
              ),
            ),
          ),
        ).toBe(2n)
        expect(constructions).toBe(1)
        expect(replay).toBe(2)
      }),
  )

  it.effect("passes the borrowed capability unchanged without inspecting or revealing it", () =>
    Effect.gen(function* () {
      const auth = Object.freeze({}) as Secret
      yield* Stream.runDrain(DS.readBytes({ ...options, auth })).pipe(
        Effect.provide(
          layer(
            {},
            {
              makeReader: (_options, secret) =>
                Effect.sync(() => {
                  expect(secret).toBe(auth)
                  return { read: () => Effect.succeed(batch(new Uint8Array())) }
                }),
            },
          ),
        ),
      )
      const writer = yield* DS.makeByteWriter({ ...options, auth }).pipe(
        Effect.provide(
          layer(
            {},
            {
              makeWriter: (_options, secret) =>
                Effect.sync(() => {
                  expect(secret).toBe(auth)
                  return { append: (request) => Effect.succeed(receipt(request)) }
                }),
            },
          ),
        ),
      )
      yield* writer.close
    }),
  )
})
