import { Cause, Context, Effect, Exit, Fiber, Stream } from "effect"
import { describe, expect, it } from "vitest"
import { agentStreamFromHandle, agentStreamToHandle } from "../src/internal/agentStream.js"
import { toWitCodec } from "../src/WitCodec.js"
import { Uint32 } from "../src/WitTypes.js"

const numbers = Effect.runSync(toWitCodec(Uint32)).codec

describe("native Effect agent streams", () => {
  it("interrupts a pending pull and joins its asynchronous finalizer", async () => {
    let started!: () => void
    const pulling = new Promise<void>((resolve) => (started = resolve))
    let finalizing!: () => void
    const finalizerStarted = new Promise<void>((resolve) => (finalizing = resolve))
    let release!: () => void
    const allowed = new Promise<void>((resolve) => (release = resolve))
    let finalized = 0
    const source = Stream.fromEffect(
      Effect.sync(started).pipe(
        Effect.andThen(Effect.never),
        Effect.ensuring(
          Effect.promise(async () => {
            finalizing()
            await allowed
            finalized++
          }),
        ),
      ),
    )
    const forwarded = agentStreamFromHandle(agentStreamToHandle(source, numbers), numbers)
    const consumer = Effect.runFork(Stream.runDrain(forwarded))
    await pulling
    let interrupted = false
    const interruption = Effect.runPromise(Fiber.interrupt(consumer)).then(
      () => (interrupted = true),
    )
    await finalizerStarted
    expect(interrupted).toBe(false)
    release()
    await interruption
    const exit = await Effect.runPromise(Fiber.await(consumer))
    expect(Exit.isFailure(exit) && Cause.hasDies(exit.cause)).toBe(false)
    expect(finalized).toBe(1)
  })

  it("keeps local programs reusable and lazy", async () => {
    let pulls = 0
    const source = Stream.fromIterable([1, 2, 3]).pipe(Stream.tap(() => Effect.sync(() => pulls++)))
    const first = agentStreamFromHandle(agentStreamToHandle(source, numbers), numbers)
    expect(pulls).toBe(0)
    expect([...(await Effect.runPromise(Stream.runCollect(first)))]).toEqual([1, 2, 3])
    const second = agentStreamFromHandle(agentStreamToHandle(source, numbers), numbers)
    expect([...(await Effect.runPromise(Stream.runCollect(second)))]).toEqual([1, 2, 3])
    expect(pulls).toBe(6)
  })

  it("captures Effect services used after the invocation boundary", async () => {
    class Input extends Context.Service<Input, { readonly value: number }>()("test/Input") {}
    const source = Stream.fromEffect(Effect.map(Input, (_) => _.value))
    const handle = await Effect.runPromise(
      Effect.map(Effect.context<Input>(), (context) =>
        agentStreamToHandle(source, numbers, context),
      ).pipe(Effect.provideService(Input, { value: 37 })),
    )
    const result = agentStreamFromHandle(handle, numbers)
    expect([...(await Effect.runPromise(Stream.runCollect(result)))]).toEqual([37])
  })

  it("forwards an unread received handle without pulling", async () => {
    let pulls = 0
    const source = Stream.make(17, 93).pipe(Stream.tap(() => Effect.sync(() => pulls++)))
    const received = agentStreamFromHandle(agentStreamToHandle(source, numbers), numbers)
    const forwarded = agentStreamFromHandle(agentStreamToHandle(received, numbers), numbers)
    expect(pulls).toBe(0)
    expect([...(await Effect.runPromise(Stream.runCollect(forwarded)))]).toEqual([17, 93])
  })

  it("makes received endpoints one-shot across mapped aliases", async () => {
    const stream = agentStreamFromHandle(agentStreamToHandle(Stream.make(1, 2), numbers), numbers)
    const alias = stream.pipe(Stream.map((n) => n * 10))
    expect([...(await Effect.runPromise(Stream.runCollect(alias)))]).toEqual([10, 20])
    await expect(Effect.runPromise(Stream.runCollect(stream))).rejects.toThrow(/closed|transferred/)
  })

  it("rejects non-Stream values at the schema boundary", () => {
    expect(() => agentStreamToHandle([1, 2] as never, numbers)).toThrow(/Effect Stream/)
  })

  it.each(["pending pull", "between pulls"])("keeps one reader during %s", async (phase) => {
    let started!: () => void
    const paused = new Promise<void>((resolve) => (started = resolve))
    let release!: () => void
    const gate = new Promise<void>((resolve) => (release = resolve))
    let closed = 0
    let pulls = 0
    const pause = Effect.promise(async () => {
      started()
      await gate
    })
    const source = Stream.make(11, 29, 47).pipe(
      Stream.tap((n) =>
        Effect.sync(() => pulls++).pipe(
          Effect.andThen(n === 11 && phase === "pending pull" ? pause : Effect.void),
        ),
      ),
      Stream.ensuring(Effect.sync(() => closed++)),
    )
    const handle = agentStreamToHandle(source, numbers)
    const received = agentStreamFromHandle(handle, numbers)
    const prepared = agentStreamToHandle(received, numbers)
    const winner = Effect.runPromise(
      Stream.runCollect(
        received.pipe(
          Stream.tap((n) => (n === 11 && phase === "between pulls" ? pause : Effect.void)),
        ),
      ),
    )
    await paused
    try {
      await expect(
        Effect.runPromise(Stream.runCollect(received.pipe(Stream.map((n) => n * 2)))),
      ).rejects.toThrow(/reader/)
      expect(() => agentStreamToHandle(received, numbers)).toThrow(/reader/)
      expect(() => prepared.take()).toThrow(/reader/)
      expect(() => handle.reserve({})).toThrow(/reader/)
      expect(closed).toBe(0)
      expect(pulls).toBe(1)
    } finally {
      release()
    }
    expect(await winner).toEqual([11, 29, 47])
    expect(closed).toBe(1)
  })

  it("claims a transformed producer lazily and never duplicates its received endpoint", async () => {
    const received = agentStreamFromHandle(
      agentStreamToHandle(Stream.make(13, 31), numbers),
      numbers,
    )
    const alias = agentStreamFromHandle(
      agentStreamToHandle(received.pipe(Stream.map((n) => n * 2)), numbers),
      numbers,
    )
    const moved = agentStreamToHandle(received, numbers)
    const endpoint = moved.take()!
    expect(endpoint.kind).toBe("native")
    await expect(Effect.runPromise(Stream.runCollect(alias))).rejects.toThrow(/transferred/)
    expect(() => agentStreamToHandle(received, numbers)).toThrow(/transferred/)
  })
})
