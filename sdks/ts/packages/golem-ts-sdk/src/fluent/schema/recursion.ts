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

// Shared cycle-detection mechanism for compiling self- and mutually-recursive
// schemas (e.g. a `Tree` whose children are `Tree`s) into a `FluentCodec`.
//
// Recursion is expressed differently per vendor (`z.lazy` / `v.lazy` /
// `Schema.suspend` / an ArkType `alias` node), but every vendor's recursive
// reference resolves to a STABLE object identity — the module-level schema const
// (Zod/Valibot), the target AST (Effect), or the resolved node (ArkType). This
// one registry keys on that identity, so all four walkers reuse the same logic:
// they only need to recognise their lazy/suspend/alias node and hand its resolved
// inner schema to `compile`.
//
// A type is promoted to a named `ref` DEFINITION only when a real cycle is
// observed (its ref is actually handed out during its own body walk). A
// non-recursive schema passes through byte-for-byte unchanged (no `ref`, no
// `def`) so existing behaviour is preserved.

import { type SchemaTypeDef, type TypeId, t } from '../../internal/schema-model';
import { FluentCodec } from './codec';

// Deterministic, process-stable `type-id` allocation keyed on the recursive
// node's identity. The SAME recursive type therefore always maps to the SAME id
// across the independent compiles of an agent's constructor / method / config
// roots, so the agent-level `mergeGraphDefs` deduplicates one shared recursive
// definition instead of raising a spurious conflict on two ids for one type; two
// DIFFERENT recursive types get two ids and never collide. Monotonic (never
// random / time-based) so it is reproducible within a process run.
let idCounter = 0;
const globalIds = new WeakMap<object, TypeId>();

function stableTypeId(key: object): TypeId {
  let id = globalIds.get(key);
  if (id === undefined) {
    id = `rec:${idCounter++}`;
    globalIds.set(key, id);
  }
  return id;
}

interface CompileEntry {
  typeId: TypeId;
  /** Late-binding `ref` codec handed out when `key` is re-entered (a cycle). */
  ref: FluentCodec;
  /** Whether `ref` was ever handed out — i.e. a real cycle closed on `key`. */
  refRequested: boolean;
  /** Final codec, filled once `key`'s body walk finishes. */
  codec?: FluentCodec;
  done: boolean;
}

/**
 * A cycle-aware compilation registry, one per top-level compile so independent
 * compiles never cross-contaminate. Each walker routes a (possibly recursive)
 * schema node through {@link compile}; the first entry walks its body, and any
 * re-entry encountered during that walk short-circuits to a `ref`.
 */
export class RecursionRegistry {
  private readonly entries = new Map<object, CompileEntry>();

  /**
   * Compile `key`'s body via `build`, detecting recursion by `key` identity.
   *
   * - already DONE → return the cached codec;
   * - IN-PROGRESS (a cycle) → hand out the late-binding `ref` codec;
   * - fresh → reserve an entry (with its `ref`), walk the body, then promote to a
   *   named `ref` def IFF the ref was actually taken during the walk; otherwise
   *   return the walker's inline codec unchanged.
   */
  compile(key: object, build: () => FluentCodec): FluentCodec {
    const existing = this.entries.get(key);
    if (existing) {
      if (existing.done) return existing.codec!;
      // Re-entered while still walking `key`'s body: a genuine cycle. Hand out
      // the late-binding ref codec; its `toValue`/`fromValue` resolve to the
      // finalized codec, which exists by the time any value is encoded/decoded.
      existing.refRequested = true;
      return existing.ref;
    }

    const typeId = stableTypeId(key);
    const entry: CompileEntry = {
      typeId,
      refRequested: false,
      done: false,
      ref: makeRefCodec(typeId, () => entry.codec),
    };
    this.entries.set(key, entry);

    const inline = build();

    if (!entry.refRequested) {
      // No self-reference was ever taken: this schema is not recursive. Return it
      // exactly as the walker produced it (no `ref`, no `def`), so non-recursive
      // compilation is unchanged.
      entry.codec = inline;
      entry.done = true;
      return inline;
    }

    // A real cycle closed on `key`: promote it to a named definition and make the
    // root a `ref` to it. The definition body is the walked (inline) root, which
    // already contains `ref(typeId)` at each recursive position.
    const defs = new Map<TypeId, SchemaTypeDef>(inline.graph.defs);
    defs.set(typeId, { body: inline.graph.root });
    const recursiveCodec: FluentCodec = {
      graph: { defs, root: t.ref(typeId) },
      // The record's recursive field IS the same `ref` codec, which delegates
      // back to this codec, so nested recursion round-trips through one shared
      // encode/decode pair.
      toValue: inline.toValue,
      fromValue: inline.fromValue,
      ...(inline.fields !== undefined ? { fields: inline.fields } : {}),
    };
    entry.codec = recursiveCodec;
    entry.done = true;
    return recursiveCodec;
  }
}

/**
 * A lightweight `ref` codec whose value functions LATE-BIND to the recursive
 * type's finalized codec (available once the body walk completes). Its `graph`
 * carries no defs — the single named def lives on the promoted root codec.
 */
function makeRefCodec(typeId: TypeId, getFinal: () => FluentCodec | undefined): FluentCodec {
  const resolve = (): FluentCodec => {
    const c = getFinal();
    if (c === undefined) {
      throw new Error(
        `recursive schema codec '${typeId}' was used before its definition finished compiling`,
      );
    }
    return c;
  };
  return {
    graph: { defs: new Map(), root: t.ref(typeId) },
    toValue: (value) => resolve().toValue(value),
    fromValue: (sv) => resolve().fromValue(sv),
  };
}
