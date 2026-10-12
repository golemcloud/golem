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

// Unstructured method outputs nested under `option<_>` and `result<_, E>`: the
// output schema keeps the role on the nested variant, the guest encodes each
// case, and a remote client decodes it back.

import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import { WasmRpc } from 'golem:agent/host@2.0.0';
import type { SchemaGraph as WitSchemaGraph } from 'golem:core/types@2.0.0';
import { defineAgentClient } from '../src/client';
import { defineAgent } from '../src/defineAgent';
import { method } from '../src/method';
import { s, type BinaryReferenceValue, type TextReferenceValue } from '../src/schema/markers';
import { compileSchema } from '../src/schema/adapter';
import { SchemaRef } from '../src/schema/ref';
import { Result } from '../src/host/result';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import {
  schemaGraphRootsFromWit,
  schemaValueFromWit,
  schemaValueToWit,
  v,
} from '../src/internal/schema-model';

const Pending = z.object({ tag: z.literal('pending'), retryAfter: z.number() });
type Pending = z.infer<typeof Pending>;

const returns = {
  binary: s.result(s.unstructuredBinary({ mimeTypes: ['image/png'] }), Pending),
  text: s.result(s.unstructuredText(), Pending),
  maybe: s.option(s.unstructuredBinary()),
  lookup: s.result(s.option(s.unstructuredBinary()), Pending),
};

const methods = {
  binary: method({ input: { id: z.string() }, returns: returns.binary }),
  text: method({ input: { id: z.string() }, returns: returns.text }),
  maybe: method({ input: { id: z.string() }, returns: returns.maybe }),
  lookup: method({ input: { id: z.string() }, returns: returns.lookup }),
};

const Raw = defineAgent({ name: 'UnstructuredOutputs', id: {}, methods });

const png: BinaryReferenceValue = {
  tag: 'inline',
  val: new Uint8Array([137, 80, 78, 71]),
  mimeType: 'image/png',
};
const link: BinaryReferenceValue = { tag: 'url', val: 'https://example.com/a.png' };
const ready: TextReferenceValue = { tag: 'inline', val: 'ready', languageCode: 'en' };
const pending: Pending = { tag: 'pending', retryAfter: 5 };

// Each method selects its output by the `id` input so one implementation covers every case.
const outputs: Record<string, Record<string, unknown>> = {
  binary: { inline: Result.ok(png), url: Result.ok(link), pending: Result.err(pending) },
  text: { inline: Result.ok(ready), pending: Result.err(pending) },
  maybe: { none: undefined, inline: png, url: link },
  lookup: {
    none: Result.ok(undefined),
    inline: Result.ok(png),
    url: Result.ok(link),
    pending: Result.err(pending),
  },
};

Raw.implement({
  init: () => ({}),
  methods: {
    binary: ({ id }) => outputs.binary[id] as never,
    text: ({ id }) => outputs.text[id] as never,
    maybe: ({ id }) => outputs.maybe[id] as never,
    lookup: ({ id }) => outputs.lookup[id] as never,
  },
});

const agentType = () => AgentTypeRegistry.get(new AgentClassName('UnstructuredOutputs'))!;

const outputType = (name: string) => {
  const type = agentType();
  const output = type.methods.find((m) => m.name === name)!.outputSchema;
  if (output.tag !== 'single') throw new Error(`${name} has a unit output`);
  return { graph: type.schema as WitSchemaGraph, root: output.val };
};

const guestInvoke = async (name: string, id: string) => {
  const idValue = v.record([]);
  (globalThis as any).currentAgentId =
    `UnstructuredOutputs(${JSON.stringify(schemaValueToWit(idValue))})`;
  const initiated = await AgentInitiatorRegistry.lookup('UnstructuredOutputs')!.initiate(
    schemaValueToWit(idValue),
    { tag: 'anonymous' },
  );
  if (initiated.tag !== 'ok') throw new Error('initiate failed');
  const result = await initiated.val.invoke(name, schemaValueToWit(v.record([v.string(id)])), {
    tag: 'anonymous',
  });
  if (result.tag !== 'ok') throw new Error(`invoke failed: ${JSON.stringify(result.val)}`);
  return result.val!;
};

