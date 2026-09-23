// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  field,
  schemaType,
  t,
  type SchemaGraph,
  type SchemaType,
} from '../src/internal/schema-model';
import { SchemaRef, type JsonValue } from '../src/schema/ref';

interface ConformanceCase {
  id: string;
  operation: 'roundtrip' | 'reject' | 'json-schema' | 'semantic';
  fixture: string;
  input?: JsonValue;
  inputs?: JsonValue[];
  path?: string;
  expected: JsonValue;
}

interface ConformanceCorpus {
  version: string;
  schemaKinds: string[];
  restrictionKinds: string[];
  caseIds: string[];
  cases: ConformanceCase[];
}

const corpus = JSON.parse(
  readFileSync(
    new URL('../../../../../test-data/reflection-conformance/v1.json', import.meta.url),
    'utf8',
  ),
) as ConformanceCorpus;

const supportedSchemaKinds = [
  'ref',
  'bool',
  's8',
  's16',
  's32',
  's64',
  'u8',
  'u16',
  'u32',
  'u64',
  'f32',
  'f64',
  'char',
  'string',
  'record',
  'variant',
  'enum',
  'flags',
  'tuple',
  'list',
  'fixed-list',
  'map',
  'option',
  'result',
  'text',
  'binary',
  'path',
  'url',
  'datetime',
  'duration',
  'quantity',
  'union',
  'secret',
  'quota-token',
  'permission-card',
  'future',
  'stream',
] as const;

const supportedRestrictionKinds = [
  'numeric-minimum',
  'numeric-maximum',
  'numeric-unit',
  'text-languages',
  'text-min-length',
  'text-max-length',
  'text-regex',
  'binary-mime-types',
  'binary-min-bytes',
  'binary-max-bytes',
  'path-direction',
  'path-kind',
  'path-mime-types',
  'path-extensions',
  'url-schemes',
  'url-hosts',
  'quantity-base-unit',
  'quantity-suffixes',
  'quantity-minimum',
  'quantity-maximum',
  'union-prefix',
  'union-suffix',
  'union-regex',
  'union-field',
] as const;

function schema(root: SchemaType): SchemaRef {
  return new SchemaRef({ defs: new Map(), root });
}

function fixture(name: string): SchemaRef {
  switch (name) {
    case 's64':
      return schema(t.s64());
    case 'u64':
      return schema(t.u64());
    case 'binary':
      return schema(schemaType({ tag: 'binary', restrictions: {} }));
    case 'duration':
      return schema(t.duration());
    case 'quantity':
      return schema(t.quantity({ baseUnit: 'm', allowedUnits: [] }));
    case 'optional-record': {
      const graph: SchemaGraph = {
        defs: new Map([['conformance.optional', { body: t.option(t.string()) }]]),
        root: t.record([
          field('direct', t.option(t.string())),
          field('referenced', t.ref('conformance.optional')),
        ]),
      };
      return new SchemaRef(graph);
    }
    case 'tool-input':
      return schema(
        t.record([
          field('pattern', t.string()),
          field('paths', t.list(t.string())),
          field('ignoreCase', t.option(t.bool())),
        ]),
      );
    case 'config-entry':
      return schema(t.record([field('path', t.list(t.string())), field('value', t.s64())]));
    case 'constrained-u32':
      return schema(t.u32({ max: { tag: 'unsigned', val: 10n } }));
    case 'result':
      return schema(t.result(t.string(), t.u32()));
    case 'custom-error':
      return schema(
        t.result(t.string(), t.record([field('code', t.string()), field('retryable', t.bool())])),
      );
    default:
      throw new Error(`unknown conformance fixture ${name}`);
  }
}

function atPointer(value: JsonValue, pointer: string): JsonValue {
  if (pointer === '') return value;
  return pointer
    .slice(1)
    .split('/')
    .map((part) => part.replaceAll('~1', '/').replaceAll('~0', '~'))
    .reduce<JsonValue>((current, part) => {
      if (current === null || typeof current !== 'object' || Array.isArray(current)) {
        throw new Error(`cannot resolve ${pointer}`);
      }
      const next = current[part];
      if (next === undefined) throw new Error(`missing ${pointer}`);
      return next;
    }, value);
}

function expectSubset(actual: JsonValue, expected: JsonValue): void {
  if (expected !== null && typeof expected === 'object' && !Array.isArray(expected)) {
    expect(actual).not.toBeNull();
    expect(Array.isArray(actual)).toBe(false);
    for (const [key, value] of Object.entries(expected)) {
      expectSubset((actual as Record<string, JsonValue>)[key], value);
    }
  } else {
    expect(actual).toEqual(expected);
  }
}

function assertSemantic(testCase: ConformanceCase): void {
  const expected = testCase.expected as Record<string, JsonValue>;
  switch (testCase.fixture) {
    case 'unsupported-leaves': {
      const unsupported = [
        t.secret(t.string()),
        t.quotaToken({}),
        t.permissionCard({ polymorphic: false }),
        schemaType({ tag: 'future', element: t.string() }),
        t.stream(t.string()),
      ];
      expect(unsupported).toHaveLength(expected.count as number);
      for (const type of unsupported) expectSubset(schema(type).toJsonSchema(), expected.schema);
      break;
    }
    case 'all-kinds':
      expect(supportedSchemaKinds).toEqual(expected.names);
      expect(corpus.schemaKinds).toEqual(expected.names);
      break;
    case 'all-restrictions':
      expect(supportedRestrictionKinds).toEqual(expected.names);
      expect(corpus.restrictionKinds).toEqual(expected.names);
      break;
    case 'graph': {
      const referenced = fixture('optional-record');
      const inline = schema(
        t.record([
          field('direct', t.option(t.string())),
          field('referenced', t.option(t.string())),
        ]),
      );
      expect(referenced.packJson({})).toEqual(inline.packJson({}));
      expect(referenced.validateJson({}).success).toBe(true);
      break;
    }
    default:
      throw new Error(`unknown semantic conformance fixture ${testCase.fixture}`);
  }
}

describe('reflection conformance corpus', () => {
  it('executes the complete declared case-ID set', () => {
    expect(corpus.version).toBe('1.0.0');
    const executed = new Set<string>();
    for (const testCase of corpus.cases) {
      expect(executed.has(testCase.id), `duplicate case ID ${testCase.id}`).toBe(false);
      executed.add(testCase.id);
      switch (testCase.operation) {
        case 'roundtrip': {
          const ref = fixture(testCase.fixture);
          expect(ref.unpackJson(ref.packJson(testCase.input!)), testCase.id).toEqual(
            testCase.expected,
          );
          break;
        }
        case 'reject': {
          const ref = fixture(testCase.fixture);
          for (const input of testCase.inputs ?? [testCase.input!]) {
            expect(ref.validateJson(input).success, testCase.id).toBe(false);
          }
          break;
        }
        case 'json-schema':
          expectSubset(
            atPointer(fixture(testCase.fixture).toJsonSchema(), testCase.path ?? ''),
            testCase.expected,
          );
          break;
        case 'semantic':
          assertSemantic(testCase);
          break;
        default:
          throw new Error(`unknown conformance operation for ${testCase.id}`);
      }
    }
    expect([...executed].sort()).toEqual([...corpus.caseIds].sort());
  });
});
