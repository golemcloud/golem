/** A transport producer that cannot touch its source before ownership handoff. */
export class PreparedStream<A> implements AsyncIterable<A>, AsyncIterator<A> {
  private state: "pending" | "committed" | "aborted" | "closed" = "pending"
  private wake!: () => void
  private readonly ready = new Promise<void>((resolve) => {
    this.wake = resolve
  })
  private iterator: AsyncIterator<A> | undefined
  private busy = false

  constructor(private readonly source: AsyncIterable<A>) {}

  get pending(): boolean {
    return this.state === "pending"
  }

  commit(): void {
    if (this.state !== "pending") throw new Error("stream preparation is no longer pending")
    this.state = "committed"
    this.wake()
  }

  abort(): void {
    if (this.state !== "pending") return
    this.state = "aborted"
    this.wake()
  }

  [Symbol.asyncIterator](): AsyncIterator<A> {
    return this
  }

  async next(): Promise<IteratorResult<A>> {
    if (this.busy) throw new Error("a stream read is already in progress")
    this.busy = true
    try {
      await this.ready
      if (this.state !== "committed") return { done: true, value: undefined }
      this.iterator ??= this.source[Symbol.asyncIterator]()
      return await this.iterator.next()
    } finally {
      this.busy = false
    }
  }

  async return(): Promise<IteratorResult<A>> {
    const committed = this.state === "committed"
    this.state = "closed"
    this.wake()
    if (committed && this.iterator?.return !== undefined) await this.iterator.return()
    return { done: true, value: undefined }
  }
}
