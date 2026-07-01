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

// Reproduces the review-comment bug "TS fused encoder/decoder atomicity":
//
// The `quota-token` is an affine take-once handle. The *non-fused* codec
// (`schemaValueToWit` / `schemaValueFromWit`) preflights the whole value tree
// before moving any owned handle, so a tree the boundary would reject never
// destroys a still-valid token — the encode/decode is atomic.
//
// The *fused* codec (`serializeGraphToWit` / `deserializeGraphFromWit`, and the
// equivalent compiled codec `compileGraphEncoder` / `compileGraphDecoder`) fuses
// the preflight and the move into a single pass. When it reaches a
// `quota-token` node it takes/lifts the handle inline and keeps walking; if a
// *later* sibling then fails (a type mismatch, an out-of-range index, ...), the
// whole operation throws but that handle has already been moved out. The caller
// is left with a permanently consumed token on encode, and a stranded/leaked
// owned resource on decode — something the non-fused path guarantees cannot
// happen.
//
// These tests assert the atomicity invariant the non-fused path already
// provides (see `edge-cases.test.ts` "encoding a tree where a sibling fails
// leaves the quota-token handle untransferred" and "decoding a tree where a
// later node is invalid neither lifts nor leaks the quota-token handle") and
// which the fused paths are supposed to be observationally identical to. On
// the current (buggy) fused paths they FAIL, reproducing the issue. The fix
// makes them pass.

import { describe, it, expect } from 'vitest';

import type {
  SchemaValueTree as WitSchemaValueTree,
  SchemaValueNode as WitSchemaValueNode,
} from 'golem:core/types@2.0.0';

import {
  deserializeGraph,
  serializeGraphToWit,
  deserializeGraphFromWit,
  compileGraphEncoder,
  compileGraphDecoder,
} from '../../src/internal/mapping/values/schemaValue';
import {
  encodeInputRecordToWit,
  decodeInputRecordFromWit,
  decodeOutputFromWit,
} from '../../src/internal/mapping/values/boundaryValue';
import {
  r,
  resolvedField,
  type ResolvedGraph,
} from '../../src/internal/mapping/types/resolvedType';
import type { SchemaValue } from '../../src/internal/schema-model';
import type { RuntimeOutput, RuntimeParam } from '../../src/internal/typeInfoInternal';
import { Secret } from '../../src/agentConfig';
import { GuestSecretHandle } from '../../src/internal/schema-model/secretHandle';
import { SECRET_INTERNAL } from '../../src/internal/schema-model/secretInternal';
import { QuotaToken } from '../../src/host/quota';
import { GuestQuotaTokenHandle } from '../../src/internal/schema-model/quotaTokenHandle';
import { QUOTA_INTERNAL } from '../../src/internal/schema-model/quotaInternal';

// A record `{ token: quota-token, value: u32 }`. The `token` field is lowered /
// lifted first (record fields are emitted in order), then the `value` field
// fails — exactly the "take-then-sibling-fails" window the review describes.
function recordWithQuotaAndU32(): ResolvedGraph {
  return {
    defs: new Map(),
    root: r.record(
      [
        resolvedField('token', r.quotaToken({ resourceName: 'test-resource' })),
        resolvedField('value', r.u32()),
      ],
      'Rec',
      'M',
    ),
  };
}

// A record `{ a: quota-token, b: quota-token }` to reproduce the aliasing
// case: the non-fused preflight rejects the duplicate handle without moving it,
// while the fused path takes the first before the second fails.
function recordWithTwoQuotaFields(): ResolvedGraph {
  return {
    defs: new Map(),
    root: r.record(
      [
        resolvedField('a', r.quotaToken({ resourceName: 'test-resource' })),
        resolvedField('b', r.quotaToken({ resourceName: 'test-resource' })),
      ],
      'Rec2',
      'M',
    ),
  };
}

function tokenParam(name: string): RuntimeParam {
  return {
    name,
    type: {
      tag: 'schema',
      graph: { defs: new Map(), root: r.quotaToken({ resourceName: 'test-resource' }) },
      tsType: {} as never,
    },
  };
}

function secretParam(name: string): RuntimeParam {
  return {
    name,
    type: {
      tag: 'schema',
      graph: { defs: new Map(), root: r.secret(r.string()) },
      tsType: {} as never,
    },
  };
}

