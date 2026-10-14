// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

import { describe, expect, it } from 'vitest';
import { z } from 'zod';
import { Result } from '../src/host/result';
import { Uuid } from '../src/uuid';
import { s, Path, Quantity } from '../src/schema/markers';
import { encodeToolValue } from '../src/internal/tool/invocationResult';
import {
  SchemaValueWriter,
  invocationSchemaValueReader,
  schemaValueIsCanonical,
  withDirectCodec,
} from '../src/schema/codec';
import { schemaValueFromWit, schemaValueToWit } from '../src/internal/schema-model';
import {
  compileSchema,
  directSchemaValueFromWit,
  directSchemaValueToWit,
} from '../src/schema/public';

describe('direct flat-wire schema codecs', () => {
  it('rejects nonliteral primitive payloads during strict direct decoding', () => {
    const cases = [
      [z.literal('fixed'), { tag: 'string', value: 'wrong' }],
      [z.literal(17), { tag: 'f64', value: 31 }],
      [z.literal(true), { tag: 'bool', value: false }],
      [z.literal(17n), { tag: 'u64', value: 31n }],
    ] as const;
    for (const [schema, value] of cases) {
      const codec = compileSchema(z.tuple([schema]));
      const wire = schemaValueToWit({ tag: 'tuple', elements: [value] });
      expect(() => directSchemaValueFromWit(codec, wire)).toThrow(/canonical/);
      expect(codec.invocationDirect!.read(invocationSchemaValueReader(wire), wire.root)).toEqual(
        codec.fromValue(schemaValueFromWit(wire)),
      );
    }
  });

  it('matches ordinary tool-result canonicality for primitive literals in tuples and maps', () => {
    const cases = [
      [z.tuple([z.literal('fixed')]), ['fixed'], ['wrong']],
      [z.tuple([z.literal(17)]), [17], [31]],
      [z.tuple([z.literal(true)]), [true], [false]],
      [z.tuple([z.literal(17n)]), [17n], [31n]],
      [
        z.map(z.literal('key'), z.literal('fixed')),
        new Map([['key', 'fixed']]),
        new Map([['wrong', 'fixed']]),
      ],
      [
        z.map(z.literal('key'), z.literal('fixed')),
        new Map([['key', 'fixed']]),
        new Map([['key', 'wrong']]),
      ],
      [z.record(z.literal('key'), z.literal(17)), { key: 17 }, { key: 31 }],
      [
        z.tuple([z.object({ items: z.array(z.tuple([z.literal('fixed')])) })]),
        [{ items: [['fixed']] }],
        [{ items: [['wrong']] }],
      ],
    ] as const;
    for (const [schema, valid, invalid] of cases) {
      const codec = compileSchema(schema);
      const normal = { ...codec, direct: undefined };
      expect(codec.direct).toBeDefined();
      expect(encodeToolValue(codec, valid, 'return')).toEqual(
        encodeToolValue(normal, valid, 'return'),
      );
      expect(directSchemaValueFromWit(codec, directSchemaValueToWit(codec, valid))).toEqual(valid);
      for (const candidate of [codec, normal]) {
        expect(() => encodeToolValue(candidate, invalid, 'return')).toThrow(
          expect.objectContaining({ tag: 'invalid-result' }),
        );
      }
      const writer = new SchemaValueWriter();
      const root = codec.invocationDirect!.write(invalid, writer)!;
      const ordinary = schemaValueToWit(codec.toValue(invalid));
      expect({ valueNodes: writer.valueNodes, root }).toEqual(ordinary);
      expect(codec.invocationDirect!.read(invocationSchemaValueReader(ordinary), root)).toEqual(
        codec.fromValue(schemaValueFromWit(ordinary)),
      );
    }
  });

  it('enforces rich restrictions in tool results and direct decoding, including children', () => {
    const cases = [
      [s.text({ minLength: 2 }), 'a'],
      [s.text({ maxLength: 1 }), 'ab'],
      [s.text({ regex: '^a$' }), 'b'],
      [s.binary({ minBytes: 1 }), new Uint8Array()],
      [s.binary({ maxBytes: 1 }), new Uint8Array(2)],
      [Path({ allowedExtensions: ['txt'] }), 'file.png'],
      [s.url(), 'not a URL'],
      [Quantity({ baseUnit: 'm', allowedSuffixes: [] }), { mantissa: 1n, scale: 0, unit: 's' }],
      [
        Quantity({ baseUnit: 'm', min: { mantissa: 2n, scale: 0, unit: 'm' } }),
        { mantissa: 1n, scale: 0, unit: 'm' },
      ],
      [
        Quantity({ baseUnit: 'm', max: { mantissa: 2n, scale: 0, unit: 'm' } }),
        { mantissa: 3n, scale: 0, unit: 'm' },
      ],
      [z.tuple([s.text({ minLength: 2 })]), ['a']],
      [z.map(z.string(), s.binary({ minBytes: 1 })), new Map([['a', new Uint8Array()]])],
    ] as const;
    for (const [schema, input] of cases) {
      const codec = compileSchema(schema);
      const normal = { ...codec, direct: undefined };
      expect(() => encodeToolValue(normal, input, 'return')).toThrow();
      expect(() => encodeToolValue(codec, input, 'return')).toThrow(
        expect.objectContaining({ tag: 'invalid-result' }),
      );
      const wire = schemaValueToWit(codec.toValue(input));
      expect(() => directSchemaValueFromWit(codec, wire)).toThrow();
      const writer = new SchemaValueWriter();
      expect(() => codec.invocationDirect!.write(input, writer)).not.toThrow();
      expect(codec.invocationDirect!.read(invocationSchemaValueReader(wire), wire.root)).toEqual(
        codec.fromValue(schemaValueFromWit(wire)),
      );
    }
  });

  it('encodes tuples, enums, records and Maps with ordinary invocation semantics', () => {
    const cases = [
      [z.tuple([z.literal('fixed'), s.f32()]), ['ignored', 1.1]],
      [z.tuple([z.string(), z.number()]), ['short']],
      [z.enum(['first', 'second']), 'second'],
      [z.literal(['left', 'right']), 'right'],
      [z.record(z.string(), z.literal('fixed')), { z: 'ignored', a: 'fixed' }],
      [z.map(z.tuple([s.u8(), z.string()]), s.duration()), new Map([[[7, 'key'], 31]])],
    ] as const;
    cases.forEach(([schema, input]) => {
      const codec = compileSchema(schema);
      expect(codec.invocationDirect).toBeDefined();
      const ordinary = schemaValueToWit(codec.toValue(input));
      const writer = new SchemaValueWriter();
      const root = codec.invocationDirect!.write(input, writer)!;
      expect({ valueNodes: writer.valueNodes, root }).toEqual(ordinary);
      expect(codec.invocationDirect!.read(invocationSchemaValueReader(ordinary), root)).toEqual(
        codec.fromValue(schemaValueFromWit(ordinary)),
      );
    });
  });

  it('round-trips structural codecs without converting the root model', () => {
    const cases = [
      [z.tuple([s.u8(), z.enum(['a', 'b'])]), [31, 'b']],
      [z.record(z.string(), z.array(s.u8())), { z: [31], a: [255] }],
      [
        z.map(s.u8(), z.string()),
        new Map([
          [31, 'first'],
          [7, 'second'],
        ]),
      ],
    ] as const;
    cases.forEach(([schema, input]) => {
      const base = compileSchema(schema);
      const ordinary = schemaValueToWit(base.toValue(input));
      const codec = withDirectCodec({
        ...base,
        toValue: () => {
          throw new Error('model encode');
        },
        fromValue: () => {
          throw new Error('model decode');
        },
      });
      expect(directSchemaValueToWit(codec, input)).toEqual(ordinary);
      expect(directSchemaValueFromWit(codec, ordinary)).toEqual(input);
      const writer = new SchemaValueWriter();
      expect(codec.invocationDirect!.write(input, writer)).toBe(ordinary.root);
      expect(
        codec.invocationDirect!.read(invocationSchemaValueReader(ordinary), ordinary.root),
      ).toEqual(input);
    });
  });

  it('rejects lossy strict reads but preserves invocation wire interpretation', () => {
    const cases = [
      [
        z.map(z.string(), z.string()),
        {
          tag: 'map',
          entries: [
            { key: { tag: 'string', value: 'key' }, value: { tag: 'string', value: 'first' } },
            { key: { tag: 'string', value: 'key' }, value: { tag: 'string', value: 'last' } },
          ],
        },
      ],
      [
        z.record(z.string(), z.string()),
        {
          tag: 'map',
          entries: [
            { key: { tag: 'string', value: 'key' }, value: { tag: 'string', value: 'first' } },
            { key: { tag: 'string', value: 'key' }, value: { tag: 'string', value: 'last' } },
          ],
        },
      ],
      [
        z.record(z.string(), z.string()),
        {
          tag: 'map',
          entries: [
            {
              key: { tag: 'string', value: '__proto__' },
              value: { tag: 'string', value: 'ignored' },
            },
          ],
        },
      ],
      [
        z.record(z.string(), z.string()),
        {
          tag: 'map',
          entries: [
            { key: { tag: 'string', value: '2' }, value: { tag: 'string', value: 'two' } },
            { key: { tag: 'string', value: '1' }, value: { tag: 'string', value: 'one' } },
          ],
        },
      ],
      [s.text(), { tag: 'text', text: 'bonjour', language: 'fr' }],
      [s.binary(), { tag: 'binary', bytes: new Uint8Array([17]), mimeType: 'image/png' }],
      [s.uuid(), { tag: 'uuid', value: new Uuid(17n, 31n) }],
      [z.enum(['a']), { tag: 'enum', caseIndex: 1 }],
    ] as const;
    cases.forEach(([schema, value]) => {
      const codec = compileSchema(schema);
      const wire = schemaValueToWit(value);
      const canonical = schemaValueIsCanonical(codec, schemaValueFromWit(wire));
      if (canonical) {
        expect(directSchemaValueFromWit(codec, wire)).toEqual(codec.fromValue(value));
      } else {
        expect(() => directSchemaValueFromWit(codec, wire)).toThrow();
      }
      expect(codec.invocationDirect!.read(invocationSchemaValueReader(wire), wire.root)).toEqual(
        codec.fromValue(schemaValueFromWit(wire)),
      );
    });
  });

  it('encodes rich markers directly without calling model codecs', () => {
    const cases = [
      [s.uuid(), new Uuid(17n, 31n)],
      [s.datetime(), { seconds: -17n, nanoseconds: 999999999 }],
      [s.duration(), -31n],
      [s.url(), 'https://example.com/a?q=b'],
      [Path(), '/tmp/δ'],
      [Quantity({ baseUnit: 'm' }), { mantissa: 123n, scale: -2, unit: 'm' }],
      [s.text(), 'héllo'],
      [s.binary(), new Uint8Array([0, 255, 31])],
    ] as const;
    cases.forEach(([schema, input]) => {
      const base = compileSchema(schema);
      const ordinary = schemaValueToWit(base.toValue(input));
      const codec = withDirectCodec({
        ...base,
        toValue: () => {
          throw new Error('model encode');
        },
        fromValue: () => {
          throw new Error('model decode');
        },
      });
      const wire = directSchemaValueToWit(codec, input);
      expect(wire).toEqual(ordinary);
      expect(directSchemaValueFromWit(codec, wire)).toEqual(
        base.fromValue(schemaValueFromWit(ordinary)),
      );
      const writer = new SchemaValueWriter();
      const root = codec.invocationDirect!.write(input, writer)!;
      expect({ valueNodes: writer.valueNodes, root }).toEqual(ordinary);
      expect(codec.invocationDirect!.read(invocationSchemaValueReader(ordinary), root)).toEqual(
        input,
      );
    });
    const duration = compileSchema(s.duration());
    const writer = new SchemaValueWriter();
    duration.invocationDirect!.write(31, writer);
    expect(writer.valueNodes).toEqual(schemaValueToWit(duration.toValue(31)).valueNodes);
  });

  it('rejects invalid tuple, enum, map and rich-marker values at the wire boundary', () => {
    const cases = [
      [z.tuple([s.u8()]), [256]],
      [z.enum(['a', 'b']), 'missing'],
      [z.map(s.u8(), z.string()), new Map([[256, 'x']])],
      [z.record(z.string(), s.u8()), { x: 256 }],
      [s.uuid(), { highBits: 1n, lowBits: 2n }],
      [s.uuid(), new Uuid(-1n, 0n)],
      [s.datetime(), { seconds: 0n, nanoseconds: 1000000000 }],
      [s.duration(), 1n << 63n],
      [s.url(), 17],
      [Path(), 17],
      [Quantity({ unit: 'm' }), { mantissa: 1n << 63n, scale: 0, unit: 'm' }],
      [s.text(), 17],
      [s.binary(), [0, 255]],
    ] as const;
    cases.forEach(([schema, input]) => {
      const codec = compileSchema(schema);
      expect(() => schemaValueToWit(codec.toValue(input))).toThrow();
      expect(() => codec.invocationDirect!.write(input, new SchemaValueWriter())).toThrow();
      expect(() => directSchemaValueToWit(codec, input)).toThrow();
    });
    expect(() => directSchemaValueToWit(compileSchema(z.tuple([z.string()])), [])).toThrow();
    expect(() =>
      directSchemaValueFromWit(compileSchema(z.enum(['a'])), {
        valueNodes: [{ tag: 'enum-value', val: 1 }],
        root: 0,
      }),
    ).toThrow();
    const tuple = compileSchema(z.tuple([s.u8(), s.u8()]));
    const sparse = [7, 31];
    delete sparse[1];
    expect(() => schemaValueToWit(tuple.toValue(sparse))).toThrow();
    expect(() => tuple.invocationDirect!.write(sparse, new SchemaValueWriter())).toThrow();
    expect(() =>
      directSchemaValueFromWit(compileSchema(s.datetime()), {
        valueNodes: [{ tag: 'datetime-value', val: { seconds: 0n, nanoseconds: -1 } }],
        root: 0,
      }),
    ).toThrow();
    expect(() =>
      directSchemaValueFromWit(compileSchema(z.map(z.string(), z.string())), {
        valueNodes: [
          { tag: 'string-value', val: 'shared' },
          { tag: 'map-value', val: [{ key: 0, value: 0 }] },
        ],
        root: 1,
      }),
    ).toThrow(/aliased/);
  });

  it('matches invocation conversion rather than canonical application identity', () => {
    let transforms = 0;
    const schemas = [
      z.object({
        items: z.array(s.u8()),
        label: z.string().transform((x) => {
          transforms++;
          return `${x}!`;
        }),
      }),
      z.array(s.f32()),
      z.array(z.number().min(5)),
      z.array(z.literal('fixed')),
    ];
    const inputs = [
      { items: [7, 255], label: 'already!', extra: 'discarded' },
      [1.1, -0, Infinity, NaN],
      [3, 8],
      ['ignored', 'fixed'],
    ];
    schemas.forEach((schema, i) => {
      const codec = compileSchema(schema);
      expect(codec.invocationDirect).toBeDefined();
      const writer = new SchemaValueWriter();
      const root = codec.invocationDirect!.write(inputs[i], writer)!;
      const wire = { valueNodes: writer.valueNodes, root };
      const ordinary = schemaValueToWit(codec.toValue(inputs[i]));
      expect(wire).toEqual(ordinary);
      const reader = invocationSchemaValueReader(wire);
      expect(codec.invocationDirect!.read(reader, root)).toEqual(
        codec.fromValue(schemaValueFromWit(ordinary)),
      );
      reader.finish();
    });
    expect(transforms).toBe(0);
  });

  it('rejects sparse and typed-array results like ordinary list encoding', () => {
    const codec = compileSchema(z.array(s.u8()));
    const sparse = [7, 31];
    delete sparse[1];
    for (const value of [sparse, new Uint8Array([7, 31])]) {
      expect(() => schemaValueToWit(codec.toValue(value))).toThrow();
      expect(() => codec.invocationDirect!.write(value, new SchemaValueWriter())).toThrow();
    }
  });

  it('keeps nullish, optional, results and recursive shapes on their ordinary path', () => {
    const optional = compileSchema(z.object({ count: z.number().optional() }));
    expect(optional.invocationDirect).toBeUndefined();
    for (const value of [{}, { count: undefined }, { count: null }]) {
      expect(
        optional.fromValue(schemaValueFromWit(schemaValueToWit(optional.toValue(value)))),
      ).toEqual({ count: undefined });
    }
    const nullable = compileSchema(z.string().nullable());
    expect(nullable.invocationDirect).toBeUndefined();
    expect(
      nullable.fromValue(schemaValueFromWit(schemaValueToWit(nullable.toValue(undefined)))),
    ).toBeNull();
    expect(compileSchema(s.result(z.void(), z.string())).invocationDirect).toBeUndefined();
    const tree: z.ZodType = z.lazy(() => z.object({ children: z.array(tree) }));
    expect(compileSchema(tree).invocationDirect).toBeUndefined();
  });

  it('preserves ordinary alias and unreachable-node handling without skipping preflight', () => {
    const codec = compileSchema(z.array(z.string()));
    const wire = {
      valueNodes: [
        { tag: 'string-value' as const, val: 'shared' },
        { tag: 'list-value' as const, val: [0, 0] },
        { tag: 'u8-value' as const, val: 255 },
      ],
      root: 1,
    };
    const reader = invocationSchemaValueReader(wire);
    expect(codec.invocationDirect!.read(reader, wire.root)).toEqual(
      codec.fromValue(schemaValueFromWit(wire)),
    );
    reader.finish();
    // Unreachable scalar domains are not checked by ordinary invocation preflight.
    wire.valueNodes[2].val = 256;
    expect(() => invocationSchemaValueReader(wire)).not.toThrow();
    expect(() => schemaValueFromWit(wire)).not.toThrow();
    wire.valueNodes[1].val = [9];
    expect(() => invocationSchemaValueReader(wire)).toThrow(/out of range/);
    expect(() => schemaValueFromWit(wire)).toThrow(/out of range/);
  });

  it('preserves invocation interpretation of unexpected scalar tags and extra nested record fields', () => {
    const codec = compileSchema(z.array(z.object({ name: z.string() })));
    const wire = schemaValueToWit({
      tag: 'list',
      elements: [
        {
          tag: 'record',
          fields: [
            { tag: 'bool', value: true },
            { tag: 'u8', value: 31 },
          ],
        },
      ],
    });
    const reader = invocationSchemaValueReader(wire);
    expect(codec.invocationDirect!.read(reader, wire.root)).toEqual(
      codec.fromValue(schemaValueFromWit(wire)),
    );
  });

  it('converts nested records, arrays, options, and results without generic model conversion', () => {
    const codec = compileSchema(
      z.object({ names: z.array(z.string()), count: z.number().optional() }),
    );
    const input = { names: ['a', 'b'], count: 2 };
    const wire = directSchemaValueToWit(codec, input);
    expect(directSchemaValueFromWit(codec, wire)).toEqual(input);

    const resultCodec = compileSchema(s.result(z.number(), z.string()));
    const result = Result.err('nope');
    expect(
      directSchemaValueFromWit(resultCodec, directSchemaValueToWit(resultCodec, result)),
    ).toEqual(result);
  });

  it('does not call generic model functions', () => {
    const base = compileSchema(z.string());
    const codec = {
      ...base,
      toValue: () => {
        throw new Error('generic encode called');
      },
      fromValue: () => {
        throw new Error('generic decode called');
      },
    };
    const wire = directSchemaValueToWit(codec, 'direct');
    expect(directSchemaValueFromWit(codec, wire)).toBe('direct');
  });

  it('does not reapply source transforms to handler outputs', () => {
    const codec = compileSchema(z.string().transform((value) => `${value}!`));
    const wire = directSchemaValueToWit(codec, 'already-transformed!');
    expect(wire.valueNodes).toEqual([{ tag: 'string-value', val: 'already-transformed!' }]);
    expect(directSchemaValueFromWit(codec, wire)).toBe('already-transformed!');
  });

  it('encodes optional records with an option node instead of their mirrored fields', () => {
    const codec = compileSchema(z.object({ count: z.number() }).optional());
    for (const input of [undefined, { count: 19 }]) {
      const wire = directSchemaValueToWit(codec, input);
      expect(wire.valueNodes[wire.root].tag).toBe('option-value');
      expect(directSchemaValueFromWit(codec, wire)).toEqual(input);
    }
  });

  it('checks scalar types and numeric boundaries without a source validator', () => {
    const codec = compileSchema(z.number().min(5).max(8));
    for (const value of [5, 8]) {
      expect(directSchemaValueFromWit(codec, directSchemaValueToWit(codec, value))).toBe(value);
    }
    for (const value of [4, 9, '6']) {
      expect(() => directSchemaValueToWit(codec, value)).toThrow(/declared schema/);
    }
    expect(() => directSchemaValueToWit(compileSchema(z.string()), 7)).toThrow(/declared schema/);
  });

  it('encodes result<Unit> arms as an empty-record payload', () => {
    const codec = compileSchema(s.result(z.void(), z.string()));
    const wire = directSchemaValueToWit(codec, Result.ok(undefined));
    expect(wire.valueNodes).toEqual([
      { tag: 'record-value', val: [] },
      { tag: 'result-value', val: { tag: 'ok-value', val: 0 } },
    ]);
    expect(directSchemaValueFromWit(codec, wire)).toEqual(Result.ok(undefined));
  });

  it('does not install direct conversion for resource-bearing codecs', () => {
    const codec = compileSchema(z.object({ secret: s.secret(z.string()) }));
    expect(codec.direct).toBeUndefined();
    expect(() => directSchemaValueToWit(codec, { secret: {} })).toThrow(/does not support/);
  });

  it.each([
    [{ valueNodes: [{ tag: 'list-value', val: [9] }], root: 0 }, /out of range/],
    [{ valueNodes: [{ tag: 'list-value', val: [0] }], root: 0 }, /cycle/],
    [
      {
        valueNodes: [
          { tag: 'string-value', val: 'x' },
          { tag: 'list-value', val: [0, 0] },
        ],
        root: 1,
      },
      /aliased/,
    ],
  ] as const)('rejects malformed graph %#', (tree, error) => {
    const codec = compileSchema(z.array(z.string()));
    expect(() => directSchemaValueFromWit(codec, structuredClone(tree))).toThrow(error);
  });

  it('rejects unreachable asymmetric graph nodes', () => {
    const codec = compileSchema(z.string());
    expect(() =>
      directSchemaValueFromWit(codec, {
        valueNodes: [
          { tag: 'string-value', val: 'root' },
          { tag: 'string-value', val: 'orphan' },
        ],
        root: 0,
      }),
    ).toThrow(/unreachable/);
  });
});
