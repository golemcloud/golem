// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.

import type { SchemaValueTree } from "golem:core/types@2.0.0"
import { Context, Effect, Schema, Stream } from "effect"
import { decodeFromWire } from "../WitCodec.js"
import { AbortableStreamIterable } from "./abortableStreamIterable.js"
import { HttpStreamOwner, streamDisposals } from "./ownedStream.js"
import { assertCapabilityReady } from "./schema-model/capabilityTransaction.js"
import {
  GuestSchemaValueStreamHandle,
  type GuestSchemaValueStream,
  type StreamOwnership,
} from "./schema-model/schemaValueStreamHandle.js"
import { STREAM_INTERNAL } from "./schema-model/streamInternal.js"
import { schemaValueToWitAsync } from "./schema-model/wit.js"

type ItemCodec<T> = Schema.Codec<T, import("./schema-model/model.js").SchemaValue, any, any>

export interface DirectItemCodec<T> {
  readonly encode: (value: T) => Effect.Effect<SchemaValueTree, unknown, any>
  readonly decode: (value: SchemaValueTree) => Effect.Effect<T, unknown, any>
}

interface ReceivedState {
  endpoint?: GuestSchemaValueStream
  iterator?: AsyncIterator<SchemaValueTree>
  closed?: Promise<void>
  readonly ownership: StreamOwnership
}

const received = new WeakMap<object, ReceivedState>()
const acquiring = new WeakMap<StreamOwnership, Promise<AsyncIterator<SchemaValueTree>>>()

/** @internal Move a native Effect stream into a recursive schema value. */
export function agentStreamToHandle<T, E, R>(
  stream: Stream.Stream<T, E, R>,
  itemCodec: ItemCodec<T>,
  encodingContext: Context.Context<R> = Context.empty() as Context.Context<R>,
): GuestSchemaValueStreamHandle {
  return directAgentStreamToHandle(
    stream,
    {
      decode: (tree) => decodeFromWire(itemCodec, tree),
      encode: (item) =>
        Schema.encodeEffect(itemCodec)(item).pipe(
          Effect.flatMap((value) =>
            Effect.tryPromise({
              try: (signal) => schemaValueToWitAsync(value, signal),
              catch: (error) => error,
            }),
          ),
        ),
    },
    encodingContext,
  )
}

/** @internal Move a stream using a compiler-emitted concrete item codec. */
export function directAgentStreamToHandle<T, E, R>(
  stream: Stream.Stream<T, E, R>,
  itemCodec: DirectItemCodec<T>,
  context: Context.Context<R> = Context.empty() as Context.Context<R>,
): GuestSchemaValueStreamHandle {
  if (!Stream.isStream(stream)) throw new Error("expected an Effect Stream")
  const state = received.get(stream)
  if (state) {
    assertTransferable(state)
    const endpoint = state.ownership.iterator
      ? { kind: "native" as const, value: iterableFromIterator(state.ownership.iterator) }
      : state.endpoint
    if (!endpoint) throw new Error("schema stream was already transferred")
    return new GuestSchemaValueStreamHandle(
      STREAM_INTERNAL,
      endpoint,
      undefined,
      undefined,
      state.ownership,
    )
  }
  const encoded = stream.pipe(
    Stream.mapEffect((item) => itemCodec.encode(item).pipe(Effect.provide(context))),
  )
  return new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
    kind: "native",
    value: new AbortableStreamIterable(encoded, context, streamDisposals.get(stream)),
  })
}

/** Dispose an unused received endpoint without polling any body item. */
export async function disposeAgentStream(stream: object): Promise<void> {
  const state = received.get(stream)
  if (state?.ownership.available && state.ownership.reservation === undefined) {
    await new ReceivedStreamIterable(state).return()
  }
}

/** @internal Lift a recursive schema-value-stream handle into a native Effect stream. */
export function agentStreamFromHandle<T>(
  handle: GuestSchemaValueStreamHandle,
  itemCodec: ItemCodec<T>,
  decodingContext: Context.Context<any> = Context.empty() as Context.Context<any>,
): Stream.Stream<T, unknown> {
  return directAgentStreamFromHandle(
    handle,
    {
      decode: (tree) => decodeFromWire(itemCodec, tree),
      encode: (item) =>
        Schema.encodeEffect(itemCodec)(item).pipe(
          Effect.flatMap((value) =>
            Effect.tryPromise({
              try: (signal) => schemaValueToWitAsync(value, signal),
              catch: (error) => error,
            }),
          ),
        ),
    },
    decodingContext,
  )
}

