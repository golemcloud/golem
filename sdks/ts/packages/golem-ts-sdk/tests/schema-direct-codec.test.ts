// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

import { describe, expect, it } from 'vitest';
import { z } from 'zod';
import { Result } from '../src/host/result';
import { s } from '../src/schema/markers';
import {
  compileSchema,
  directSchemaValueFromWit,
  directSchemaValueToWit,
} from '../src/schema/public';

describe('direct flat-wire schema codecs', () => {
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
