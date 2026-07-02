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

// Standard-Schema config surface for the fluent SDK (issue #3449). A `config`
// spec on `defineAgent` is a single record of named fields, each a Standard
// Schema value. A field marked with `s.secret(inner)` — at ANY depth — is a
// SECRET field; every other leaf is a plain LOCAL field. We flatten each field
// recursively (descending into object/record schemas) to a flat list of LEAF
// declarations, each carrying its full multi-segment `path`:
//   - local leaves declare their inner type directly,
//   - secret leaves declare `secret<inner>` (a capability node) so the host
//     knows the value is a secret and returns an opaque handle.
// At runtime the accessor reassembles the nested object from the leaves and
// fetches each leaf fresh by its full path (config may change between
// invocations): local leaves decode directly, secret leaves are surfaced as a
// lazy, log-safe {@link Secret} handle whose `.get()` reveals + decodes on
// demand (capability-gated via `golem:secrets/reveal`).
//
// Only object/record schemas are recursed; unions, arrays, tuples, maps, and
// primitives are read whole as a single leaf (matching the decorator SDK's
// flattening scope). A secret nested directly inside an array/union is out of
// scope — it is read whole with its enclosing leaf.

import { getConfigValue } from 'golem:agent/host@2.0.0';
import { AgentConfigSource } from 'golem:agent/common@2.0.0';
import { SchemaGraph, schemaGraphToWit, schemaValueFromWit } from '../internal/schema-model';
import { StandardSchemaV1 } from './schema/standardSchema';
import { compileSchema } from './schema/adapter';
import { FluentCodec } from './schema/codec';
import { Secret } from './secret';

/**
 * The fluent agent's config spec: a single record of named fields, each a
 * Standard Schema (Zod / Valibot / ArkType / Effect Schema). Mark a field (at
 * any depth) with `s.secret(inner)` to make it a secret (declared to the host as
 * `secret<inner>`); any other leaf is a plain local field.
 */
export type ConfigSpec = Record<string, StandardSchemaV1>;

/**
 * A single compiled config field. `graph` is the *declaration* graph the host
 * is told about (inner type for `local`, `secret<inner>` for `secret`); `codec`
 * is always the inner codec that drives plaintext encode/decode of the value.
 */
export interface ConfigDeclaration {
  readonly name: string;
  readonly source: AgentConfigSource;
  readonly path: string[];
  /** Inner codec: decodes the (revealed, for secrets) value tree. */
  readonly codec: FluentCodec;
  /** Declaration graph: inner for local, `secret<inner>` for secret. */
  readonly graph: SchemaGraph;
}

/**
 * Compile a {@link ConfigSpec} into a flat, ordered list of LEAF
 * {@link ConfigDeclaration}s, in declaration order, by recursively flattening
 * each top-level field:
 *  - an OBJECT schema (a WIT `record` with named fields, e.g. `z.object({...})`)
 *    is descended field-by-field, extending the `path`;
 *  - a SECRET marker (at any depth) becomes a `secret` leaf whose declaration
 *    graph is `secret<inner>` and whose `codec` is the inner (revealed) codec;
 *  - anything else (primitive, union, array, tuple, map, scalar marker) becomes
 *    a `local` leaf read whole at its full path.
 */
export function compileConfig(spec: ConfigSpec | undefined): ConfigDeclaration[] {
  const declarations: ConfigDeclaration[] = [];
  if (spec === undefined) return declarations;

  for (const [name, schema] of Object.entries(spec)) {
    flattenField(compileSchema(schema), [name], declarations);
  }

  return declarations;
}

/**
 * Recursively append leaf declarations for a compiled field `codec` at `path`.
 * Uses the vendor-agnostic {@link FluentCodec} hints: `secretInner` marks a
 * secret leaf, `fields` marks an object to descend.
 */
function flattenField(codec: FluentCodec, path: string[], out: ConfigDeclaration[]): void {
  if (codec.secretInner !== undefined) {
    // Secret leaf: the marker's own `graph` is `secret<inner>` (declared to the
    // host); its inner codec decodes the revealed plaintext.
    out.push({
      name: path[path.length - 1],
      source: 'secret',
      path: [...path],
      codec: codec.secretInner,
      graph: codec.graph,
    });
    return;
  }
  if (codec.fields !== undefined && codec.fields.length > 0) {
    for (const f of codec.fields) flattenField(f.codec, [...path, f.name], out);
    return;
  }
  // Local leaf: primitive / union / array / map / scalar marker, or an empty
  // object (read whole).
  out.push({
    name: path[path.length - 1],
    source: 'local',
    path: [...path],
    codec,
    graph: codec.graph,
  });
}

/**
 * Build the runtime config accessor: reassemble the nested object from the flat
 * leaf declarations. Intermediate path segments become nested objects; each leaf
 * becomes either a fresh-reading getter (local) or a lazy, log-safe
 * {@link Secret} handle (secret). Local getters read FRESH on every access, and
 * `Secret.get()` reveals fresh on every call (config may change between
 * invocations).
 *
 * Note: the host bindings (`getConfigValue` / `reveal`) only resolve inside the
 * Golem guest, so this accessor is wired in `initiate` and exercised at
 * invocation time — never in plain Node.
 */
export function buildConfigAccessor(
  declarations: readonly ConfigDeclaration[],
): Record<string, unknown> {
  const root: Record<string, unknown> = {};

  for (const d of declarations) {
    if (d.path.length === 0) continue;

    // Walk/create the intermediate objects along the path.
    let current = root;
    for (let i = 0; i < d.path.length - 1; i++) {
      const key = d.path[i];
      const existing = current[key];
      if (typeof existing !== 'object' || existing === null) {
        const next: Record<string, unknown> = {};
        current[key] = next;
        current = next;
      } else {
        current = existing as Record<string, unknown>;
      }
    }

    const leafKey = d.path[d.path.length - 1];
    if (d.source === 'secret') {
      // A lazy, log-safe `Secret` handle; the plaintext is only revealed by
      // `Secret.get()` (which re-reads the live value each call).
      current[leafKey] = new Secret(d);
    } else {
      Object.defineProperty(current, leafKey, {
        enumerable: true,
        configurable: true,
        get(): unknown {
          const tree = getConfigValue(d.path, schemaGraphToWit(d.graph));
          return d.codec.fromValue(schemaValueFromWit(tree));
        },
      });
    }
  }

  return root;
}
