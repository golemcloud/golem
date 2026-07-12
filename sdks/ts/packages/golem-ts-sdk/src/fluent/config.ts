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
// recursively (descending into object schemas — including OPTIONAL object
// groups) to a flat list of LEAF declarations, each carrying its full
// multi-segment `path`:
//   - local leaves declare their inner type directly,
//   - secret leaves declare `secret<inner>` (a capability node) so the host
//     knows the value is a secret and returns an opaque handle.
//
// OPTIONAL object groups (`z.object({...}).optional()`) get special treatment,
// mirroring the base/decorator SDK:
//   - Every leaf UNDER an optional ancestor is declared to the host as
//     `option<leaf>` (lifted). This matters because the host TRAPS the guest
//     when `get-config-value` is called for an unset leaf whose expected type is
//     NOT an option, but returns an `option none` when the expected type IS an
//     option. Declaring under-optional leaves as options means an unset leaf
//     reads back as `undefined` instead of trapping — so a group can be probed
//     for presence without crashing.
//   - At runtime a group is PRESENT iff every REQUIRED (non-`.optional()`) child
//     — leaf or nested subgroup — resolves to a defined value; otherwise the
//     whole group reads as `undefined` (absent). An all-optional group is
//     therefore always present (`{}` when nothing is configured). Within a
//     present group an unset optional leaf reads as `undefined` (so it is
//     omitted by `JSON.stringify`).
//
// Only object schemas are recursed; unions, arrays, tuples, maps, and primitives
// are read whole as a single leaf. A secret nested directly inside an
// array/union is out of scope — it is read whole with its enclosing leaf.

import { getConfigValue } from 'golem:agent/host@2.0.0';
import { AgentConfigSource } from 'golem:agent/common@2.0.0';
import {
  SchemaGraph,
  SchemaValue,
  schemaGraphToWit,
  schemaValueFromWit,
  t,
  v,
} from '../internal/schema-model';
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
 * is told about (inner type for `local`, `secret<inner>` for `secret`,
 * `option<inner>` when the leaf sits under an optional group); `codec` is the
 * codec that decodes the value read at `path` back into a plaintext value.
 */
export interface ConfigDeclaration {
  readonly name: string;
  readonly source: AgentConfigSource;
  readonly path: string[];
  /** Inner codec: decodes the (revealed, for secrets) value tree. */
  readonly codec: FluentCodec;
  /** Declaration graph: inner for local, `secret<inner>` for secret. */
  readonly graph: SchemaGraph;
  /**
   * Whether this leaf is REQUIRED within its immediate parent group — i.e. it is
   * NOT itself `.optional()`. A required leaf that is unset makes its enclosing
   * group absent; an optional leaf never affects its group's presence.
   */
  readonly required: boolean;
}

/** A node in the config presence tree: a leaf field or a nested object group. */
export type ConfigNode = ConfigLeafNode | ConfigGroupNode;

/** A leaf field (primitive / union / array / map / secret / read-whole object). */
export interface ConfigLeafNode {
  readonly kind: 'leaf';
  readonly decl: ConfigDeclaration;
}

/** A nested object group (`z.object({...})`, optionally `.optional()`). */
export interface ConfigGroupNode {
  readonly kind: 'group';
  readonly name: string;
  /**
   * Whether this group is itself `.optional()` — i.e. it does NOT count toward
   * its PARENT group's presence. (The synthetic root group is not optional.)
   */
  readonly optional: boolean;
  /** Direct children, in declaration order. */
  readonly children: ConfigNode[];
  /**
   * Names of the direct children that are REQUIRED (each a non-`.optional()`
   * leaf or a non-optional subgroup). This group is present iff every one of
   * them resolves to a defined value.
   */
  readonly requiredKeys: string[];
}

/**
 * Wrap a codec so it is DECLARED as `option<inner>` and decodes an `option none`
 * (an unset leaf) to `undefined` / an `option some` to the inner value. Used to
 * lift a required (non-option) leaf that sits under an optional group so the
 * host returns `none` for an unset value instead of trapping.
 */
function liftCodecToOption(inner: FluentCodec): FluentCodec {
  return {
    graph: { defs: inner.graph.defs, root: t.option(inner.graph.root) },
    optionKind: 'optional',
    toValue: (value) =>
      value === undefined ? v.option(undefined) : v.option(inner.toValue(value)),
    fromValue: (sv) => {
      const opt = (sv as Extract<SchemaValue, { tag: 'option' }>).value;
      return opt === undefined ? undefined : inner.fromValue(opt);
    },
  };
}

/** A codec is a descendable object group when it exposes `fields` and is not a secret. */
function isObjectGroup(codec: FluentCodec): boolean {
  return codec.secretInner === undefined && codec.fields !== undefined && codec.fields.length > 0;
}

/** Whether a compiled leaf codec is itself an option (declared/derived as `option<...>`). */
function isOptionCodec(codec: FluentCodec): boolean {
  return codec.graph.root.body.tag === 'option';
}

/** Build a single leaf declaration, lifting to `option<...>` when under an optional ancestor. */
function buildLeafDeclaration(
  codec: FluentCodec,
  path: string[],
  underOptional: boolean,
): ConfigDeclaration {
  const name = path[path.length - 1];
  if (codec.secretInner !== undefined) {
    // Secret leaf: the marker's own `graph` is `secret<inner>` (declared to the
    // host); its inner codec decodes the revealed plaintext. Secrets are read
    // via their opaque handle (never option-lifted here); a required secret is
    // always treated as present (its handle is created without reading).
    return {
      name,
      source: 'secret',
      path: [...path],
      codec: codec.secretInner,
      graph: codec.graph,
      required: true,
    };
  }
  const leafIsOptional = isOptionCodec(codec) && codec.optionKind !== 'nullable';
  // Lift a non-option leaf that sits under an optional group so an unset value
  // reads as `option none` (=> undefined) instead of trapping the guest.
  const declCodec = underOptional && !leafIsOptional ? liftCodecToOption(codec) : codec;
  return {
    name,
    source: 'local',
    path: [...path],
    codec: declCodec,
    graph: declCodec.graph,
    required: !leafIsOptional,
  };
}