describe('unstructured outputs under option and result', () => {
  it('keeps the unstructured role on the nested output node', () => {
    const node = (graph: WitSchemaGraph, index: number) => graph.typeNodes[index]!;

    const binary = outputType('binary');
    const binaryRoot = node(binary.graph, binary.root);
    if (binaryRoot.body.tag !== 'result-type') throw new Error(binaryRoot.body.tag);
    expect(binaryRoot.metadata.role).toBeUndefined();
    const binaryOk = node(binary.graph, binaryRoot.body.val.ok!);
    expect(binaryOk.metadata.role).toEqual({ tag: 'unstructured-binary' });
    expect(binaryOk.body.tag).toBe('variant-type');
    expect(node(binary.graph, binaryRoot.body.val.err!).body.tag).toBe('record-type');

    const text = outputType('text');
    const textRoot = node(text.graph, text.root);
    if (textRoot.body.tag !== 'result-type') throw new Error(textRoot.body.tag);
    expect(node(text.graph, textRoot.body.val.ok!).metadata.role).toEqual({
      tag: 'unstructured-text',
    });

    const maybe = outputType('maybe');
    const maybeRoot = node(maybe.graph, maybe.root);
    if (maybeRoot.body.tag !== 'option-type') throw new Error(maybeRoot.body.tag);
    expect(maybeRoot.metadata.role).toBeUndefined();
    expect(node(maybe.graph, maybeRoot.body.val).metadata.role).toEqual({
      tag: 'unstructured-binary',
    });

    const lookup = outputType('lookup');
    const lookupRoot = node(lookup.graph, lookup.root);
    if (lookupRoot.body.tag !== 'result-type') throw new Error(lookupRoot.body.tag);
    const lookupOk = node(lookup.graph, lookupRoot.body.val.ok!);
    if (lookupOk.body.tag !== 'option-type') throw new Error(lookupOk.body.tag);
    expect(lookupOk.metadata.role).toBeUndefined();
    expect(node(lookup.graph, lookupOk.body.val).metadata.role).toEqual({
      tag: 'unstructured-binary',
    });
    expect(node(lookup.graph, lookupRoot.body.val.err!).body.tag).toBe('record-type');
  });

  it('round-trips every case through the guest and a remote client', async () => {
    const client = defineAgentClient({ name: 'UnstructuredOutputs', id: {}, methods }).client.get(
      {},
    );
    const rpc = vi.mocked(WasmRpc.create).mock.results.at(-1)!.value;
    for (const [name, cases] of Object.entries(outputs)) {
      const codec = compileSchema(returns[name as keyof typeof returns]);
      for (const [id, expected] of Object.entries(cases)) {
        const tree = await guestInvoke(name, id);
        const value = schemaValueFromWit(tree);
        expect(codec.fromValue(value)).toEqual(expected);

        const { graph, root } = outputType(name);
        const decoded = schemaGraphRootsFromWit(graph, [root]);
        const ref = new SchemaRef({ defs: decoded.defs, root: decoded.roots[0]! });
        expect(ref.validateValue(value)).toMatchObject({ success: true });

        rpc.asyncInvokeAndAwait.mockReturnValue({
          metadata: { agentId: 'unstructured', idempotencyKey: 'key' },
          future: { get: vi.fn().mockResolvedValue(tree), cancel: vi.fn() },
        });
        const remote = await (client as any)[name]({ id });
        expect(remote).toEqual(expected);
      }
    }
  });

  it('renders none and the typed error as canonical JSON', async () => {
    const json = async (name: string, id: string) => {
      const { graph, root } = outputType(name);
      const decoded = schemaGraphRootsFromWit(graph, [root]);
      const ref = new SchemaRef({ defs: decoded.defs, root: decoded.roots[0]! });
      return ref.unpackJson(schemaValueFromWit(await guestInvoke(name, id)));
    };
    expect(await json('maybe', 'none')).toBeNull();
    expect(await json('lookup', 'none')).toEqual({ ok: null });
    expect(await json('lookup', 'pending')).toEqual({ err: pending });
  });

  it('rejects an optional element value outside the declared restrictions', () => {
    const codec = compileSchema(
      s.result(s.option(s.unstructuredBinary({ mimeTypes: ['image/png'] })), Pending),
    );
    expect(() =>
      codec.fromValue(
        v.ok(
          v.option(
            v.variant(0, { tag: 'binary', bytes: new Uint8Array([0]), mimeType: 'text/html' }),
          ),
        ),
      ),
    ).toThrow(/Mime type/);
  });
});
