import {
  SchemaValueStream,
  type SchemaValueTree,
  type StreamOutcome,
} from 'golem:core/types@2.0.0';
import { ownStream, withNativeStreamScope } from './streamScope';
import { StreamError } from './errors';
import { t, v } from './model';
import { typedSchemaValueFromWit, typedSchemaValueToWitAsync } from './wit';

/** Pair a native producer with an explicit outcome; a rejected pull is not item EOF. */
export async function wrapSchemaValueStream(
  source: AsyncIterable<SchemaValueTree>,
): Promise<SchemaValueStream> {
  let resolve!: (outcome: StreamOutcome | undefined) => void;
  const outcome = new Promise<StreamOutcome | undefined>((done) => {
    resolve = done;
  });
  const items = {
    [Symbol.asyncIterator]() {
      let iterator: AsyncIterator<SchemaValueTree> | undefined;
      let closed = false;
      return {
        async next(): Promise<IteratorResult<SchemaValueTree>> {
          if (closed) return { done: true, value: undefined };
          try {
            iterator ??= source[Symbol.asyncIterator]();
            const item = await iterator.next();
            if (!item.done) return item;
            closed = true;
            resolve({ tag: 'ok', val: undefined });
          } catch (error) {
            closed = true;
            const failure =
              error instanceof StreamError
                ? error.failure
                : {
                    tag: 'failed' as const,
                    val: {
                      graph: { defs: new Map(), root: t.string() },
                      value: v.string(error instanceof Error ? error.message : String(error)),
                    },
                  };
            resolve({
              tag: 'err',
              val:
                failure.tag === 'failed'
                  ? { tag: 'failed', val: await typedSchemaValueToWitAsync(failure.val) }
                  : failure,
            });
          }
          return { done: true, value: undefined };
        },
        async return(): Promise<IteratorResult<SchemaValueTree>> {
          if (!closed) {
            closed = true;
            resolve(undefined);
            iterator ??= source[Symbol.asyncIterator]();
            await iterator.return?.();
          }
          return { done: true, value: undefined };
        },
      };
    },
  };
  return SchemaValueStream.wrap(items, outcome);
}

/** Consume the outcome only after all items, without awaiting it on reader cancellation. */
export async function unwrapSchemaValueStream(
  value: SchemaValueStream,
): Promise<AsyncIterable<SchemaValueTree>> {
  const [items, outcome] = await SchemaValueStream.unwrap(value);
  return {
    [Symbol.asyncIterator]() {
      const iterator = items[Symbol.asyncIterator]();
      let closed = false;
      return {
        async next(): Promise<IteratorResult<SchemaValueTree>> {
          if (closed) return { done: true, value: undefined };
          const item = await iterator.next();
          if (!item.done) return item;
          closed = true;
          const result = await outcome;
          if (result.tag === 'err') {
            const failure = result.val;
            throw new StreamError(
              failure.tag === 'failed'
                ? {
                    tag: 'failed',
                    val: await withNativeStreamScope(() => typedSchemaValueFromWit(failure.val)),
                  }
                : failure,
            );
          }
          return { done: true, value: undefined };
        },
        async return(): Promise<IteratorResult<SchemaValueTree>> {
          closed = true;
          await iterator.return?.();
          return { done: true, value: undefined };
        },
      };
    },
  };
}

export type GuestSchemaValueStream =
  | { kind: 'wrapped'; value: SchemaValueStream }
  | { kind: 'native'; value: AsyncIterable<SchemaValueTree> };

export class GuestSchemaValueStreamHandle {
  #value: GuestSchemaValueStream | undefined;

  constructor(value: GuestSchemaValueStream) {
    this.#value = value;
    ownStream(() => this.close());
  }

  peek(): GuestSchemaValueStream | undefined {
    return this.#value;
  }

  async close(): Promise<void> {
    const endpoint = this.take();
    if (endpoint?.kind === 'wrapped') {
      const source = await unwrapSchemaValueStream(endpoint.value);
      await source[Symbol.asyncIterator]().return?.();
    } else if (endpoint?.kind === 'native') {
      await endpoint.value[Symbol.asyncIterator]().return?.();
    }
  }

  take(): GuestSchemaValueStream | undefined {
    const value = this.#value;
    this.#value = undefined;
    return value;
  }
}
