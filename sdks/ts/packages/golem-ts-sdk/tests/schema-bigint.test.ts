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

import { describe, it, expect } from 'vitest';
import { z } from 'zod';
import * as vb from 'valibot';
import { type as arkType } from 'arktype';
import { Schema } from 'effect';

import { compileSchema } from '../src/schema/adapter';
import { s } from '../src/schema/markers';
import '../src/schema/zod';
import '../src/schema/valibot';
import '../src/schema/arktype';
import '../src/schema/effect';
import {
  SchemaEncodeError,
  schemaValueFromWit,
  schemaValueToWit,
} from '../src/internal/schema-model';

const S64_MIN = -(2n ** 63n);
const S64_MAX = 2n ** 63n - 1n;

const ark = (def: unknown) => arkType(def as never);
const std = <A, I>(schema: Schema.Schema<A, I>) => {
  const fnForm = Schema.standardSchemaV1(schema);
  return { '~standard': fnForm['~standard'], ast: schema.ast };
};

const bigintSchemas: [string, unknown][] = [
  ['zod', z.bigint()],
  ['valibot', vb.bigint()],
  ['arktype', ark('bigint')],
  ['effect', std(Schema.BigIntFromSelf)],
];

const negativeLiteralSchemas: [string, unknown][] = [
  ['zod', z.literal(-5n)],
  ['valibot', vb.literal(-5n)],
  ['arktype', ark('-5n')],
  ['effect', std(Schema.Literal(-5n))],
];

const positiveLiteralSchemas: [string, unknown][] = [
  ['zod', z.literal(5n)],
  ['valibot', vb.literal(5n)],
  ['arktype', ark('5n')],
  ['effect', std(Schema.Literal(5n))],
];

describe('bigint maps to s64', () => {
  it.each(bigintSchemas)('%s bigint is s64 and round-trips the full s64 range', (_, schema) => {
    const codec = compileSchema(schema as never);
    expect(codec.graph.root.body.tag).toBe('s64');
    for (const n of [S64_MIN, -1n, 0n, 42n, S64_MAX]) {
      const value = codec.toValue(n);
      expect(value).toEqual({ tag: 's64', value: n });
      const decoded = codec.fromValue(schemaValueFromWit(schemaValueToWit(value)));
      expect(typeof decoded).toBe('bigint');
      expect(decoded).toBe(n);
    }
  });

  it.each(bigintSchemas)('%s bigint outside the s64 range fails to encode', (_, schema) => {
    const codec = compileSchema(schema as never);
    for (const n of [S64_MAX + 1n, S64_MIN - 1n, 2n ** 64n - 1n]) {
      expect(() => schemaValueToWit(codec.toValue(n))).toThrow(SchemaEncodeError);
    }
  });

  it.each(negativeLiteralSchemas)('%s negative bigint literal is s64', (_, schema) => {
    const codec = compileSchema(schema as never);
    expect(codec.graph.root.body.tag).toBe('s64');
    const value = codec.toValue(-5n);
    expect(value).toEqual({ tag: 's64', value: -5n });
    expect(codec.fromValue(schemaValueFromWit(schemaValueToWit(value)))).toBe(-5n);
  });

  it.each(positiveLiteralSchemas)('%s positive bigint literal is s64', (_, schema) => {
    const codec = compileSchema(schema as never);
    expect(codec.graph.root.body.tag).toBe('s64');
    expect(codec.toValue(5n)).toEqual({ tag: 's64', value: 5n });
  });

  it('s.u64() carries unsigned values above the s64 range', () => {
    const codec = compileSchema(s.u64());
    expect(codec.graph.root.body.tag).toBe('u64');
    const big = 2n ** 64n - 1n;
    const value = codec.toValue(big);
    expect(codec.fromValue(schemaValueFromWit(schemaValueToWit(value)))).toBe(big);
  });
});
