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
// spec on `defineAgent` carries named `local` and `secret` fields, each a
// Standard Schema value. We compile each field once into a `FluentCodec` plus a
// declaration `SchemaGraph`:
//   - local fields declare their inner type directly,
//   - secret fields declare `secret<inner>` (a capability node) so the host
//     knows the value is a secret and returns an opaque handle.
// At runtime the read accessor fetches each value fresh (config may change
// between invocations): local values decode directly, secret values are
// revealed (capability-gated via `golem:secrets/reveal`) then decoded.

import { getConfigValue } from 'golem:agent/host@2.0.0';
import { reveal } from 'golem:secrets/reveal@0.1.0';
import { AgentConfigSource } from 'golem:agent/common@2.0.0';
import {
  SchemaGraph,
  SchemaValue,
  schemaGraphToWit,
  schemaValueFromWit,
  t,
} from '../internal/schema-model';
import { StandardSchemaV1 } from './schema/standardSchema';
import { compileSchema } from './schema/adapter';
import { FluentCodec } from './schema/codec';

/**
 * The fluent agent's config spec: named `local` and `secret` fields. Each value
 * is a Standard Schema (Zod / Valibot / ArkType / Effect Schema). A `secret`
 * field's schema describes the *inner* (revealed) value; the SDK wraps it in a
 * `secret<inner>` declaration node automatically.
 */
export interface ConfigSpec {
  readonly local?: Record<string, StandardSchemaV1>;
  readonly secret?: Record<string, StandardSchemaV1>;
}

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
 * Compile a {@link ConfigSpec} into a flat, ordered list of
 * {@link ConfigDeclaration}s. `local` fields are emitted first (declaration
 * order), then `secret` fields. Each field's `path` is a single segment (its
 * name).
 */
export function compileConfig(spec: ConfigSpec | undefined): ConfigDeclaration[] {
  const declarations: ConfigDeclaration[] = [];
  if (spec === undefined) return declarations;

  for (const [name, schema] of Object.entries(spec.local ?? {})) {
    const codec = compileSchema(schema);
    declarations.push({
      name,
      source: 'local',
      path: [name],
      codec,
      graph: codec.graph,
    });
  }

  for (const [name, schema] of Object.entries(spec.secret ?? {})) {
    const codec = compileSchema(schema);
    // The agent registry requires a secret-typed config field to declare its
    // value type as `secret<inner>` (a capability node), not the bare inner
    // type. Wrap the inner graph's root in a `secret` node; the inner codec
    // still drives plaintext encode/decode of the revealed value.
    const graph: SchemaGraph = { ...codec.graph, root: t.secret(codec.graph.root) };
    declarations.push({
      name,
      source: 'secret',
      path: [name],
      codec,
      graph,
    });
  }

  return declarations;
}

/**
 * Build the runtime config accessor: an object with one getter per declaration.
 * Getters read FRESH on every access (config may change between invocations).
 *
 * Note: the host bindings (`getConfigValue` / `reveal`) only resolve inside the
 * Golem guest, so this accessor is wired in `initiate` and exercised at
 * invocation time — never in plain Node.
 */
export function buildConfigAccessor(
  declarations: readonly ConfigDeclaration[],
): Record<string, unknown> {
  const accessor: Record<string, unknown> = {};
  for (const d of declarations) {
    Object.defineProperty(accessor, d.name, {
      enumerable: true,
      configurable: true,
      get(): unknown {
        const tree = getConfigValue(d.path, schemaGraphToWit(d.graph));
        if (d.source !== 'secret') {
          return d.codec.fromValue(schemaValueFromWit(tree));
        }
        // A secret leaf's declared value type is `secret<inner>`: the host
        // returns an opaque secret handle, not the plaintext. Reveal it
        // (capability-gated via `golem:secrets/reveal`) against the inner-type
        // graph to obtain the inner value tree, which the inner codec decodes.
        const sv = schemaValueFromWit(tree);
        if (sv.tag !== 'secret') {
          throw new Error(
            `Expected a secret config value at '${d.path.join('.')}', got '${sv.tag}'`,
          );
        }
        const handle = (sv as Extract<SchemaValue, { tag: 'secret' }>).handle;
        const revealedTree = handle.withHandle((raw) =>
          reveal(raw, schemaGraphToWit(d.codec.graph)),
        );
        if (revealedTree === undefined) {
          throw new Error(`Secret config handle at '${d.path.join('.')}' was already transferred`);
        }
        return d.codec.fromValue(schemaValueFromWit(revealedTree));
      },
    });
  }
  return accessor;
}
