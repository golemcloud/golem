import { Cause, Context, Effect, Exit, Fiber, Stream } from "effect"
import { describe, expect, it } from "vitest"
import {
  AgentStream,
  agentStreamFromHandle,
  agentStreamToHandle,
} from "../src/internal/agentStream.js"
import { toWitCodec } from "../src/WitCodec.js"
import { Uint32 } from "../src/WitTypes.js"

const numbers = Effect.runSync(toWitCodec(Uint32)).codec

describe("AgentStream Effect bridge", () => {
  it("interrupts a pending pull and joins its asynchronous finalizer", async () => {
    let started!: () => void
    const pulling = new Promise<void>((resolve) => {
      started = resolve
    })
    let finalizing!: () => void
    const finalizerStarted = new Promise<void>((resolve) => {
      finalizing = resolve
    })
    let release!: () => void
    const allowed = new Promise<void>((resolve) => {
      release = resolve
    })
    let finalized = 0
    const source = Stream.fromEffect(
      Effect.sync(() => started()).pipe(
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
    const stream = await Effect.runPromise(AgentStream.fromEffect(source))
    const consumer = Effect.runFork(Stream.runDrain(stream.toEffect(String)))
    await pulling
    let interrupted = false
    const interruption = Effect.runPromise(Fiber.interrupt(consumer)).then(() => {
      interrupted = true
    })
    await finalizerStarted
    expect(interrupted).toBe(false)
    expect(finalized).toBe(0)
    release()
    await interruption
    const exit = await Effect.runPromise(Fiber.await(consumer))
    expect(Exit.isFailure(exit) && Cause.hasDies(exit.cause)).toBe(false)
    expect(finalized).toBe(1)
    await stream.return()
    expect(finalized).toBe(1)
  })

  it("consumes an Effect Stream lazily", async () => {
    const stream = await Effect.runPromise(AgentStream.fromEffect(Stream.make(1, 2, 3)))
    const values = await Effect.runPromise(Stream.runCollect(stream.toEffect(String)))
    expect([...values]).toEqual([1, 2, 3])
  })

  it("captures Effect services and finalizes the source on early termination", async () => {
    class Input extends Context.Service<Input, { readonly value: number }>()("test/Input") {}
    let acquired = 0
    let released = 0
    const source = Stream.fromEffect(
      Effect.gen(function* () {
        const input = yield* Input
        yield* Effect.acquireRelease(
          Effect.sync(() => acquired++),
          () => Effect.sync(() => released++),
        )
        return input.value
      }),
    ).pipe(Stream.concat(Stream.never), Stream.scoped)
    const stream = await Effect.runPromise(
      AgentStream.fromEffect(source).pipe(Effect.provideService(Input, { value: 37 })),
    )
    expect(acquired).toBe(0)
    const values = await Effect.runPromise(
      stream.toEffect(String).pipe(Stream.take(1), Stream.runCollect),
    )
    expect([...values]).toEqual([37])
    expect(acquired).toBe(1)
    expect(released).toBe(1)
  })

  it("blocks local operations while reserved and restores them on release", async () => {
    const stream = AgentStream.from([19, 41])
    const handle = agentStreamToHandle(stream, numbers)
    const transaction = {}
    handle.reserve(transaction)
    await expect(stream.next()).rejects.toThrow(/reserved/)
    await expect(stream.return()).rejects.toThrow(/reserved/)
    handle.unreserve(transaction)
    await expect(stream.next()).resolves.toEqual({ done: false, value: 19 })
    await stream.return()
  })

  it("transfers ownership once", () => {
    const stream = AgentStream.from([1])
    const first = agentStreamToHandle(stream, numbers)
    const alias = agentStreamToHandle(stream, numbers)
    expect(first.take()).toBeDefined()
    expect(alias.take()).toBeUndefined()
    expect(() => agentStreamToHandle(stream, numbers)).toThrow(/transferred|closed|reserved/)
  })

  it("shares an iterator and closed state between decoded aliases", async () => {
    const source = AgentStream.from([17, 93])
    const handle = agentStreamToHandle(source, numbers)
    const first = agentStreamFromHandle(handle, numbers)
    const second = agentStreamFromHandle(handle, numbers)
    expect(await first.next()).toEqual({ done: false, value: 17 })
    expect(await second.next()).toEqual({ done: false, value: 93 })
    await first.return()
    await expect(second.next()).rejects.toThrow(/closed/)
    await expect(source.next()).rejects.toThrow(/closed/)
    expect(handle.take()).toBeUndefined()
  })

  it("blocks transfer and alias reads while a source read is pending", async () => {
    let resume!: (item: IteratorResult<number>) => void
    const stream = AgentStream.from({
      [Symbol.asyncIterator]: () => ({
        next: () =>
          new Promise<IteratorResult<number>>((resolve) => {
            resume = resolve
          }),
      }),
    })
    const handle = agentStreamToHandle(stream, numbers)
    const alias = agentStreamFromHandle(handle, numbers)
    const pending = stream.next()
    await expect(alias.next()).rejects.toThrow(/in progress/)
    expect(() => handle.reserve({})).toThrow(/in progress/)
    expect(() => handle.take()).toThrow(/in progress/)
    resume({ done: false, value: 43 })
    expect(await pending).toEqual({ done: false, value: 43 })
    await stream.return()
  })

  it("refuses JSON persistence", () => {
    expect(() => JSON.stringify(AgentStream.from([5]))).toThrow(/cannot be serialized/)
  })
})
