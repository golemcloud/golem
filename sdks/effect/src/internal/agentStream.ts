// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import type { SchemaValueTree } from "golem:core/types@2.0.0"
import { Context, Effect, Schema, Stream as EffectStream } from "effect"
import {
  GuestSchemaValueStreamHandle,
  type GuestSchemaValueStream,
  type StreamOwnership,
} from "./schema-model/schemaValueStreamHandle.js"
import { STREAM_INTERNAL } from "./schema-model/streamInternal.js"
import { assertCapabilityReady } from "./schema-model/capabilityTransaction.js"
import { schemaValueToWitAsync } from "./schema-model/wit.js"
import { decodeFromWire } from "../WitCodec.js"
import { AbortableStreamIterable } from "./abortableStreamIterable.js"

type ItemCodec<T> = Schema.Codec<T, import("./schema-model/model.js").SchemaValue, any, any>

interface TypedStreamState<T> {
  readonly kind: "typed"
  readonly source: AsyncIterable<T>
  iterator?: AsyncIterator<T>
}

interface WireStreamState {
  readonly kind: "wire"
  endpoint?: GuestSchemaValueStream
  readonly itemCodec: ItemCodec<any>
  readonly decodingContext: Context.Context<any>
}

type AgentStreamState<T> = (TypedStreamState<T> | WireStreamState) & { ownership: StreamOwnership }

const states = new WeakMap<object, AgentStreamState<unknown>>()
const closing = new WeakMap<object, Promise<void>>()
const acquiring = new WeakMap<StreamOwnership, Promise<AsyncIterator<SchemaValueTree>>>()

/**
 * A demand-driven, single-reader stream for values nested anywhere in an agent method input or
 * output.
 *
 * Reading is lazy and non-concurrent: wait for each operation before starting another. Normal
 * completion returns `{ done: true, value: undefined }`. Encoding or forwarding an `AgentStream`
 * transfers its ownership; the original object cannot be used again.
 *
 * For a stream received through a connected P3 agent invocation, calling `return()`, including the
 * call made by an early exit from `for await`, closes its readable endpoint. Calling `throw()` also
 * closes it and rejects with the same local reason; the reason is not transmitted to the producer.
 *
 * When a stream created with {@link AgentStream.from} is sent through a connected P3 invocation,
 * each accepted downstream write gates the next source pull. If a subsequent write observes that
 * the remote reader was dropped, the runtime stops pulling and invokes and awaits the source
 * iterator's `return()` exactly once. P3 does not interrupt an arbitrary pending source `next()` or
 * guarantee cleanup before a later agent invocation. A source or cleanup rejection fails the
 * active producer, write, or invocation session rather than appearing as clean EOF. P3 streams do
 * not carry a recoverable terminal error; use an item type such as `Result<T, E>` when errors must
 * be represented in the stream contract.
 */
export class AgentStream<T> implements AsyncIterable<T>, AsyncIterator<T> {
  private constructor() {}

  /**
   * Create a stream from a synchronous or asynchronous iterable without pulling it eagerly.
   *
   * The source iterator's `return()` is used for cleanup when either the local stream is closed or
   * a connected remote reader is dropped.
   */
  static from<T>(source: Iterable<T> | AsyncIterable<T>): AgentStream<T> {
    return createAgentStream({
      kind: "typed",
      source: asAsyncIterable(source),
    })
  }

  /** Capture the source's Effect services without starting its scoped producer. */
  static fromEffect<T, E, R>(
    source: EffectStream.Stream<T, E, R>,
  ): Effect.Effect<AgentStream<T>, never, R> {
    return Effect.map(Effect.context<R>(), (context) =>
      AgentStream.from(new AbortableStreamIterable(source, context)),
    )
  }

  /** View this affine stream as an Effect Stream; early termination closes it. */
  toEffect<E>(onError: (cause: unknown) => E): EffectStream.Stream<T, E> {
    return EffectStream.fromAsyncIterable(this, onError)
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return this
  }

  /** Read the next item, or `{ done: true, value: undefined }` after clean completion. */
  async next(): Promise<IteratorResult<T>> {
    const state = streamState(this)
    if (state.ownership.busy) {
      throw new Error("an AgentStream operation is already in progress")
    }
    state.ownership.busy = true
    try {
      if (state.kind === "typed") {
        state.iterator ??= state.source[Symbol.asyncIterator]()
        const item = await state.iterator.next()
        return item.done ? { done: true, value: undefined } : item
      }

      const iterator = await wireIterator(state)
      const item = await iterator.next()
      return item.done
        ? { done: true, value: undefined }
        : {
            done: false,
            value: (await decodeItem(state.itemCodec, item.value, state.decodingContext)) as T,
          }
    } finally {
      state.ownership.busy = false
    }
  }

  /**
   * Close the stream and its underlying iterator. `for await` invokes this automatically on an
   * early `break` or `return`.
   */
  async return(value?: unknown): Promise<IteratorResult<T>> {
    let close = closing.get(this)
    if (!close) {
      const state = streamState(this)
      state.ownership.available = false
      states.delete(this)
      close = Promise.resolve().then(async () => {
        const iterator =
          state.kind === "typed"
            ? (state.iterator ??= state.source[Symbol.asyncIterator]())
            : await wireIterator(state)
        await iterator.return?.(value)
      })
      closing.set(this, close)
    }
    await close
    return { done: true, value: value as T }
  }

