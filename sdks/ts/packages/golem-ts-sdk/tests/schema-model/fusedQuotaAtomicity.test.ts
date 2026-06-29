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
  serializeGraphToWit,
  deserializeGraphFromWit,
  compileGraphEncoder,
  compileGraphDecoder,
} from '../../src/internal/mapping/values/schemaValue';
import {
  r,
  resolvedField,
  type ResolvedGraph,
} from '../../src/internal/mapping/types/resolvedType';
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

// A sentinel standing in for the opaque `own<quota-token>` resource, wrapped in
// a take-once handle and then a `QuotaToken`. The same `handle` reference is kept
// so the test can observe whether the affine move happened.
function makeToken() {
  const raw = { id: 'opaque-quota-token' } as never;
  const handle = GuestQuotaTokenHandle.fromRaw(QUOTA_INTERNAL, raw);
  const token = QuotaToken._fromHandle(QUOTA_INTERNAL, handle);
  return { raw, handle, token };
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
});