/** @internal Lift a wire stream using a compiler-emitted concrete item codec. */
export function directAgentStreamFromHandle<T>(
  handle: GuestSchemaValueStreamHandle,
  itemCodec: DirectItemCodec<T>,
  context: Context.Context<any> = Context.empty() as Context.Context<any>,
): Stream.Stream<T, unknown> {
  const endpoint = handle.peek()
  if (!endpoint) throw new Error("schema value stream was already transferred")
  const state: ReceivedState = { endpoint, ownership: handle.ownership(STREAM_INTERNAL) }
  const stream = Stream.unwrap(
    Effect.acquireRelease(
      Effect.try({
        try: () => {
          assertTransferable(state)
          state.ownership.reader = {}
          return new ReceivedStreamIterable(state)
        },
        catch: (error) => error,
      }),
      (iterable) => Effect.promise(() => iterable.return()).pipe(Effect.asVoid),
    ).pipe(
      Effect.map((iterable) =>
        Stream.unfold(undefined, () =>
          Effect.tryPromise({
            try: (signal) =>
              new Promise<IteratorResult<SchemaValueTree>>((resolve, reject) => {
                signal.addEventListener("abort", () => resolve({ done: true, value: undefined }), {
                  once: true,
                })
                iterable.next().then(resolve, reject)
              }),
            catch: (error) => error,
          }).pipe(
            Effect.flatMap((item) =>
              item.done
                ? Effect.succeed(undefined)
                : itemCodec.decode(item.value).pipe(
                    Effect.provide(context),
                    Effect.map((value) => [value, undefined] as const),
                  ),
            ),
          ),
        ),
      ),
    ),
  )
  received.set(stream, state)
  Context.get(context, HttpStreamOwner)?.add(() => disposeAgentStream(stream))
  return stream
}

class ReceivedStreamIterable implements AsyncIterableIterator<SchemaValueTree> {
  constructor(private readonly state: ReceivedState) {}

  [Symbol.asyncIterator](): AsyncIterableIterator<SchemaValueTree> {
    return this
  }

  async next(): Promise<IteratorResult<SchemaValueTree>> {
    const state = this.available()
    if (state.ownership.busy) throw new Error("a schema stream operation is already in progress")
    state.ownership.busy = true
    try {
      const item = await (await wireIterator(state)).next()
      if (item.done) {
        state.ownership.available = false
        state.closed ??= Promise.resolve()
        return { done: true, value: undefined }
      }
      return item
    } finally {
      state.ownership.busy = false
    }
  }

  return(): Promise<IteratorResult<SchemaValueTree>> {
    const state = this.state
    if (!state.closed) {
      this.available()
      state.ownership.available = false
      state.closed = Promise.resolve().then(async () => {
        const iterator = await wireIterator(state)
        await iterator.return?.()
      })
    }
    return state.closed.then(() => ({ done: true, value: undefined }))
  }

  private available(): ReceivedState {
    assertCapabilityReady(this.state.ownership)
    if (!this.state.ownership.available)
      throw new Error("schema stream was already transferred or closed")
    if (this.state.ownership.reservation !== undefined)
      throw new Error("schema stream is reserved for transfer")
    return this.state
  }
}

function assertTransferable(state: ReceivedState): void {
  assertCapabilityReady(state.ownership)
  if (state.ownership.reader) throw new Error("schema stream already has a reader or is closed")
  if (state.ownership.busy) throw new Error("cannot transfer a schema stream while reading")
  if (state.ownership.reservation !== undefined || !state.ownership.available)
    throw new Error("schema stream was already transferred or reserved")
}

async function wireIterator(state: ReceivedState): Promise<AsyncIterator<SchemaValueTree>> {
  if (state.ownership.iterator) return state.ownership.iterator
  const pending = acquiring.get(state.ownership)
  if (pending) return pending
  const endpoint = state.endpoint
  if (!endpoint) throw new Error("schema stream was already transferred")
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

function iterableFromIterator<T>(iterator: AsyncIterator<T>): AsyncIterable<T> {
  return { [Symbol.asyncIterator]: () => iterator }
}
