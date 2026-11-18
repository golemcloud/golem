import type { SchemaValueStream, SchemaValueTree, StreamOutcome } from "golem:core/types@2.0.0"
import { STREAM_INTERNAL, type StreamInternal } from "./streamInternal.js"
import { assertCapabilityReady } from "./capabilityTransaction.js"
import { StreamError } from "./errors.js"
import { schemaGraphToWit, schemaValueToWitAsync, typedSchemaValueFromWit } from "./wit.js"

export async function wrapSchemaValueStream(
  source: AsyncIterable<SchemaValueTree>,
): Promise<SchemaValueStream> {
  let resolve!: (outcome: StreamOutcome | undefined) => void
  const outcome = new Promise<StreamOutcome | undefined>((done) => {
    resolve = done
  })
  const items = {
    [Symbol.asyncIterator]() {
      let iterator: AsyncIterator<SchemaValueTree> | undefined
      let closed = false
      return {
        async next(): Promise<IteratorResult<SchemaValueTree>> {
          if (closed) return { done: true, value: undefined }
          try {
            iterator ??= source[Symbol.asyncIterator]()
            const item = await iterator.next()
            if (!item.done) return item
            closed = true
            resolve({ tag: "ok", val: undefined })
          } catch (error) {
            closed = true
            const failure = StreamError.from(error).failure
            resolve({
              tag: "err",
              val:
                failure.tag === "failed"
                  ? {
                      tag: "failed",
                      val: {
                        graph: schemaGraphToWit(failure.val.graph),
                        value: await schemaValueToWitAsync(failure.val.value),
                      },
                    }
                  : failure,
            })
          }
          return { done: true, value: undefined }
        },
        async return(): Promise<IteratorResult<SchemaValueTree>> {
          if (!closed) {
            closed = true
            resolve(undefined)
            iterator ??= source[Symbol.asyncIterator]()
            await iterator.return?.()
          }
          return { done: true, value: undefined }
        },
      }
    },
  }
  const { SchemaValueStream } = await import("golem:core/types@2.0.0")
  return SchemaValueStream.wrap(items, outcome)
}

export async function unwrapSchemaValueStream(
  value: SchemaValueStream,
): Promise<AsyncIterable<SchemaValueTree>> {
  const { SchemaValueStream } = await import("golem:core/types@2.0.0")
  const [items, outcome] = await SchemaValueStream.unwrap(value)
  return {
    [Symbol.asyncIterator]() {
      const iterator = items[Symbol.asyncIterator]()
      let closed = false
      return {
        async next(): Promise<IteratorResult<SchemaValueTree>> {
          if (closed) return { done: true, value: undefined }
          const item = await iterator.next()
          if (!item.done) return item
          closed = true
          const result = await outcome
          if (result.tag === "err") {
            const failure = result.val
            throw new StreamError(
              failure.tag === "failed"
                ? {
                    tag: "failed",
                    val: typedSchemaValueFromWit(failure.val),
                  }
                : failure,
            )
          }
          return { done: true, value: undefined }
        },
        async return(): Promise<IteratorResult<SchemaValueTree>> {
          closed = true
          await iterator.return?.()
          return { done: true, value: undefined }
        },
      }
    },
  }
}

export type GuestSchemaValueStream =
  | { kind: "wrapped"; value: SchemaValueStream }
  | { kind: "native"; value: AsyncIterable<SchemaValueTree> }

export interface StreamOwnership {
  available: boolean
  reservation?: object
  busy?: boolean
  reader?: object
  iterator?: AsyncIterator<SchemaValueTree>
}

export class GuestSchemaValueStreamHandle {
  #value: GuestSchemaValueStream | undefined
  readonly #ownership: StreamOwnership
  readonly #onTake: (() => void) | undefined
  readonly #onRelease: (() => void) | undefined

  constructor(
    key: StreamInternal,
    value: GuestSchemaValueStream,
    onTake?: () => void,
    onRelease?: () => void,
    ownership: StreamOwnership = { available: true },
  ) {
    if (key !== STREAM_INTERNAL) {
      throw new Error("GuestSchemaValueStreamHandle construction is an internal SDK operation")
    }
    this.#value = value
    this.#ownership = ownership
    this.#onTake = onTake
    this.#onRelease = onRelease
  }

  ownership(key: StreamInternal): StreamOwnership {
    if (key !== STREAM_INTERNAL) throw new Error("stream ownership is internal")
    return this.#ownership
  }

  peek(): GuestSchemaValueStream | undefined {
    return this.#ownership.available ? this.#value : undefined
  }

  reserve(owner: object): void {
    assertCapabilityReady(this.#ownership)
    if (this.#ownership.reader) throw new Error("schema stream already has a reader")
    if (this.#ownership.busy) throw new Error("a schema stream operation is in progress")
    if (this.#ownership.reservation !== undefined || this.peek() === undefined) {
      throw new Error("schema stream is already reserved or transferred")
    }
    this.#ownership.reservation = owner
  }

  unreserve(owner: object): void {
    if (this.#ownership.reservation === owner) this.#ownership.reservation = undefined
  }

  take(owner?: object): GuestSchemaValueStream | undefined {
    assertCapabilityReady(this.#ownership)
    if (this.#ownership.reader) throw new Error("schema stream already has a reader")
    if (this.#ownership.busy) throw new Error("a schema stream operation is in progress")
    if (this.#ownership.reservation !== owner)
      throw new Error("schema stream is reserved for transfer")
    this.#ownership.reservation = undefined
    const value = this.peek()
    this.#ownership.available = false
    this.#value = undefined
    if (value !== undefined) this.#onTake?.()
    return value
  }

  release(): void {
    assertCapabilityReady(this.#ownership)
    if (this.#ownership.reader) throw new Error("schema stream already has a reader")
    if (this.#ownership.busy) throw new Error("a schema stream operation is in progress")
    if (this.#ownership.reservation !== undefined)
      throw new Error("schema stream is reserved for transfer")
    if (this.peek() !== undefined) {
      this.#ownership.available = false
      this.#value = undefined
      this.#onRelease?.()
    }
  }
}