/** Build a presence-tree node (leaf or group) for a compiled field `codec` at `path`. */
function buildConfigNode(
  codec: FluentCodec,
  name: string,
  path: string[],
  underOptional: boolean,
): ConfigNode {
  if (isObjectGroup(codec)) {
    const optional = codec.optionalGroup === true;
    const childUnderOptional = underOptional || optional;
    const children: ConfigNode[] = [];
    const requiredKeys: string[] = [];
    for (const f of codec.fields!) {
      const child = buildConfigNode(f.codec, f.name, [...path, f.name], childUnderOptional);
      children.push(child);
      if (isRequiredChild(child)) requiredKeys.push(f.name);
    }
    return { kind: 'group', name, optional, children, requiredKeys };
  }
  return { kind: 'leaf', decl: buildLeafDeclaration(codec, path, underOptional) };
}

/** Whether a child node counts toward its parent group's presence. */
function isRequiredChild(node: ConfigNode): boolean {
  return node.kind === 'leaf' ? node.decl.required : !node.optional;
}

/**
 * Compile a {@link ConfigSpec} into its presence tree (a synthetic root group
 * whose children are the top-level fields). The tree drives BOTH the flat host
 * declarations (via {@link collectConfigLeaves}) and the runtime accessor (via
 * {@link buildConfigAccessor}).
 */
export function compileConfigTree(spec: ConfigSpec | undefined): ConfigGroupNode {
  const children: ConfigNode[] = [];
  const requiredKeys: string[] = [];
  if (spec !== undefined) {
    for (const [name, schema] of Object.entries(spec)) {
      const node = buildConfigNode(compileSchema(schema), name, [name], false);
      children.push(node);
      if (isRequiredChild(node)) requiredKeys.push(name);
    }
  }
  // The root group is never pruned to `undefined`; `requiredKeys` here is unused.
  return { kind: 'group', name: '', optional: false, children, requiredKeys };
}

/** Collect the flat, ordered LEAF declarations from a presence tree. */
export function collectConfigLeaves(tree: ConfigGroupNode): ConfigDeclaration[] {
  const out: ConfigDeclaration[] = [];
  const walk = (node: ConfigNode): void => {
    if (node.kind === 'leaf') out.push(node.decl);
    else for (const child of node.children) walk(child);
  };
  for (const child of tree.children) walk(child);
  return out;
}

/**
 * Compile a {@link ConfigSpec} into a flat, ordered list of LEAF
 * {@link ConfigDeclaration}s, in declaration order (used for the host
 * `agent-config-declaration`s and RPC config overrides).
 */
export function compileConfig(spec: ConfigSpec | undefined): ConfigDeclaration[] {
  return collectConfigLeaves(compileConfigTree(spec));
}

/** Read + decode a single local leaf FRESH from the host (undefined when unset/option-none). */
function readLocalLeaf(d: ConfigDeclaration): unknown {
  const tree = getConfigValue(d.path, schemaGraphToWit(d.graph));
  return d.codec.fromValue(schemaValueFromWit(tree));
}

/**
 * Materialize a group into a plain object, or `undefined` when the group is
 * ABSENT (a required child resolved to `undefined`). Local leaves are read fresh
 * here; secret leaves become lazy {@link Secret} handles; subgroups recurse.
 */
function materializeGroup(group: ConfigGroupNode): Record<string, unknown> | undefined {
  const obj: Record<string, unknown> = {};
  for (const child of group.children) {
    if (child.kind === 'leaf') {
      const d = child.decl;
      obj[d.name] = d.source === 'secret' ? new Secret(d) : readLocalLeaf(d);
    } else {
      obj[child.name] = materializeGroup(child);
    }
  }
  // Presence: any required child missing (undefined) prunes the whole group.
  for (const key of group.requiredKeys) {
    if (obj[key] === undefined) return undefined;
  }
  return obj;
}

/**
 * Build the runtime config accessor from the presence {@link ConfigGroupNode}
 * tree. Top-level local fields are fresh-reading getters; secrets are lazy
 * {@link Secret} handles; groups are getters that materialize the nested object
 * FRESH on each access — returning `undefined` when the (optional) group's
 * required children are not configured.
 *
 * Note: the host bindings (`getConfigValue` / `reveal`) only resolve inside the
 * Golem guest, so this accessor is wired in `initiate` and exercised at
 * invocation time — never in plain Node.
 */
export function buildConfigAccessor(tree: ConfigGroupNode): Record<string, unknown> {
  const root: Record<string, unknown> = {};
  for (const child of tree.children) {
    if (child.kind === 'leaf') {
      const d = child.decl;
      if (d.source === 'secret') {
        // A lazy, log-safe `Secret` handle; the plaintext is only revealed by
        // `Secret.get()` (which re-reads the live value each call).
        root[d.name] = new Secret(d);
      } else {
        Object.defineProperty(root, d.name, {
          enumerable: true,
          configurable: true,
          get: () => readLocalLeaf(d),
        });
      }
    } else {
      const group = child;
      Object.defineProperty(root, group.name, {
        enumerable: true,
        configurable: true,
        get: () => materializeGroup(group),
      });
    }
  }
  return root;
}
