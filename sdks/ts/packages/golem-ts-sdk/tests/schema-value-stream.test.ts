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

import { describe, expect, it } from 'vitest';
import type { SchemaValueStream, SchemaValueTree } from 'golem:core/types@2.0.0';
import { AgentStream, agentStreamFromHandle, agentStreamToHandle } from '../src/schema/agentStream';
import {
  ownSchemaValueStreams,
  withNativeStreamScope,
} from '../src/internal/schema-model/streamScope';
import { throwIfAborted } from '../src/internal/pollableUtils';
import {
  secretHandleToSchemaValue,
  secretHandleFromSchemaValue,
  permissionCardHandleToSchemaValue,
  permissionCardHandleFromSchemaValue,
} from '../src/bridge/schema';
import { withCapabilityAdoptionTransaction } from '../src/internal/schema-model/capabilityTransaction';
import { compileSchema } from '../src/schema/adapter';
import { s } from '../src/schema/markers';
import { Result } from '../src/host/result';
import {
  GuestSchemaValueStreamHandle,
  schemaValueFromWit,
  schemaValueToWitAsync,
  v,
} from '../src/internal/schema-model';

function streamNode(
  tree: SchemaValueTree,
): Extract<SchemaValueTree['valueNodes'][number], { tag: 'stream-value' }> {
  return tree.valueNodes[tree.root] as Extract<
    SchemaValueTree['valueNodes'][number],
    { tag: 'stream-value' }
  >;
}