function u32Param(name: string): RuntimeParam {
  return {
    name,
    type: {
      tag: 'schema',
      graph: { defs: new Map(), root: r.u32() },
      tsType: {} as never,
    },
  };
}

function u8Param(name: string): RuntimeParam {
  return {
    name,
    type: {
      tag: 'schema',
      graph: { defs: new Map(), root: r.u8() },
      tsType: {} as never,
    },
  };
}

function unstructuredTextParam(name: string): RuntimeParam {
  return {
    name,
    type: {
      tag: 'unstructured-text',
      languages: [],
      tsType: {} as never,
    },
  };
}

function schemaParam(name: string, graph: ResolvedGraph): RuntimeParam {
  return {
    name,
    type: {
      tag: 'schema',
      graph,
      tsType: {} as never,
    },
  };
}

function stringOutput(): RuntimeOutput {
  return {
    tag: 'single',
    type: {
      tag: 'schema',
      graph: { defs: new Map(), root: r.string() },
      tsType: {} as never,
    },
  };
}

// A sentinel standing in for the opaque `own<quota-token>` resource, wrapped in
// a take-once handle and then a `QuotaToken`. The same `handle` reference is kept
// so the test can observe whether the affine move happened.
function makeToken() {
  const raw = { id: 'opaque-quota-token' } as never;
  const handle = GuestQuotaTokenHandle.fromRaw(QUOTA_INTERNAL, raw);
  const token = QuotaToken._fromHandle(QUOTA_INTERNAL, handle);
  return { raw, handle, token };
}

function makeSecret() {
  const raw = { id: 'opaque-secret' } as never;
  const handle = GuestSecretHandle.fromRaw(SECRET_INTERNAL, raw);
  const secret = Secret._fromHandle<string>(SECRET_INTERNAL, handle, {
    defs: new Map(),
    root: r.string(),
  });
  return { raw, handle, secret };
}