  /**
   * Fail local iteration and close a connected readable endpoint.
   *
   * Connected P3 streams reject with the same local reason, but do not transfer that reason to the
   * producer. Streams created with {@link AgentStream.from} delegate to the source iterator's
   * `throw()` when it provides one, but the `AgentStream` is consumed regardless of its result.
   */
  async throw(error?: unknown): Promise<IteratorResult<T>> {
    const state = streamState(this)
    if (state.ownership.busy) {
      throw new Error("an AgentStream operation is already in progress")
    }
    state.ownership.busy = true
    try {
      state.ownership.available = false
      states.delete(this)
      const iterator =
        state.kind === "typed"
          ? (state.iterator ??= state.source[Symbol.asyncIterator]())
          : await wireIterator(state)
      if (iterator.throw) {
        return (await iterator.throw(error)) as IteratorResult<T>
      }
      if (iterator.return) {
        await iterator.return()
      }
      throw error
    } finally {
      state.ownership.busy = false
    }
  }

  /** Live endpoints cannot be persisted as ordinary JSON state. */
  toJSON(): never {
    throw new Error("AgentStream cannot be serialized; transfer it through a schema value")
  }
}

/** @internal Move an AgentStream into a recursive schema value. */
export function agentStreamToHandle<T>(
  stream: AgentStream<T>,
  itemCodec: ItemCodec<T>,
  encodingContext: Context.Context<any> = Context.empty() as Context.Context<any>,
): GuestSchemaValueStreamHandle {
  const state = streamState(stream)
  if (state.ownership.busy) {
    throw new Error("cannot transfer an AgentStream while an operation is in progress")
  }
  const handle = (endpoint: GuestSchemaValueStream): GuestSchemaValueStreamHandle => {
    return new GuestSchemaValueStreamHandle(
      STREAM_INTERNAL,
      endpoint,
      () => states.delete(stream),
      undefined,
      state.ownership,
    )
  }

  if (state.kind === "wire") {
    if (state.ownership.iterator !== undefined) {
      return handle({ kind: "native", value: iterableFromIterator(state.ownership.iterator) })
    }
    if (state.endpoint !== undefined) {
      return handle(state.endpoint)
    }
    throw new Error("AgentStream was already transferred")
  }

  const source: AsyncIterable<T> = {
    [Symbol.asyncIterator]: () => (state.iterator ??= state.source[Symbol.asyncIterator]()),
  }
  return handle({
    kind: "native",
    value: encodeItems(source, itemCodec, encodingContext),
  })
}

/** @internal Lift a recursive schema-value-stream handle into an AgentStream. */
export function agentStreamFromHandle<T>(
  handle: GuestSchemaValueStreamHandle,
  itemCodec: ItemCodec<T>,
  decodingContext: Context.Context<any> = Context.empty() as Context.Context<any>,
): AgentStream<T> {
  const endpoint = handle.peek()
  if (endpoint === undefined) {
    throw new Error("schema value stream was already transferred")
  }
  return createAgentStream(
    {
      kind: "wire",
      endpoint,
      itemCodec,
      decodingContext,
    },
    handle.ownership(STREAM_INTERNAL),
  )
}

function createAgentStream<T>(
  state: TypedStreamState<T> | WireStreamState,
  ownership: StreamOwnership = { available: true },
): AgentStream<T> {
  const stream = Object.create(AgentStream.prototype) as AgentStream<T>
  states.set(stream, { ...state, ownership } as AgentStreamState<unknown>)
  return stream
}

function streamState<T>(stream: AgentStream<T>): AgentStreamState<T> {
  const state = states.get(stream) as AgentStreamState<T> | undefined
  if (state === undefined || !state.ownership.available) {
    throw new Error("AgentStream was already transferred or closed")
  }
  assertCapabilityReady(state.ownership)
  if (state.ownership.reservation !== undefined) {
    throw new Error("AgentStream is reserved for transfer")
  }
  return state
}

async function wireIterator(
  state: WireStreamState & { ownership: StreamOwnership },
): Promise<AsyncIterator<SchemaValueTree>> {
  if (state.ownership.iterator !== undefined) {
    return state.ownership.iterator
  }
  const pending = acquiring.get(state.ownership)
  if (pending) return pending
  const endpoint = state.endpoint
  if (endpoint === undefined) {
    throw new Error("AgentStream was already transferred")
  }
  state.endpoint = undefined
  const acquisition = (async () => {
    const source =
      endpoint.kind === "native"
        ? endpoint.value
        : await (await import("golem:core/types@2.0.0")).SchemaValueStream.unwrap(endpoint.value)
    state.ownership.iterator = source[Symbol.asyncIterator]()
    return state.ownership.iterator
  })()
  acquiring.set(state.ownership, acquisition)
  return acquisition
}

function asAsyncIterable<T>(source: Iterable<T> | AsyncIterable<T>): AsyncIterable<T> {
  if (Symbol.asyncIterator in Object(source)) {
    return source as AsyncIterable<T>
  }
  return {
    async *[Symbol.asyncIterator]() {
      yield* source as Iterable<T>
    },
  }
}

function iterableFromIterator<T>(iterator: AsyncIterator<T>): AsyncIterable<T> {
  return {
    [Symbol.asyncIterator]: () => iterator,
  }
}

async function* encodeItems<T>(
  source: AsyncIterable<T>,
  itemCodec: ItemCodec<T>,
  encodingContext: Context.Context<any>,
): AsyncIterable<SchemaValueTree> {
  for await (const item of source) {
    const encoded = await Effect.runPromise(
      Schema.encodeEffect(itemCodec)(item).pipe(Effect.provide(encodingContext)),
    )
    yield await schemaValueToWitAsync(encoded)
  }
}

const decodeItem = <T>(
  codec: ItemCodec<T>,
  value: SchemaValueTree,
  context: Context.Context<any>,
): Promise<T> => Effect.runPromise(decodeFromWire(codec, value).pipe(Effect.provide(context)))