describe('schema-native AgentStream', () => {
  it('compiles a stream marker with its recursive item schema', () => {
    const codec = compileSchema(s.stream(s.u32()));
    expect(codec.graph.root.body).toMatchObject({
      tag: 'stream',
      element: { body: { tag: 'u32' } },
    });
  });

  it('does not pull a producer while encoding the enclosing value', async () => {
    let pulls = 0;
    const source = AgentStream.from(
      (async function* () {
        pulls += 1;
        yield 11;
      })(),
    );
    const codec = compileSchema(s.stream(s.u32()));

    const tree = await schemaValueToWitAsync(codec.toValue(source));
    expect(pulls).toBe(0);

    const decoded = codec.fromValue(schemaValueFromWit(tree)) as AgentStream<number>;
    expect(await decoded.next()).toEqual({ done: false, value: 11 });
    expect(pulls).toBe(1);
  });

  it('normalizes a source iterator terminal value to undefined', async () => {
    const stream = AgentStream.from({
      [Symbol.asyncIterator]: () => ({
        next: async () => ({ done: true as const, value: 42 }),
      }),
    });

    expect(await stream.next()).toEqual({ done: true, value: undefined });
  });

  it('preflights invalid siblings before moving a stream', async () => {
    const handle = nativeHandle();
    const value = v.tuple([v.stream(handle), v.u32(-1)]);

    await expect(schemaValueToWitAsync(value)).rejects.toThrow('u32 value out of range');
    expect(handle.peek()).toBeDefined();
  });

  it('preflights invalid nested values before moving an outer sibling stream', async () => {
    const handle = nativeHandle();
    const value = v.record([v.stream(handle), v.option(v.list([v.u32(0x1_0000_0000)]))]);

    await expect(schemaValueToWitAsync(value)).rejects.toThrow('u32 value out of range');
    expect(handle.peek()).toBeDefined();
  });

  it('rejects aliased stream handles before moving either sibling', async () => {
    const handle = nativeHandle();
    const value = v.tuple([v.stream(handle), v.stream(handle)]);

    await expect(schemaValueToWitAsync(value)).rejects.toThrow(
      'the same schema value stream appeared more than once',
    );
    expect(handle.peek()).toBeDefined();
  });

  it('rejects a consumed nested stream before moving its sibling', async () => {
    const sibling = nativeHandle();
    const consumed = nativeHandle();
    consumed.take();
    const value = v.tuple([v.stream(sibling), v.option(v.stream(consumed))]);

    await expect(schemaValueToWitAsync(value)).rejects.toThrow(
      'schema value stream was already transferred',
    );
    expect(sibling.peek()).toBeDefined();
  });

  it('disposes resources wrapped before a later sequential wrap fails', async () => {
    let disposed = 0;
    const firstSource = Object.assign(emptyWireStream(), {
      onDispose: () => {
        disposed += 1;
      },
    });
    const failingSource = Object.assign(emptyWireStream(), { failWrap: true });
    const first = new GuestSchemaValueStreamHandle({ kind: 'native', value: firstSource });
    const failing = new GuestSchemaValueStreamHandle({ kind: 'native', value: failingSource });

    await expect(
      schemaValueToWitAsync(v.tuple([v.stream(first), v.stream(failing)])),
    ).rejects.toThrow('test schema value stream wrap failure');
    expect(disposed).toBe(1);
    expect(first.peek()).toBeUndefined();
  });

  it('passes an unread wrapped stream through without replacing it', async () => {
    const raw = { reader: emptyWireStream() } as unknown as SchemaValueStream;
    const codec = compileSchema(s.stream(s.u32()));
    const received = codec.fromValue(
      schemaValueFromWit({ valueNodes: [{ tag: 'stream-value', val: raw }], root: 0 }),
    ) as AgentStream<number>;

    const encoded = await schemaValueToWitAsync(codec.toValue(received));
    expect(streamNode(encoded).val).toBe(raw);
  });

  it('reassociates the same reader after partial consumption', async () => {
    let pulls = 0;
    const itemCodec = compileSchema(s.u32());
    const original = (async function* (): AsyncIterable<SchemaValueTree> {
      for (const value of [1, 2, 3]) {
        pulls += 1;
        yield await schemaValueToWitAsync(itemCodec.toValue(value));
      }
    })();
    const raw = { reader: original } as unknown as SchemaValueStream;
    const streamCodec = compileSchema(s.stream(s.u32()));
    const stream = streamCodec.fromValue(
      schemaValueFromWit({ valueNodes: [{ tag: 'stream-value', val: raw }], root: 0 }),
    ) as AgentStream<number>;

    expect(await stream.next()).toEqual({ done: false, value: 1 });
    expect(pulls).toBe(1);

    const remainderTree = await schemaValueToWitAsync(streamCodec.toValue(stream));
    expect(pulls).toBe(1);
    const remainder = streamCodec.fromValue(
      schemaValueFromWit(remainderTree),
    ) as AgentStream<number>;
    expect(await remainder.next()).toEqual({ done: false, value: 2 });
    expect(await remainder.next()).toEqual({ done: false, value: 3 });
    expect(pulls).toBe(3);
  });

  it('supports streams nested in another schema value', async () => {
    const codec = compileSchema(s.result(s.stream(s.u32()), s.stream(s.u32())));
    const source = AgentStream.from([4, 5]);
    const result = Result.ok(source);
    const tree = await schemaValueToWitAsync(codec.toValue(result));
    const decoded = codec.fromValue(schemaValueFromWit(tree)) as {
      readonly tag: 'ok';
      readonly val: AgentStream<number>;
    };
    expect(await decoded.val.next()).toEqual({ done: false, value: 4 });
    expect(await decoded.val.next()).toEqual({ done: false, value: 5 });
  });

  it('forwards iterator return to cancel an active producer', async () => {
    let pulls = 0;
    let cancelled = false;
    const stream = AgentStream.from(
      (async function* () {
        try {
          pulls += 1;
          yield 1;
        } finally {
          cancelled = true;
        }
      })(),
    );

    expect(await stream.next()).toEqual({ done: false, value: 1 });
    await stream.return();
    expect(pulls).toBe(1);
    expect(cancelled).toBe(true);
  });

  it('remains closed when source cleanup rejects from return', async () => {
    const stream = AgentStream.from({
      [Symbol.asyncIterator]: () => ({
        next: async () => ({ done: false as const, value: 1 }),
        return: async () => {
          throw new Error('cleanup failed');
        },
      }),
    });

    await expect(stream.return()).rejects.toThrow('cleanup failed');
    await expect(stream.next()).rejects.toThrow('AgentStream was already transferred or closed');
  });

  it('remains closed when source iterator initialization rejects from throw', async () => {
    let initializationAttempts = 0;
    const stream = AgentStream.from({
      [Symbol.asyncIterator]: () => {
        initializationAttempts += 1;
        if (initializationAttempts === 1) {
          throw new Error('iterator initialization failed');
        }
        return {
          next: async () => ({ done: false as const, value: 1 }),
        };
      },
    });

    await expect(stream.throw('local failure')).rejects.toThrow('iterator initialization failed');
    await expect(stream.next()).rejects.toThrow('AgentStream was already transferred or closed');
  });
});

