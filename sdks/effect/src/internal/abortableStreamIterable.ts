import { Context, Effect, Exit, Pull, Scope, Stream } from "effect"

/** An iterator whose close interrupts and joins an in-flight Effect pull before scope release. */
export class AbortableStreamIterable<A, E, R> implements AsyncIterableIterator<A> {
  private readonly scope = Scope.makeUnsafe()
  private readonly runPromise: ReturnType<typeof Effect.runPromiseWith<R>>
  private pull: Effect.Effect<ReadonlyArray<A>, unknown, R> | undefined
  private current: Iterator<A> | undefined
  private pending: AbortController | undefined
  private execution: Promise<unknown> | undefined
  private closePromise: Promise<void> | undefined

  constructor(
    private readonly stream: Stream.Stream<A, E, R>,
    context: Context.Context<R>,
  ) {
    this.runPromise = Effect.runPromiseWith(context)
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<A> {
    return this
  }

  async next(): Promise<IteratorResult<A>> {
    if (this.closePromise) return { done: true, value: undefined }
    const buffered = this.current?.next()
    if (buffered && !buffered.done) return buffered
    this.current = undefined
    const controller = new AbortController()
    this.pending = controller
    try {
      if (!this.pull) {
        const acquisition = this.runPromise(
          Stream.toPull(this.stream).pipe(Scope.provide(this.scope)),
          { signal: controller.signal },
        )
        this.execution = acquisition
        this.pull = await acquisition
        if (this.closePromise) return { done: true, value: undefined }
      }
      const pulling = this.runPromise(
        Pull.catchDone(this.pull, () => Effect.succeed<ReadonlyArray<A>>([])),
        { signal: controller.signal },
      )
      this.execution = pulling
      const chunk = await pulling
      if (chunk.length === 0) {
        await this.close()
        return { done: true, value: undefined }
      }
      this.current = chunk[Symbol.iterator]()
      return this.current.next()
    } catch (error) {
      if (controller.signal.aborted) {
        await this.close()
        return { done: true, value: undefined }
      }
      await this.close().catch(() => undefined)
      throw error
    } finally {
      if (this.pending === controller) this.pending = undefined
    }
  }

  return(): Promise<IteratorResult<A>> {
    return this.close().then(() => ({ done: true, value: undefined }))
  }

  close(): Promise<void> {
    if (!this.closePromise) {
      this.pending?.abort()
      this.closePromise = (async () => {
        await this.execution?.catch(() => undefined)
        await this.runPromise(Scope.close(this.scope, Exit.void))
      })()
    }
    return this.closePromise
  }
}
