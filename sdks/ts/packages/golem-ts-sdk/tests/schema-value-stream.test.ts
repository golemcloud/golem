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
import { AgentStream } from '../src/schema/agentStream';
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
});

async function* emptyWireStream(): AsyncIterable<SchemaValueTree> {}

function nativeHandle(): GuestSchemaValueStreamHandle {
  return new GuestSchemaValueStreamHandle({ kind: 'native', value: emptyWireStream() });
}