describe('fused codec quota-token atomicity (reproduces review comment)', () => {
  // ---------------------------------------------------------------- encode

  it('fused encode: a sibling failure does not consume the quota-token handle', () => {
    const graph = recordWithQuotaAndU32();
    const { handle, token } = makeToken();

    // The `u32` field receives a string, so the encode throws a type mismatch
    // *after* the leading `token` field was already walked.
    expect(() => serializeGraphToWit({ token, value: 'not-a-number' }, graph)).toThrow();

    // Atomic invariant (matches the non-fused `schemaValueToWit`): the encode
    // failed, so the caller must still own its token. On the buggy fused path
    // `handle.take()` already ran, so this is `false`.
    expect(handle.isPresent()).toBe(true);
  });

  it('fused encode: aliasing one token twice is rejected without consuming it', () => {
    const graph = recordWithTwoQuotaFields();
    const { handle, token } = makeToken();

    // The same `token` appears in both record fields. The non-fused preflight
    // rejects this as "more than once" without moving the handle; the fused
    // path takes the first field's handle, then the second field's `take()`
    // returns `undefined` and throws "already transferred" — but the handle is
    // already consumed.
    expect(() => serializeGraphToWit({ a: token, b: token }, graph)).toThrow();

    expect(handle.isPresent()).toBe(true);
  });

  it('compiled encode: a sibling failure does not consume the quota-token handle', () => {
    const graph = recordWithQuotaAndU32();
    const encode = compileGraphEncoder(graph);
    const { handle, token } = makeToken();

    expect(() => encode({ token, value: 'not-a-number' })).toThrow();

    // The compiled codec is what the runtime boundary actually uses on the hot
    // path (`encodeOutputToWit`), so it must be atomic too.
    expect(handle.isPresent()).toBe(true);
  });

  it('compiled encode: aliasing one token twice is rejected without consuming it', () => {
    const graph = recordWithTwoQuotaFields();
    const encode = compileGraphEncoder(graph);
    const { handle, token } = makeToken();

    expect(() => encode({ a: token, b: token })).toThrow();

    expect(handle.isPresent()).toBe(true);
  });

  // ---------------------------------------------------------------- decode

  // A wire tree matching `recordWithQuotaAndU32`: a `record-value` root whose
  // first field is a `quota-token-handle` node (lifted) and whose second field
  // is a `string-value` node that the `u32` decoder rejects (type mismatch).
  function mismatchedRecordWire(raw: unknown): WitSchemaValueTree {
    return {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] } as WitSchemaValueNode,
        { tag: 'quota-token-handle', val: raw } as WitSchemaValueNode,
        { tag: 'string-value', val: 'oops' } as WitSchemaValueNode,
      ],
      root: 0,
    };
  }

  it('fused decode: a sibling failure does not lift the quota-token handle', () => {
    const graph = recordWithQuotaAndU32();
    const raw = { id: 'opaque-quota-token' } as never;
    const wit = mismatchedRecordWire(raw);

    // The `u32` field sees a `string-value` node, so the decode throws a wire
    // mismatch *after* the leading `token` field was already lifted.
    expect(() => deserializeGraphFromWit(wit, graph)).toThrow();

    // Atomic invariant (matches the non-fused `schemaValueFromWit`, which
    // preflights and drains before any lift): the decode failed, so the owned
    // resource must still be sitting in the wire tree for the runtime to
    // release. On the buggy fused path the lift already ran (`val = undefined`
    // and a `QuotaToken` was created and discarded), stranding the resource.
    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw);
  });

  it('compiled decode: a sibling failure does not lift the quota-token handle', () => {
    const graph = recordWithQuotaAndU32();
    const decode = compileGraphDecoder(graph);
    const raw = { id: 'opaque-quota-token' } as never;
    const wit = mismatchedRecordWire(raw);

    expect(() => decode(wit)).toThrow();

    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw);
  });

  // -------------------------------------------------- input-record boundary

  it('input-record encode: a later parameter failure does not consume a quota-token', () => {
    const { handle, token } = makeToken();

    expect(() =>
      encodeInputRecordToWit([token, 'not-a-number'], [tokenParam('token'), u32Param('value')]),
    ).toThrow();

    expect(handle.isPresent()).toBe(true);
  });

  it('input-record encode: using one secret for two parameters is rejected without consuming it', () => {
    const { handle, secret } = makeSecret();

    expect(() =>
      encodeInputRecordToWit([secret, secret], [secretParam('a'), secretParam('b')]),
    ).toThrow();

    expect(handle.isPresent()).toBe(true);
  });

  it('input-record encode: out-of-range u8 is rejected before consuming a secret handle', () => {
    const { handle, secret } = makeSecret();

    let error: unknown;
    try {
      encodeInputRecordToWit([secret, 256], [secretParam('secret'), u8Param('byte')]);
    } catch (e) {
      error = e;
    }

    expect({
      threwRangeError: error instanceof Error && /u8 value out of range/.test(error.message),
      handlePresent: handle.isPresent(),
    }).toEqual({ threwRangeError: true, handlePresent: true });
  });

  it('input-record encode: out-of-range s64 is rejected before consuming a secret handle', () => {
    const { handle, secret } = makeSecret();

    let error: unknown;
    try {
      encodeInputRecordToWit(
        [secret, 2n ** 63n],
        [secretParam('secret'), schemaParam('count', { defs: new Map(), root: r.s64() })],
      );
    } catch (e) {
      error = e;
    }

    expect({
      threwRangeError: error instanceof Error && /s64 value out of range/.test(error.message),
      handlePresent: handle.isPresent(),
    }).toEqual({ threwRangeError: true, handlePresent: true });
  });

  it('fused encode: invalid char is rejected before consuming a secret handle', () => {
    const graph: ResolvedGraph = {
      defs: new Map(),
      root: r.record([
        resolvedField('secret', r.secret(r.string())),
        resolvedField('char', r.char()),
      ]),
    };
    const { handle, secret } = makeSecret();

    let error: unknown;
    try {
      serializeGraphToWit({ secret, char: 'ab' }, graph);
    } catch (e) {
      error = e;
    }

    expect({
      rejectedInvalidChar: error instanceof Error && /char value/.test(error.message),
      handlePresent: handle.isPresent(),
    }).toEqual({ rejectedInvalidChar: true, handlePresent: true });
  });

  it('compiled encode: out-of-range u8 is rejected before consuming a quota-token handle', () => {
    const graph: ResolvedGraph = {
      defs: new Map(),
      root: r.record([
        resolvedField('token', r.quotaToken({ resourceName: 'test-resource' })),
        resolvedField('byte', r.u8()),
      ]),
    };
    const encode = compileGraphEncoder(graph);
    const { handle, token } = makeToken();

    let error: unknown;
    try {
      encode({ token, byte: 256 });
    } catch (e) {
      error = e;
    }

    expect({
      threwRangeError: error instanceof Error && /u8 value out of range/.test(error.message),
      handlePresent: handle.isPresent(),
    }).toEqual({ threwRangeError: true, handlePresent: true });
  });

  it('input-record decode: a later parameter failure does not lift a secret handle', () => {
    const raw = { id: 'opaque-secret' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
        { tag: 'string-value', val: 'oops' } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect(() =>
      decodeInputRecordFromWit(wit, [secretParam('secret'), u32Param('value')], {
        tag: 'anonymous',
      }),
    ).toThrow();

    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw);
  });

  it('input-record rich decode failure does not lift a preceding secret handle', () => {
    const raw = { id: 'opaque-secret' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
        { tag: 'string-value', val: 'not-a-rich-variant' } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect(() =>
      decodeInputRecordFromWit(wit, [secretParam('secret'), unstructuredTextParam('text')], {
        tag: 'anonymous',
      }),
    ).toThrow(/Expected variant value/);

    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw);
  });

  it('input-record decode: extra fields are rejected before lifting secret handles', () => {
    const raw1 = { id: 'opaque-secret-1' } as never;
    const raw2 = { id: 'opaque-secret-2' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw1 } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw2 } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect(() =>
      decodeInputRecordFromWit(wit, [secretParam('secret')], { tag: 'anonymous' }),
    ).toThrow(/Unexpected extra arguments/);

    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw1);
    expect((wit.valueNodes[2] as { val: unknown }).val).toBe(raw2);
  });

  it('review repro: constructor WIT decode path rejects extra fields without consuming secret handles', () => {
    const raw = { id: 'opaque-secret' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
        { tag: 'u32-value', val: 7 } as WitSchemaValueNode,
      ],
      root: 0,
    };

    let error: unknown;
    try {
      decodeInputRecordFromWit(wit, [secretParam('secret')], { tag: 'anonymous' });
    } catch (e) {
      error = e;
    }

    expect(error).toBeInstanceOf(Error);
    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw);
  });

  it('fused graph decode rejects unreferenced secret handle nodes', () => {
    const raw = { id: 'opaque-secret' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'string-value', val: 'ok' } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect(() => deserializeGraphFromWit(wit, { defs: new Map(), root: r.string() })).toThrow(
      /secret handle not referenced/,
    );
    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw);
  });

  it('fused graph decode drains unreferenced quota-token handles on preflight rejection', () => {
    const raw = { id: 'unreferenced-quota-token' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'string-value', val: 'ok' } as WitSchemaValueNode,
        { tag: 'quota-token-handle', val: raw } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect(() => deserializeGraphFromWit(wit, { defs: new Map(), root: r.string() })).toThrow(
      /quota-token handle not referenced/,
    );
    expect((wit.valueNodes[1] as { val: unknown }).val).toBeUndefined();
  });

  it('input-record fused decode rejects unreferenced secret handle nodes', () => {
    const raw = { id: 'opaque-secret' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'record-value', val: [1] } as WitSchemaValueNode,
        { tag: 'u32-value', val: 1 } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect(() => decodeInputRecordFromWit(wit, [u32Param('value')], { tag: 'anonymous' })).toThrow(
      /secret handle not referenced/,
    );
    expect((wit.valueNodes[2] as { val: unknown }).val).toBe(raw);
  });

  it('input-record decode preserves each parameter graph definitions on the owned-handle path', () => {
    const raw = { id: 'opaque-secret' } as never;
    const firstGraph: ResolvedGraph = {
      defs: new Map([['X', r.option(r.u32(), 'null')]]),
      root: r.ref('X'),
    };
    const secondGraph: ResolvedGraph = {
      defs: new Map([['X', r.option(r.u32(), 'undefined')]]),
      root: r.secret(r.string()),
    };
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] } as WitSchemaValueNode,
        { tag: 'option-value', val: undefined } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
      ],
      root: 0,
    };

    const args = decodeInputRecordFromWit(
      wit,
      [schemaParam('maybe', firstGraph), schemaParam('secret', secondGraph)],
      { tag: 'anonymous' },
    );

    expect(args[0]).toBeNull();
  });

  it('output fused decode rejects unreferenced secret handle nodes on the compiled path', () => {
    const raw = { id: 'opaque-secret' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'string-value', val: 'ok' } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect(() => decodeOutputFromWit(wit, stringOutput())).toThrow(/secret handle not referenced/);
    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw);
  });

  it('compiled graph decode rejects unreferenced secret handle nodes', () => {
    const raw = { id: 'opaque-secret' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'string-value', val: 'ok' } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
      ],
      root: 0,
    };

    const decode = compileGraphDecoder({ defs: new Map(), root: r.string() });

    expect(() => decode(wit)).toThrow(/secret handle not referenced/);
    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw);
  });

  it('schema decode rejects extra record fields carrying owned secret handles', () => {
    const raw = { id: 'ignored-secret' } as never;
    const graph: ResolvedGraph = {
      defs: new Map(),
      root: r.record([resolvedField('value', r.u32())]),
    };
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'record-value', val: [1, 2] } as WitSchemaValueNode,
        { tag: 'u32-value', val: 7 } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect(() => deserializeGraphFromWit(wit, graph)).toThrow(/record|extra|mismatch/);
    expect((wit.valueNodes[2] as { val: unknown }).val).toBe(raw);
  });

  it('output schema decode rejects payloads on no-payload variant cases carrying owned secrets', () => {
    const raw = { id: 'ignored-secret' } as never;
    const graph: ResolvedGraph = {
      defs: new Map(),
      root: r.variant(true, [{ name: 'empty' }]),
    };
    const output: RuntimeOutput = {
      tag: 'single',
      type: {
        tag: 'schema',
        graph,
        tsType: {} as never,
      },
    };
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'variant-value', val: { case_: 0, payload: 1 } } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect(() => decodeOutputFromWit(wit, output)).toThrow(/variant|payload|mismatch/);
    expect((wit.valueNodes[1] as { val: unknown }).val).toBe(raw);
  });

  it('non-fused schema decode rejects extra record fields carrying owned secret handles', () => {
    const { handle } = makeSecret();
    const graph: ResolvedGraph = {
      defs: new Map(),
      root: r.record([resolvedField('value', r.u32())]),
    };
    const value: SchemaValue = {
      tag: 'record',
      fields: [
        { tag: 'u32', value: 7 },
        { tag: 'secret', handle },
      ],
    };

    expect(() => deserializeGraph(value, graph)).toThrow(/record|extra|mismatch/);
  });

  it('non-fused schema decode rejects payloads on no-payload variant cases carrying owned secrets', () => {
    const { handle } = makeSecret();
    const graph: ResolvedGraph = {
      defs: new Map(),
      root: r.variant(true, [{ name: 'empty' }]),
    };
    const value: SchemaValue = {
      tag: 'variant',
      caseIndex: 0,
      payload: { tag: 'secret', handle },
    };

    expect(() => deserializeGraph(value, graph)).toThrow(/variant|payload|mismatch/);
  });

  it('output rich decode failure does not lift a secret handle', () => {
    const raw = { id: 'opaque-secret' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [{ tag: 'secret-value', val: raw } as WitSchemaValueNode],
      root: 0,
    };
    const output: RuntimeOutput = {
      tag: 'single',
      type: {
        tag: 'unstructured-text',
        languages: [],
        tsType: {} as never,
      },
    };

    expect(() => decodeOutputFromWit(wit, output)).toThrow(/Expected variant value/);
    expect((wit.valueNodes[0] as { val: unknown }).val).toBe(raw);
  });

  it('output multimodal decode success consumes a secret handle from the original wire tree', () => {
    const raw = { id: 'opaque-secret' } as never;
    const wit: WitSchemaValueTree = {
      valueNodes: [
        { tag: 'list-value', val: [1] } as WitSchemaValueNode,
        { tag: 'variant-value', val: { case_: 0, payload: 2 } } as WitSchemaValueNode,
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
      ],
      root: 0,
    };
    const output: RuntimeOutput = {
      tag: 'single',
      type: {
        tag: 'multimodal',
        cases: [
          {
            name: 'secret',
            type: {
              tag: 'schema',
              graph: { defs: new Map(), root: r.secret(r.string()) },
              tsType: {} as never,
            },
          },
        ],
        tsType: {} as never,
      },
    };

    const decoded = decodeOutputFromWit(wit, output);

    expect(decoded[0].tag).toBe('secret');
    expect(decoded[0].val).toBeInstanceOf(Secret);
    expect((wit.valueNodes[2] as { val: unknown }).val).toBeUndefined();
  });

  it('Secret objects cannot be serialized to JSON', () => {
    const { secret } = makeSecret();

    expect(() => JSON.stringify(secret)).toThrow(/secret values cannot be serialized/);
  });
});
