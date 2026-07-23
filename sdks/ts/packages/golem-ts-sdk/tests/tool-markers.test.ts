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

import { type as arkType } from 'arktype';
import * as z3 from 'zod3';
import * as z4 from 'zod/v4';
import { describe, expect, it } from 'vitest';
import { Bytes, KeyValue, Path, Quantity, s } from '../src/fluent/schema/markers';
import { compileSchema } from '../src/fluent/schema/adapter';
import { getExtendedToolDefinition, toolDefinition } from '../src/fluent/tool';
import { encodeTool } from '../src/internal/tool';

describe('tool schema markers', () => {
  it('uses the Rust path defaults and preserves path restrictions', () => {
    const defaults = compileSchema(Path());
    expect(defaults.graph.root.body).toEqual({
      tag: 'path',
      spec: {
        direction: 'in-out',
        kind: 'any',
        allowedMimeTypes: undefined,
        allowedExtensions: undefined,
      },
    });
    expect(defaults.fromValue(defaults.toValue('/tmp/input'))).toBe('/tmp/input');

    const restricted = compileSchema(
      Path({
        direction: 'input',
        kind: 'file',
        allowedMimeTypes: ['text/plain'],
        allowedExtensions: ['txt'],
      }),
    );
    expect(restricted.graph.root.body).toMatchObject({
      tag: 'path',
      spec: {
        direction: 'input',
        kind: 'file',
        allowedMimeTypes: ['text/plain'],
        allowedExtensions: ['txt'],
      },
    });
  });

  it('preserves quantity units and round-trips the idiomatic fixed-point value', () => {
    const codec = compileSchema(Quantity({ baseUnit: 'B', allowedSuffixes: ['B', 'KiB', 'MiB'] }));
    const value = { mantissa: 4n, scale: 0, unit: 'KiB' };
    expect(codec.graph.root.body).toMatchObject({
      tag: 'quantity',
      spec: { baseUnit: 'B', allowedSuffixes: ['B', 'KiB', 'MiB'] },
    });
    expect(codec.fromValue(codec.toValue(value))).toEqual(value);
  });

  it('composes KeyValue with Zod 3.24, Zod 4, and ArkType schemas', () => {
    const schemas = [z3.number(), z4.number(), arkType('number')];
    for (const schema of schemas) {
      const codec = compileSchema(KeyValue(schema));
      expect(codec.graph.root.body.tag).toBe('map');
      const value = new Map([
        ['first', 1],
        ['second', 2],
      ]);
      expect(codec.fromValue(codec.toValue(value))).toEqual(value);
    }
  });

  it.each([
    ['Zod 3.24', z3.object({ message: z3.string(), count: z3.number() })],
    ['Zod 4', z4.object({ message: z4.string(), count: z4.number() })],
    ['ArkType 2', arkType({ message: 'string', count: 'number' })],
  ])('compiles a complete tool definition with %s', (_vendor, schema) => {
    const definition = toolDefinition('vendor-tool').body((body) =>
      body.positional('payload', schema).returns(schema),
    );
    const model = getExtendedToolDefinition(definition);
    const root = model.commandByPath([]);
    if (!root) throw new Error('vendor tool root was not compiled');
    const input = model.canonicalInputModel(root);
    const payload = { message: 'hello', count: 2 };

    expect(encodeTool(model).commands.nodes[0].body?.positionals.fixed).toHaveLength(1);
    expect(input.decode(input.encode({ payload }))).toEqual({ payload });
    expect(root.body?.result?.codec.fromValue(root.body.result.codec.toValue(payload))).toEqual(
      payload,
    );
  });

  it('keeps tool Bytes distinct from the list<u8> bytes marker', () => {
    const binary = compileSchema(Bytes({ minBytes: 1, maxBytes: 4 }));
    const byteList = compileSchema(s.bytes());
    expect(binary.graph.root.body).toEqual({
      tag: 'binary',
      restrictions: { mimeTypes: undefined, minBytes: 1, maxBytes: 4 },
    });
    expect(byteList.graph.root.body.tag).toBe('list');

    const value = new Uint8Array([1, 2, 3]);
    expect(binary.fromValue(binary.toValue(value))).toEqual(value);
    expect(byteList.fromValue(byteList.toValue(value))).toEqual(value);
  });

  it('enforces Path extension restrictions through its Standard Schema validator', () => {
    const result = Path({ allowedExtensions: ['txt'] })['~standard'].validate('report.rs');

    expect(result).toHaveProperty('issues');
  });

  it('enforces Quantity bounds through its Standard Schema validator', () => {
    const result = Quantity({
      baseUnit: 'B',
      min: { mantissa: 10n, scale: 0, unit: 'B' },
      max: { mantissa: 20n, scale: 0, unit: 'B' },
    })['~standard'].validate({ mantissa: 5n, scale: 0, unit: 'B' });

    expect(result).toHaveProperty('issues');
  });

  it('rejects quantity comparisons that overflow the canonical validator', () => {
    const result = Quantity({
      baseUnit: 'B',
      min: { mantissa: -2n, scale: -38, unit: 'B' },
    })['~standard'].validate({ mantissa: 0n, scale: 0, unit: 'B' });

    expect(result).toHaveProperty('issues');
  });

  it('validates KeyValue entries with the supplied key and value schemas', async () => {
    const marker = KeyValue(z4.number(), { keySchema: z4.number() });

    expect(await marker['~standard'].validate(new Map([[1, 'not-a-number']]))).toHaveProperty(
      'issues',
    );
    expect(await marker['~standard'].validate(new Map([['not-a-number', 1]]))).toHaveProperty(
      'issues',
    );
  });

  it('returns transformed child outputs from KeyValue validation', async () => {
    const marker = KeyValue(z4.coerce.number());

    const result = await marker['~standard'].validate(new Map([['count', '42']]));

    expect(result).toEqual({ value: new Map([['count', 42]]) });
  });
});