async function* emptyWireStream(): AsyncIterable<SchemaValueTree> {}

function nativeHandle(): GuestSchemaValueStreamHandle {
  return new GuestSchemaValueStreamHandle({ kind: 'native', value: emptyWireStream() });
}

describe('native typed conversion ownership', () => {
  const itemCodec = compileSchema(s.u32());
  function producer(cleanupError?: Error) {
    let pulls = 0;
    let closes = 0;
    const stream = AgentStream.from({
      [Symbol.asyncIterator]: () => ({
        next: async () => {
          pulls++;
          return { done: false as const, value: 7 };
        },
        return: async () => {
          closes++;
          if (cleanupError) throw cleanupError;
          return { done: true as const, value: undefined };
        },
      }),
    });
    return { stream, pulls: () => pulls, closes: () => closes };
  }

  it('closes a transferred sibling exactly once without masking typed encoding failure', async () => {
    const first = producer(new Error('cleanup'));
    const original = new Error('later field');
    await expect(
      withNativeStreamScope(() => {
        agentStreamToHandle(first.stream, itemCodec);
        throw original;
      }),
    ).rejects.toBe(original);
    expect(first.closes()).toBe(1);
    expect(first.pulls()).toBe(0);
    await expect(first.stream.next()).rejects.toThrow('transferred');
  });

  it('cleans typed transfers when WIT preflight rejects a later sibling', async () => {
    const first = producer();
    await expect(
      withNativeStreamScope(
        () => v.record([v.stream(agentStreamToHandle(first.stream, itemCodec)), v.u32(-1)]),
        schemaValueToWitAsync,
      ),
    ).rejects.toThrow('u32');
    expect(first.closes()).toBe(1);
    expect(first.pulls()).toBe(0);
  });

  it('rejects aliasing and releases the first transfer', async () => {
    const first = producer();
    await expect(
      withNativeStreamScope(() => [
        agentStreamToHandle(first.stream, itemCodec),
        agentStreamToHandle(first.stream, itemCodec),
      ]),
    ).rejects.toThrow('transferred');
    expect(first.closes()).toBe(1);
  });

  it('releases both decoded and unread siblings after typed decoding fails', async () => {
    const first = producer();
    const second = producer();
    const h1 = agentStreamToHandle(first.stream, itemCodec);
    const h2 = agentStreamToHandle(second.stream, itemCodec);
    const model = v.record([v.stream(h1), v.stream(h2)]);
    const original = new Error('decode');
    await expect(
      withNativeStreamScope(() => {
        ownSchemaValueStreams(model);
        agentStreamFromHandle(h1, itemCodec);
        throw original;
      }),
    ).rejects.toBe(original);
    expect([first.closes(), second.closes()]).toEqual([1, 1]);
    expect([first.pulls(), second.pulls()]).toEqual([0, 0]);
  });

  it('forwards unread endpoints without running item codecs or pulling', async () => {
    const first = producer();
    const initial = agentStreamToHandle(first.stream, itemCodec);
    const endpoint = initial.peek();
    const poison = {
      ...itemCodec,
      toValue: () => {
        throw new Error('encode');
      },
      fromValue: () => {
        throw new Error('decode');
      },
    };
    const forwarded = await withNativeStreamScope(() => {
      const typed = agentStreamFromHandle(initial, poison);
      return agentStreamToHandle(typed, poison);
    });
    expect(forwarded.peek()).toBe(endpoint);
    expect(first.pulls()).toBe(0);
    expect(first.closes()).toBe(0);
    await forwarded.close();
    expect(first.closes()).toBe(1);
  });

  it('cleans nested acquisitions when a lazy item encoder fails', async () => {
    const nested = producer();
    const original = new Error('item encode');
    const outer = agentStreamToHandle(AgentStream.from([nested.stream]), {
      ...itemCodec,
      toValue: (item) => {
        agentStreamToHandle(item as AgentStream<number>, itemCodec);
        throw original;
      },
    });
    const endpoint = outer.take()!;
    if (endpoint.kind !== 'native') throw new Error('expected native');
    const iterator = endpoint.value[Symbol.asyncIterator]();
    await expect(iterator.next()).rejects.toBe(original);
    await iterator.return?.();
    expect(nested.closes()).toBe(1);
    expect(nested.pulls()).toBe(0);
  });

  it.each([
    ['secret', secretHandleToSchemaValue, secretHandleFromSchemaValue],
    ['permission card', permissionCardHandleToSchemaValue, permissionCardHandleFromSchemaValue],
  ] as const)(
    'rolls back %s adoption when a generated lazy item encoder fails',
    async (_, encode, decode) => {
      const raw = { id: 'stream-item-capability' } as never;
      const original = new Error('later item field');
      let closes = 0;
      const source = AgentStream.from(
        (async function* () {
          try {
            yield {
              capability: raw,
              get later(): string {
                throw original;
              },
            };
          } finally {
            closes += 1;
          }
        })(),
      );
      const outer = agentStreamToHandle(source, {
        ...itemCodec,
        toValue: (item) =>
          withCapabilityAdoptionTransaction(() => {
            const record = item as { capability: never; later: string };
            return v.record([encode(record.capability), v.string(record.later)]);
          }),
      });
      expect(closes).toBe(0);
      const endpoint = outer.take()!;
      if (endpoint.kind !== 'native') throw new Error('expected native');
      const iterator = endpoint.value[Symbol.asyncIterator]();
      await expect(iterator.next()).rejects.toBe(original);
      await iterator.return?.();
      expect(closes).toBe(1);
      expect(decode(encode(raw))).toBe(raw);
    },
  );

  it('cleans nested acquisitions when a lazy item decoder fails', async () => {
    const nested = producer();
    const streamCodec = compileSchema(s.stream(s.u32()));
    const outer = agentStreamToHandle(AgentStream.from([nested.stream]), streamCodec);
    const original = new Error('item decode');
    const typed = agentStreamFromHandle(outer, {
      ...streamCodec,
      fromValue: (value) => {
        streamCodec.fromValue(value);
        throw original;
      },
    });
    await expect(typed.next()).rejects.toBe(original);
    await expect(typed.return()).rejects.toThrow('closed');
    expect(nested.closes()).toBe(1);
    expect(nested.pulls()).toBe(0);
  });

  it('closes the outer endpoint when a lazy item decoder fails', async () => {
    let closes = 0;
    const wireItem = await schemaValueToWitAsync(itemCodec.toValue(7));
    const outer = new GuestSchemaValueStreamHandle({
      kind: 'native',
      value: {
        [Symbol.asyncIterator]: () => ({
          next: async () => ({ done: false as const, value: wireItem }),
          return: async () => {
            closes++;
            return { done: true as const, value: undefined };
          },
        }),
      },
    });
    const original = new Error('item decode');
    const typed = agentStreamFromHandle(outer, {
      ...itemCodec,
      fromValue: () => {
        throw original;
      },
    });

    await expect(typed.next()).rejects.toBe(original);
    expect(closes).toBe(1);
    await expect(typed.next()).rejects.toThrow('closed');
  });

  it('checks pre-abort before typed encoding transfers input', async () => {
    const first = producer();
    const signal = AbortSignal.abort(new Error('pre-abort'));
    const invoke = async () => {
      throwIfAborted(signal);
      return withNativeStreamScope(() => agentStreamToHandle(first.stream, itemCodec));
    };
    await expect(invoke()).rejects.toThrow('pre-abort');
    expect(await first.stream.next()).toEqual({ done: false, value: 7 });
    await first.stream.return();
    expect(first.closes()).toBe(1);
  });
});
