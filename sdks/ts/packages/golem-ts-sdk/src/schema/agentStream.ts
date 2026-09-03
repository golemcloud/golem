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

import { SchemaValueStream, type SchemaValueTree } from 'golem:core/types@2.0.0';
import {
  GuestSchemaValueStreamHandle,
  type GuestSchemaValueStream,
  schemaValueFromWit,
  schemaValueToWitAsync,
} from '../internal/schema-model';
import type { SchemaCodec } from './codec';

interface TypedStreamState<T> {
  readonly kind: 'typed';
  readonly source: AsyncIterable<T>;
  iterator?: AsyncIterator<T>;
  busy: boolean;
}

interface WireStreamState {
  readonly kind: 'wire';
  endpoint?: GuestSchemaValueStream;
  iterator?: AsyncIterator<SchemaValueTree>;
  readonly itemCodec: SchemaCodec;
  busy: boolean;
}

type AgentStreamState<T> = TypedStreamState<T> | WireStreamState;

const states = new WeakMap<object, AgentStreamState<unknown>>();

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
      kind: 'typed',
      source: asAsyncIterable(source),
      busy: false,
    });
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return this;
  }

  /** Read the next item, or `{ done: true, value: undefined }` after clean completion. */
  async next(): Promise<IteratorResult<T>> {
    const state = streamState(this);
    if (state.busy) {
      throw new Error('an AgentStream operation is already in progress');
    }
    state.busy = true;
    try {
      if (state.kind === 'typed') {
        state.iterator ??= state.source[Symbol.asyncIterator]();
        const item = await state.iterator.next();
        return item.done ? { done: true, value: undefined } : item;
      }

      const iterator = await wireIterator(state);
      const item = await iterator.next();
      return item.done
        ? { done: true, value: undefined }
        : {
            done: false,
            value: state.itemCodec.fromValue(schemaValueFromWit(item.value)) as T,
          };
    } finally {
      state.busy = false;
    }
  }

  /**
   * Close the stream and its underlying iterator. `for await` invokes this automatically on an
   * early `break` or `return`.
   */
  async return(value?: unknown): Promise<IteratorResult<T>> {
    const state = streamState(this);
    if (state.busy) {
      throw new Error('an AgentStream operation is already in progress');
    }
    state.busy = true;
    try {
      states.delete(this);
      const iterator =
        state.kind === 'typed'
          ? (state.iterator ??= state.source[Symbol.asyncIterator]())
          : await wireIterator(state);
      if (iterator.return) {
        await iterator.return(value);
      }
      return { done: true, value: value as T };
    } finally {
      state.busy = false;
    }
  }

  /**
   * Fail local iteration and close a connected readable endpoint.
   *
   * Connected P3 streams reject with the same local reason, but do not transfer that reason to the
   * producer. Streams created with {@link AgentStream.from} delegate to the source iterator's
   * `throw()` when it provides one, but the `AgentStream` is consumed regardless of its result.
   */
  async throw(error?: unknown): Promise<IteratorResult<T>> {
    const state = streamState(this);
    if (state.busy) {
      throw new Error('an AgentStream operation is already in progress');
    }
    state.busy = true;
    try {
      states.delete(this);
      const iterator =
        state.kind === 'typed'
          ? (state.iterator ??= state.source[Symbol.asyncIterator]())
          : await wireIterator(state);
      if (iterator.throw) {
        return (await iterator.throw(error)) as IteratorResult<T>;
      }
      if (iterator.return) {
        await iterator.return();
      }
      throw error;
    } finally {
      state.busy = false;
    }
  }
}

/** @internal Move an AgentStream into a recursive schema value. */
export function agentStreamToHandle<T>(
  stream: AgentStream<T>,
  itemCodec: SchemaCodec,
): GuestSchemaValueStreamHandle {
  const state = streamState(stream);
  if (state.busy) {
    throw new Error('cannot transfer an AgentStream while an operation is in progress');
  }
  states.delete(stream);

  if (state.kind === 'wire') {
    if (state.endpoint !== undefined) {
      return new GuestSchemaValueStreamHandle(state.endpoint);
    }
    if (state.iterator !== undefined) {
      return new GuestSchemaValueStreamHandle({
        kind: 'native',
        value: iterableFromIterator(state.iterator),
      });
    }
    throw new Error('AgentStream was already transferred');
  }

  const source = state.iterator === undefined ? state.source : iterableFromIterator(state.iterator);
  return new GuestSchemaValueStreamHandle({
    kind: 'native',
    value: encodeItems(source, itemCodec),
  });
}

/** @internal Lift a recursive schema-value-stream handle into an AgentStream. */
export function agentStreamFromHandle<T>(
  handle: GuestSchemaValueStreamHandle,
  itemCodec: SchemaCodec,
): AgentStream<T> {
  const endpoint = handle.take();
  if (endpoint === undefined) {
    throw new Error('schema value stream was already transferred');
  }
  return createAgentStream({
    kind: 'wire',
    endpoint,
    itemCodec,
    busy: false,
  });
}

function createAgentStream<T>(state: AgentStreamState<T>): AgentStream<T> {
  const stream = Object.create(AgentStream.prototype) as AgentStream<T>;
  states.set(stream, state as AgentStreamState<unknown>);
  return stream;
}

function streamState<T>(stream: AgentStream<T>): AgentStreamState<T> {
  const state = states.get(stream) as AgentStreamState<T> | undefined;
  if (state === undefined) {
    throw new Error('AgentStream was already transferred or closed');
  }
  return state;
}

async function wireIterator(state: WireStreamState): Promise<AsyncIterator<SchemaValueTree>> {
  if (state.iterator !== undefined) {
    return state.iterator;
  }
  const endpoint = state.endpoint;
  if (endpoint === undefined) {
    throw new Error('AgentStream was already transferred');
  }
  state.endpoint = undefined;
  const source =
    endpoint.kind === 'native' ? endpoint.value : await SchemaValueStream.unwrap(endpoint.value);
  state.iterator = source[Symbol.asyncIterator]();
  return state.iterator;
}

function asAsyncIterable<T>(source: Iterable<T> | AsyncIterable<T>): AsyncIterable<T> {
  if (Symbol.asyncIterator in Object(source)) {
    return source as AsyncIterable<T>;
  }
  return {
    async *[Symbol.asyncIterator]() {
      yield* source as Iterable<T>;
    },
  };
}

function iterableFromIterator<T>(iterator: AsyncIterator<T>): AsyncIterable<T> {
  return {
    [Symbol.asyncIterator]: () => iterator,
  };
}

async function* encodeItems<T>(
  source: AsyncIterable<T>,
  itemCodec: SchemaCodec,
): AsyncIterable<SchemaValueTree> {
  for await (const item of source) {
    yield await schemaValueToWitAsync(itemCodec.toValue(item));
  }
}
