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

// Shared support for plain (non-discriminated) unions across the vendor
// walkers. A plain union compiles to a WIT `variant` with auto-named cases
// `case0`/`case1`/… (structurally compatible with the discriminated-union
// variants, which use the same `case${i}` naming and index-based decode).
//
// Decode is unambiguous (the value carries its `caseIndex`). Encode disambiguates
// STRUCTURALLY: try each member in declaration order and emit the first that
// accepts the JS value. Two acceptance strategies are provided; each walker uses
// whichever fits its member representation:
//   - `matchesStandard` — for vendors whose union members are directly available
//     as Standard Schema values (Zod / Valibot `.options`): runs the member's
//     own `~standard.validate` synchronously.
//   - `matchesSchemaType` — a vendor-agnostic structural predicate over the
//     compiled schema-model type, for vendors whose members are internal nodes
//     (ArkType branch nodes / Effect AST members) rather than Standard Schemas.

import { FluentCodec } from './codec';
import {
  mergeGraphDefs,
  SchemaType,
  SchemaTypeDef,
  SchemaValue,
  t,
  TypeId,
  v,
  variantCase,
  VariantCaseType,
} from '../../internal/schema-model';
import { StandardSchemaV1 } from './standardSchema';

/**
 * Structural acceptance via a member's Standard Schema: runs
 * `schema['~standard'].validate(value)` and returns `true` when it completes
 * SYNCHRONOUSLY with no issues. A `Promise` result (async validation) is treated
 * as a non-match so encode disambiguation stays synchronous — matching the
 * "try each member synchronously" behaviour of effect's `Schema.Union`.
 */
export function matchesStandard(schema: StandardSchemaV1, value: unknown): boolean {
  const result = schema['~standard'].validate(value);
  if (result instanceof Promise) return false;
  return (result as { issues?: unknown }).issues === undefined;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return (
    value !== null &&
    typeof value === 'object' &&
    !Array.isArray(value) &&
    !(value instanceof Map) &&
    !(value instanceof Uint8Array)
  );
}

function isDatetimeLike(value: unknown): boolean {
  return (
    value !== null &&
    typeof value === 'object' &&
    typeof (value as { seconds?: unknown }).seconds === 'bigint'
  );
}

/**
 * Vendor-agnostic structural acceptance over a compiled schema-model type: does
 * the JS runtime shape of `value` match the WIT type kind of `type`? Used for
 * union members that are not available as Standard Schema values. `defs` resolves
 * `ref` nodes; each member's self-contained `graph.defs` is sufficient.
 */
export function matchesSchemaType(
  defs: ReadonlyMap<TypeId, SchemaTypeDef>,
  type: SchemaType,
  value: unknown,
): boolean {
  const body = type.body;
  switch (body.tag) {
    case 'ref': {
      const def = defs.get(body.id);
      return def ? matchesSchemaType(defs, def.body, value) : true;
    }
    case 'bool':
      return typeof value === 'boolean';
    case 's8':
    case 's16':
    case 's32':
    case 'u8':
    case 'u16':
    case 'u32':
    case 'f32':
    case 'f64':
      return typeof value === 'number';
    case 's64':
    case 'u64':
    case 'duration':
      return typeof value === 'bigint' || (typeof value === 'number' && Number.isInteger(value));
    case 'char':
    case 'string':
    case 'url':
      return typeof value === 'string';
    case 'enum':
      return typeof value === 'string';
    case 'binary':
      return value instanceof Uint8Array || Array.isArray(value);
    case 'list':
    case 'fixed-list':
      return Array.isArray(value);
    case 'tuple':
      return Array.isArray(value) && value.length === body.elements.length;
    case 'map':
      return value instanceof Map || isPlainObject(value);
    case 'option':
      return value === undefined || value === null || matchesSchemaType(defs, body.element, value);
    case 'datetime':
      return isDatetimeLike(value);
    case 'record':
      // A record accepts an object where every REQUIRED field (a non-`option`
      // field) is present — enough to disambiguate a union of objects with
      // distinct required keys.
      return (
        isPlainObject(value) &&
        body.fields.every((f) => f.body.body.tag === 'option' || f.name in value)
      );
    case 'secret':
    case 'quota-token':
    case 'variant':
      return value !== null && typeof value === 'object';
    default:
      // Unknown / exotic kinds: accept, deferring to declaration order.
      return true;
  }
}

function runtimeType(value: unknown): string {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  return typeof value;
}

/**
 * Build a WIT `variant` codec for a plain (non-discriminated) union: cases are
 * auto-named `case0..caseN-1`, decode is by `caseIndex`, and encode picks the
 * first member for which `pick(value)` returns its index. `pick` returns `-1`
 * when no member accepts the value, which surfaces a clear encode error.
 */
export function buildUnionVariantCodec(
  memberCodecs: readonly FluentCodec[],
  pick: (value: unknown) => number,
  label: string,
): FluentCodec {
  const defs = mergeGraphDefs(memberCodecs.map((c) => c.graph));
  const cases: VariantCaseType[] = memberCodecs.map((c, i) =>
    variantCase(`case${i}`, c.graph.root),
  );
  return {
    graph: { defs, root: t.variant(cases) },
    toValue: (value) => {
      const i = pick(value);
      if (i < 0 || i >= memberCodecs.length) {
        throw new Error(
          `No ${label} member accepts a value of runtime type '${runtimeType(value)}'. ` +
            `A plain union encodes by trying each member in declaration order; none matched.`,
        );
      }
      return v.variant(i, memberCodecs[i].toValue(value));
    },
    fromValue: (sv) => {
      const vv = sv as Extract<SchemaValue, { tag: 'variant' }>;
      return memberCodecs[vv.caseIndex].fromValue(vv.payload!);
    },
  };
}
