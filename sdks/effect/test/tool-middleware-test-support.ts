import type * as Streams from "golem:tool/streams@0.1.0"

export type ByteStreamItem = Streams.ByteStreamItem

export const byteItems = (
  source: AsyncIterable<number>,
): AsyncIterable<Streams.ByteStreamItem> => ({
  [Symbol.asyncIterator]() {
    const iterator = source[Symbol.asyncIterator]()
    return {
      async next(): Promise<IteratorResult<Streams.ByteStreamItem>> {
        const next = await iterator.next()
        return next.done
          ? { done: true, value: undefined }
          : { done: false, value: { tag: "ok", val: Uint8Array.of(next.value) } }
      },
      async return(): Promise<IteratorResult<Streams.ByteStreamItem>> {
        await iterator.return?.()
        return { done: true, value: undefined }
      },
    }
  },
})

export const startMiddleware = <I extends (...args: any[]) => Promise<any>>(
  invoke: I,
  ...args: Parameters<I>
) => {
  type Pending = {
    readonly bytes: Uint8Array
    offset: number
    readonly resolve: () => void
    readonly reject: (error: Streams.StreamWriteError) => void
  }
  const queued: Pending[] = []
  let waiting:
    | {
        readonly resolve: (result: IteratorResult<number>) => void
        readonly reject: (error: unknown) => void
      }
    | undefined
  let terminal: IteratorResult<number> | undefined
  let failure: unknown
  let closed = false

  const deliver = () => {
    if (!waiting) return
    if (failure !== undefined) {
      const { reject } = waiting
      waiting = undefined
      reject(failure)
      return
    }
    const pending = queued[0]
    if (pending) {
      const { resolve } = waiting
      waiting = undefined
      const value = pending.bytes[pending.offset++]!
      if (pending.offset === pending.bytes.length) {
        queued.shift()
        pending.resolve()
      }
      resolve({ done: false, value })
    } else if (terminal) {
      const { resolve } = waiting
      waiting = undefined
      resolve(terminal)
    }
  }

  const closeError = (): Streams.StreamWriteError => ({
    tag: "closed",
    val: { tag: "consumer-cancelled" },
  })
  const writer = {
    write: (bytes: Uint8Array) =>
      new Promise<void>((resolve, reject) => {
        if (closed) return reject(closeError())
        queued.push({ bytes, offset: 0, resolve, reject })
        deliver()
      }),
    finish: async () => {
      if (closed) throw closeError()
      terminal = { done: true, value: undefined }
      deliver()
    },
    fail: async (reason: Streams.ByteStreamFailure) => {
      if (closed) throw closeError()
      failure = reason
      deliver()
    },
  } satisfies Pick<Streams.ToolStdoutWriter, "write" | "finish" | "fail">
  const stdout: AsyncIterableIterator<number> = {
    [Symbol.asyncIterator]() {
      return this
    },
    next() {
      if (failure !== undefined) return Promise.reject(failure)
      const pending = queued[0]
      if (pending) {
        const value = pending.bytes[pending.offset++]!
        if (pending.offset === pending.bytes.length) {
          queued.shift()
          pending.resolve()
        }
        return Promise.resolve({ done: false as const, value })
      }
      if (terminal) return Promise.resolve(terminal)
      return new Promise<IteratorResult<number>>((resolve, reject) => {
        waiting = { resolve, reject }
      })
    },
    async return() {
      closed = true
      terminal = { done: true, value: undefined }
      const error = closeError()
      for (const pending of queued.splice(0)) pending.reject(error)
      deliver()
      return terminal
    },
  }
  ;(args as unknown[])[7] = writer as Streams.ToolStdoutWriter
  return { completion: invoke(...args), stdout }
}
