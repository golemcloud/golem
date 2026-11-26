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

// Static config fields retain their complete schema graph, including records,
// recursive references, unions and tuple positions. Defaults are canonical JSON
// metadata for the host resolver, never a fallback in the accessor.
// Objects containing explicit secret fields remain namespaces of declarations;
// their optional ancestors lift local leaves so the host can return absence.
// Capabilities inside static collections or unions are rejected.

import { getConfigValue } from 'golem:agent/host@2.0.0';
import { AgentConfigSource } from 'golem:agent/common@2.0.0';
import type { Secret as RawSecret } from 'golem:core/types@2.0.0';
import {
  SchemaGraph,
  SchemaValue,
  schemaGraphToWit,
  schemaValueFromWit,
  t,
  v,
} from './internal/schema-model';
import { StandardSchemaV1 } from './schema/standardSchema';
import { compileSchema } from './schema/adapter';
import { SchemaCodec, graphMayContainCapability } from './schema/codec';
import { canonicalConfigGraph, canonicalConfigName } from './schema/config';
import { SchemaRef } from './schema/ref';
import { Secret } from './secret';
import { reveal } from 'golem:secrets/reveal@0.1.0';
import { SECRET_INTERNAL } from './internal/schema-model/secretInternal';
import { releaseGuestSecretHandle } from './internal/schema-model/secretHandle';

/**
 * The agent's config spec: a single record of named fields, each a
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
  readonly codec: SchemaCodec;
  /** Declaration graph: inner for local, `secret<inner>` for secret. */
  readonly graph: SchemaGraph;
  /** Native static data; undefined differs from an option-none value. */
  readonly defaultValue?: SchemaValue;
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
function liftCodecToOption(inner: SchemaCodec): SchemaCodec {
  return {
    graph: { defs: inner.graph.defs, root: t.option(inner.graph.root) },
    optionKind: 'optional',
    optionInner: inner,
    toValue: (value) =>
      value === undefined ? v.option(undefined) : v.option(inner.toValue(value)),
    fromValue: (sv) => {
      const opt = (sv as Extract<SchemaValue, { tag: 'option' }>).value;
      return opt === undefined ? undefined : inner.fromValue(opt);
    },
  };
}

/** A codec is a descendable object group when it exposes `fields` and is not a secret. */
function isObjectGroup(codec: SchemaCodec): boolean {
  return (
    codec.secretInner === undefined &&
    codec.fields !== undefined &&
    codec.fields.length > 0 &&
    graphMayContainCapability(codec.graph)
  );
}

/** Whether a compiled leaf codec is itself an option (declared/derived as `option<...>`). */
function isOptionCodec(codec: SchemaCodec): boolean {
  return codec.graph.root.body.tag === 'option';
}

/** Build a single leaf declaration, lifting to `option<...>` when under an optional ancestor. */
function buildLeafDeclaration(
  codec: SchemaCodec,
  name: string,
  path: string[],
  underOptional: boolean,
): ConfigDeclaration {
  if (codec.secretInner !== undefined) {
    if (codec.configDefault !== undefined)
      throw new TypeError('Secret config cannot have a code default');
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
  const graph = canonicalConfigGraph(declCodec.graph);
  let defaultValue: SchemaValue | undefined;
  if (codec.configDefault !== undefined) {
    const ref = new SchemaRef(graph);
    const packed = ref.packJson(codec.configDefault);
    const decoded = declCodec.fromValue(packed);
    validateConfigDefault(declCodec, packed);
    const json = ref.unpackJson(packed);
    if (JSON.stringify(ref.unpackJson(declCodec.toValue(decoded))) !== JSON.stringify(json)) {
      throw new TypeError(`Static config default changes during encoding at '${path.join('.')}'`);
    }
    defaultValue = packed;
  }
  return {
    name,
    source: 'local',
    path: [...path],
    codec: declCodec,
    graph,
    defaultValue,
    required: !leafIsOptional,
  };
}

function validateConfigDefault(codec: SchemaCodec, value: SchemaValue): void {
  if (value.tag === 'option' && codec.optionInner) {
    if (value.value) validateConfigDefault(codec.optionInner, value.value);
  } else if (value.tag === 'record' && codec.fields) {
    codec.fields.forEach((field, index) => validateConfigDefault(field.codec, value.fields[index]));
  } else if (value.tag === 'tuple' && codec.tupleItems) {
    codec.tupleItems.forEach((item, index) => validateConfigDefault(item, value.elements[index]));
  } else if (value.tag === 'list' && codec.listItem) {
    value.elements.forEach((item) => validateConfigDefault(codec.listItem!, item));
  } else {
    const result = codec.sourceSchema?.['~standard'].validate(codec.fromValue(value));
    if (result instanceof Promise || result?.issues)
      throw new TypeError('Invalid static config default');
  }
  codec.validateConfigContainer?.(codec.fromValue(value));
}

/** Build a presence-tree node (leaf or group) for a compiled field `codec` at `path`. */
function buildConfigNode(
  codec: SchemaCodec,
  name: string,
  path: string[],
  underOptional: boolean,
): ConfigNode {
  if (isObjectGroup(codec)) {
    if (codec.configDefault !== undefined)
      throw new TypeError('Config containing managed fields cannot have a code default');
    const optional = codec.optionalGroup === true;
    const childUnderOptional = underOptional || optional;
    const children: ConfigNode[] = [];
    const requiredKeys: string[] = [];
    const names = new Set<string>();
    for (const f of codec.fields!) {
      const canonical = canonicalConfigName(f.name);
      if (names.has(canonical))
        throw new TypeError(`Duplicate canonical config field '${canonical}'`);
      names.add(canonical);
      const child = buildConfigNode(f.codec, f.name, [...path, canonical], childUnderOptional);
      children.push(child);
      if (isRequiredChild(child)) requiredKeys.push(f.name);
    }
    return { kind: 'group', name, optional, children, requiredKeys };
  }
  return { kind: 'leaf', decl: buildLeafDeclaration(codec, name, path, underOptional) };
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
    const names = new Set<string>();
    for (const [name, schema] of Object.entries(spec)) {
      const canonical = canonicalConfigName(name);
      if (names.has(canonical))
        throw new TypeError(`Duplicate canonical config field '${canonical}'`);
      names.add(canonical);
      const node = buildConfigNode(
        compileSchema(schema, { config: true }),
        name,
        [canonical],
        false,
      );
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

function secretLeaf(d: ConfigDeclaration): Secret<unknown> {
  const withHandle = <R>(use: (handle: RawSecret) => R): R => {
    const sv = schemaValueFromWit(getConfigValue(d.path, schemaGraphToWit(d.graph)));
    if (sv.tag !== 'secret')
      throw new Error(`Expected a secret config value at '${d.path.join('.')}', got '${sv.tag}'`);
    const raw = releaseGuestSecretHandle(SECRET_INTERNAL, sv.handle);
    if (raw === undefined)
      throw new Error(`Secret config handle at '${d.path.join('.')}' was already transferred`);
    try {
      return use(raw);
    } finally {
      (raw as RawSecret & { [Symbol.dispose](): void })[Symbol.dispose]();
    }
  };
  return new Secret(
    () =>
      withHandle((raw) =>
        d.codec.fromValue(schemaValueFromWit(reveal(raw, schemaGraphToWit(d.codec.graph)))),
      ),
    withHandle,
  );
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
      obj[d.name] = d.source === 'secret' ? secretLeaf(d) : readLocalLeaf(d);
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
        root[d.name] = secretLeaf(d);
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
