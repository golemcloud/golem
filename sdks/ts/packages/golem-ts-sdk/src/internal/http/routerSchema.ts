// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { field, mergeGraphDefs, t, v, type SchemaValue } from '../schema-model';
import type { SchemaCodec } from '../../schema/codec';
import { compileSchema } from '../../schema/adapter';
import { s, WIT_MARKER, type MarkerSchema } from '../../schema/markers';
import type { AgentStream } from '../../schema/agentStream';
import { copyHttpHeaders, type HttpRequest, type HttpResponse } from '../../httpRouterContract';

function schema<T>(codec: SchemaCodec): MarkerSchema<T> {
  return {
    '~standard': {
      version: 1,
      vendor: 'golem.marker',
      validate: (value) => ({ value: value as T }),
    },
    [WIT_MARKER]: () => codec,
  };
}

const stringCodec: SchemaCodec = {
  graph: { defs: new Map(), root: t.string() },
  toValue: (value) => {
    if (typeof value !== 'string') throw new TypeError('Expected HTTP string');
    return v.string(value);
  },
  fromValue: (value) => (value as Extract<SchemaValue, { tag: 'string' }>).value,
};

export const httpStringSchema = schema<string>(stringCodec);
const bytes = compileSchema(s.bytes());
const body = compileSchema(s.stream(s.bytes()));

function record(fields: readonly (readonly [string, SchemaCodec])[]): SchemaCodec {
  return {
    graph: {
      defs: mergeGraphDefs(fields.map(([, codec]) => codec.graph)),
      root: t.record(fields.map(([name, codec]) => field(name, codec.graph.root))),
    },
    toValue: (value) =>
      v.record(
        fields.map(([name, codec]) => codec.toValue((value as Record<string, unknown>)[name])),
      ),
    fromValue: (value) =>
      Object.fromEntries(
        fields.map(([name, codec], index) => [
          name,
          codec.fromValue((value as Extract<SchemaValue, { tag: 'record' }>).fields[index]),
        ]),
      ),
  };
}

const header = record([
  ['name', stringCodec],
  ['value', bytes],
]);
const headers: SchemaCodec = {
  graph: { defs: new Map(), root: t.list(header.graph.root) },
  toValue: (value) =>
    v.list(copyHttpHeaders(value as HttpResponse<unknown>['headers']).map(header.toValue)),
  fromValue: (value) =>
    (value as Extract<SchemaValue, { tag: 'list' }>).elements.map(header.fromValue),
};
const query: SchemaCodec = {
  graph: { defs: new Map(), root: t.option(t.string()) },
  toValue: (value) => v.option(value === undefined ? undefined : stringCodec.toValue(value)),
  fromValue: (value) => {
    const inner = (value as Extract<SchemaValue, { tag: 'option' }>).value;
    return inner === undefined ? undefined : stringCodec.fromValue(inner);
  },
};

export const httpRequestSchema = schema<HttpRequest<AgentStream<Uint8Array>>>(
  record([
    ['method', stringCodec],
    ['scheme', stringCodec],
    ['authority', stringCodec],
    ['path', stringCodec],
    ['query', query],
    ['headers', headers],
    ['body', body],
  ]),
);

export const httpResponseSchema = schema<HttpResponse<AgentStream<Uint8Array>>>(
  record([
    ['status', compileSchema(s.u16())],
    ['headers', headers],
    ['body', body],
  ]),
);
