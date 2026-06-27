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

// The runtime value boundary. Replaces the legacy `DataValue` codec: it maps
// between the SDK's runtime parameter/return values and the schema-native
// `SchemaValue` carried across the agent boundary.
//
// Constructor/method inputs are a `SchemaValue` record whose fields correspond,
// in order, to the *user-supplied* parameters. Auto-injected parameters
// (`principal`) and `config` parameters do NOT consume a record field — they are
// injected/reconstructed at decode time, exactly mirroring the Rust SDK.

import { Principal as HostPrincipal } from 'golem:agent/common@2.0.0';
import type { SchemaValueTree } from 'golem:core/types@2.0.0';
import {
  drainUnconsumedQuotaHandles,
  preflightWitValueTree,
  SchemaValue,
  schemaValueFromWit,
  schemaValueToWit,
  v,
} from '../../schema-model';
import {
  createWireDecoder,
  createWireEncoder,
  deserializeGraph,
  deserializeGraphFromWit,
  getGraphCodec,
  serializeGraph,
  serializeGraphToWit,
} from './schemaValue';
import { r, resolvedField } from '../types/resolvedType';
import {
  MultimodalCase,
  RuntimeOutput,
  RuntimeParam,
  RuntimeTypeInfo,
} from '../../typeInfoInternal';
import type { ResolvedGraph, ResolvedType } from '../types/resolvedType';
import {
  unstructuredBinaryFromValue,
  unstructuredBinaryToValue,
  unstructuredTextFromValue,
  unstructuredTextToValue,
} from '../../schema/rich';
import { sdkPrincipalFromHost } from '../../../principal';
import { Config } from '../../../agentConfig';

// ============================================================
// Eager codec compilation (Wizer snapshot capture)
// ============================================================

// Compile the codec for a plain `schema` type's graph now, so the
// compiled function objects are reachable when `golem build`'s Wizer step
// snapshots the QuickJS heap. Called from the `@agent()` decorator, which runs
// during top-level module evaluation — the exact moment Wizer pre-initializes.
// Non-`schema` types have no compiled codec; they are skipped.
function precompileRuntimeTypeInfo(type: RuntimeTypeInfo): void {
  if (type.tag === 'schema') {
    getGraphCodec(type.graph);
  }
}

/** Eagerly compile codecs for an ordered parameter list (constructor / method input). */
export function precompileParamCodecs(params: RuntimeParam[]): void {
  for (const param of params) {
    precompileRuntimeTypeInfo(param.type);
  }
}

/** Eagerly compile the codec for a method output. */
export function precompileOutputCodec(output: RuntimeOutput): void {
  if (output.tag === 'single') {
    precompileRuntimeTypeInfo(output.type);
  }
}

// ============================================================
// Per-value serialize / deserialize
// ============================================================

export function serializeRuntimeValue(value: any, type: RuntimeTypeInfo): SchemaValue {
  switch (type.tag) {
    case 'schema':
      return serializeGraph(value, type.graph);
    case 'unstructured-text':
      return unstructuredTextToValue(value);
    case 'unstructured-binary':
      return unstructuredBinaryToValue(value);
    case 'multimodal':
      return serializeMultimodalValue(value, type.cases);
    case 'principal':
      throw new Error("Internal error: 'principal' value should never be serialized");
    case 'config':
      throw new Error("Internal error: 'config' value should never be serialized");
  }
}

export function deserializeRuntimeValue(
  parameterName: string,
  value: SchemaValue,
  type: RuntimeTypeInfo,
): any {
  switch (type.tag) {
    case 'schema':
      return deserializeGraph(value, type.graph);
    case 'unstructured-text':
      return unstructuredTextFromValue(parameterName, value, type.languages);
    case 'unstructured-binary':
      return unstructuredBinaryFromValue(parameterName, value, type.mimeTypes);
    case 'multimodal':
      return deserializeMultimodalValue(parameterName, value, type.cases);
    case 'principal':
      throw new Error("Internal error: 'principal' value should never be deserialized");
    case 'config':
      throw new Error("Internal error: 'config' value should never be deserialized");
  }
}

// ============================================================
// Multimodal value
// ============================================================

function serializeMultimodalValue(value: any, cases: MultimodalCase[]): SchemaValue {
  if (!Array.isArray(value)) {
    throw new Error('Multimodal value must be an array of tagged modality values');
  }
  const elements = value.map((elem) => {
    if (
      elem === null ||
      typeof elem !== 'object' ||
      typeof elem.tag !== 'string' ||
      !Object.prototype.hasOwnProperty.call(elem, 'val')
    ) {
      throw new Error(
        `Multimodal element must be an object with a string 'tag' and a 'val', got ${JSON.stringify(elem)}`,
      );
    }
    const caseIndex = cases.findIndex((c) => c.name === elem.tag);
    if (caseIndex < 0) {
      throw new Error(
        `Unknown multimodal modality '${elem.tag}'. Allowed: ${cases.map((c) => c.name).join(', ')}`,
      );
    }
    return v.variant(caseIndex, serializeRuntimeValue(elem.val, cases[caseIndex].type));
  });
  return v.list(elements);
}

function deserializeMultimodalValue(
  parameterName: string,
  value: SchemaValue,
  cases: MultimodalCase[],
): any[] {
  if (value.tag !== 'list') {
    throw new Error(
      `Expected list value for multimodal parameter ${parameterName}, got ${value.tag}`,
    );
  }
  return value.elements.map((elem) => {
    if (elem.tag !== 'variant') {
      throw new Error(`Expected variant element in multimodal parameter ${parameterName}`);
    }
    const c = cases[elem.caseIndex];
    if (!c) {
      throw new Error(
        `Unknown multimodal case index ${elem.caseIndex} for parameter ${parameterName}`,
      );
    }
    if (elem.payload === undefined) {
      throw new Error(
        `Missing payload for multimodal case '${c.name}' in parameter ${parameterName}`,
      );
    }
    return {
      tag: c.name,
      val: deserializeRuntimeValue(`${parameterName}.${c.name}`, elem.payload, c.type),
    };
  });
}

// ============================================================
// Input record (constructor / method input)
// ============================================================

/**
 * Decode a `SchemaValue` record into the ordered TypeScript argument list.
 * The record's fields correspond, in order, to the user-supplied parameters;
 * `principal` parameters are injected from `principal` and `config` parameters
 * are reconstructed, neither consuming a record field.
 */
export function decodeInputRecord(
  input: SchemaValue,
  params: RuntimeParam[],
  principal: HostPrincipal,
): any[] {
  if (input.tag !== 'record') {
    throw new Error(`Expected record value for agent input, got ${input.tag}`);
  }

  const fields = input.fields;
  let fieldIndex = 0;

  const args = params.map((param) => {
    const type = param.type;
    if (type.tag === 'principal') {
      return sdkPrincipalFromHost(principal);
    }
    if (type.tag === 'config') {
      return new Config(type.tsType.properties, type.tsType.requiredMembers);
    }
    if (fieldIndex >= fields.length) {
      throw new Error(`Missing argument for parameter '${param.name}'`);
    }
    return deserializeRuntimeValue(param.name, fields[fieldIndex++], type);
  });

  if (fieldIndex !== fields.length) {
    throw new Error(`Unexpected extra arguments: expected ${fieldIndex}, got ${fields.length}`);
  }

  return args;
}

/**
 * Encode an ordered list of user-supplied argument values (already aligned with
 * `userParams`, in order) into a `SchemaValue` record. Used by remote-client /
 * agent-id construction. `userParams` must contain only `schema` / rich /
 * multimodal parameters (no `principal` / `config`).
 *
 * Trailing arguments may be omitted: every user parameter always produces a
 * record field, and a missing trailing argument is encoded as `undefined`. For
 * an optional (`option`) parameter that materialises as `option none`; for a
 * required parameter it raises a type-mismatch error. This mirrors the legacy
 * SDK, which let callers omit trailing optional arguments.
 */
export function encodeInputRecord(args: any[], userParams: RuntimeParam[]): SchemaValue {
  if (args.length > userParams.length) {
    throw new Error(`Expected at most ${userParams.length} arguments, got ${args.length}`);
  }
  return v.record(
    userParams.map((param, i) =>
      serializeRuntimeValue(i < args.length ? args[i] : undefined, param.type),
    ),
  );
}

/**
 * Fused encode of an argument list directly into the flat wire
 * `schema-value-tree`, skipping the intermediate `SchemaValue` record. Semantics
 * match {@link encodeInputRecord} exactly. When every user parameter is a plain
 * `schema` value the single-pass fused encoder is used; rich / multimodal
 * parameters fall back to the two-step path (`encodeInputRecord` + wire codec).
 */
export function encodeInputRecordToWit(args: any[], userParams: RuntimeParam[]): SchemaValueTree {
  if (args.length > userParams.length) {
    throw new Error(`Expected at most ${userParams.length} arguments, got ${args.length}`);
  }

  if (!userParams.every((p) => p.type.tag === 'schema')) {
    return schemaValueToWit(encodeInputRecord(args, userParams));
  }

  if (userParams.some((p) => runtimeTypeContainsOwnedHandle(p.type))) {
    const schemaParams = userParams as RuntimeParamWithSchema[];
    return serializeGraphToWit(
      inputRecordObject(args, schemaParams),
      inputRecordGraph(schemaParams),
    );
  }

  const enc = createWireEncoder();
  const fieldIndices = userParams.map((param, i) => {
    const type = param.type as Extract<RuntimeTypeInfo, { tag: 'schema' }>;
    const value = i < args.length ? args[i] : undefined;
    const codec = getGraphCodec(type.graph);
    return codec ? codec.emit(value, enc.valueNodes) : enc.emitGraph(value, type.graph);
  });
  const root = enc.pushRecord(fieldIndices);
  return { valueNodes: enc.valueNodes, root };
}

/**
 * Fused decode of an input record `schema-value-tree` directly into the ordered
 * argument list, skipping the intermediate `SchemaValue`. Semantics match
 * {@link decodeInputRecord} exactly. The fused single-pass decoder is used when
 * every field-consuming parameter is a plain `schema` value; otherwise the
 * two-step path (`schemaValueFromWit` + `decodeInputRecord`) is used.
 */
export function decodeInputRecordFromWit(
  input: SchemaValueTree,
  params: RuntimeParam[],
  principal: HostPrincipal,
): any[] {
  preflightBoundaryDecode(input.valueNodes, input.root);

  const consumesField = (p: RuntimeParam) => p.type.tag !== 'principal' && p.type.tag !== 'config';
  const fieldParams = params.filter(consumesField);
  if (!fieldParams.every((p) => p.type.tag === 'schema')) {
    const clone = cloneWitValueTree(input);
    const result = decodeInputRecord(schemaValueFromWit(clone), params, principal);
    consumeOwnedHandlesFromClone(input, clone);
    return result;
  }

  if (fieldParams.some((p) => runtimeTypeContainsOwnedHandle(p.type))) {
    const schemaParams = fieldParams as RuntimeParamWithSchema[];
    const dec = createWireDecoder(input.valueNodes);
    const fieldIndices = dec.recordFieldIndices(input.root);
    if (fieldIndices.length < schemaParams.length) {
      throw new Error(`Missing argument for parameter '${schemaParams[fieldIndices.length].name}'`);
    }
    if (fieldIndices.length > schemaParams.length) {
      throw new Error(
        `Unexpected extra arguments: expected ${schemaParams.length}, got ${fieldIndices.length}`,
      );
    }

    const record = dec.readGraph(input.root, inputRecordGraph(schemaParams));
    let fieldIndex = 0;
    return params.map((param) => {
      const type = param.type;
      if (type.tag === 'principal') {
        return sdkPrincipalFromHost(principal);
      }
      if (type.tag === 'config') {
        return new Config(type.tsType.properties, type.tsType.requiredMembers);
      }
      return record[inputRecordFieldName(fieldIndex++)];
    });
  }

  const dec = createWireDecoder(input.valueNodes);
  const fieldIndices = dec.recordFieldIndices(input.root);
  let fieldIndex = 0;
  // Cycle guard shared across compiled-codec field reads. Allocated lazily
  // because rich/multimodal-only inputs never take the compiled path. Sibling
  // fields are independent subtrees, so a single shared guard is correct (each
  // read restores its entries to 0 on the way out).
  let onPath: Uint8Array | undefined;

  const args = params.map((param) => {
    const type = param.type;
    if (type.tag === 'principal') {
      return sdkPrincipalFromHost(principal);
    }
    if (type.tag === 'config') {
      return new Config(type.tsType.properties, type.tsType.requiredMembers);
    }
    if (fieldIndex >= fieldIndices.length) {
      throw new Error(`Missing argument for parameter '${param.name}'`);
    }
    const schemaType = type as Extract<RuntimeTypeInfo, { tag: 'schema' }>;
    const idx = fieldIndices[fieldIndex++];
    const codec = getGraphCodec(schemaType.graph);
    let result;
    if (codec) {
      if (!onPath) onPath = new Uint8Array(input.valueNodes.length);
      result = codec.read(idx, input.valueNodes, onPath);
    } else {
      result = dec.readGraph(idx, schemaType.graph);
    }
    return result;
  });

  if (fieldIndex !== fieldIndices.length) {
    throw new Error(
      `Unexpected extra arguments: expected ${fieldIndex}, got ${fieldIndices.length}`,
    );
  }

  return args;
}

type RuntimeParamWithSchema = RuntimeParam & {
  type: Extract<RuntimeTypeInfo, { tag: 'schema' }>;
};

function inputRecordObject(args: any[], userParams: RuntimeParamWithSchema[]): Record<string, any> {
  const record: Record<string, any> = {};
  for (let i = 0; i < userParams.length; i++) {
    record[inputRecordFieldName(i)] = i < args.length ? args[i] : undefined;
  }
  return record;
}

function inputRecordGraph(userParams: RuntimeParamWithSchema[]): ResolvedGraph {
  const defs = new Map<string, ResolvedType>();
  const fields = userParams.map((param, i) => {
    const prefix = `${inputRecordFieldName(i)}:`;
    for (const [id, def] of param.type.graph.defs) {
      defs.set(`${prefix}${id}`, namespaceResolvedType(def, prefix));
    }
    return resolvedField(
      inputRecordFieldName(i),
      namespaceResolvedType(param.type.graph.root, prefix),
    );
  });

  return {
    defs,
    root: r.record(fields),
  };
}

function inputRecordFieldName(index: number): string {
  return `$${index}`;
}

function namespaceResolvedType(type: ResolvedType, prefix: string): ResolvedType {
  const body = type.body;
  switch (body.tag) {
    case 'ref':
      return { ...type, body: { tag: 'ref', id: `${prefix}${body.id}` } };
    case 'list':
      return { ...type, body: { ...body, element: namespaceResolvedType(body.element, prefix) } };
    case 'map':
      return {
        ...type,
        body: {
          ...body,
          key: namespaceResolvedType(body.key, prefix),
          value: namespaceResolvedType(body.value, prefix),
        },
      };
    case 'tuple':
      return {
        ...type,
        body: {
          ...body,
          elements: body.elements.map((element) => namespaceResolvedType(element, prefix)),
        },
      };
    case 'record':
      return {
        ...type,
        body: {
          ...body,
          fields: body.fields.map((field) => ({
            ...field,
            type: namespaceResolvedType(field.type, prefix),
          })),
        },
      };
    case 'variant':
      return {
        ...type,
        body: {
          ...body,
          cases: body.cases.map((c) => ({
            ...c,
            payload: c.payload !== undefined ? namespaceResolvedType(c.payload, prefix) : undefined,
          })),
        },
      };
    case 'option':
      return { ...type, body: { ...body, element: namespaceResolvedType(body.element, prefix) } };
    case 'result':
      return {
        ...type,
        body: {
          ...body,
          ok: body.ok !== undefined ? namespaceResolvedType(body.ok, prefix) : undefined,
          err: body.err !== undefined ? namespaceResolvedType(body.err, prefix) : undefined,
        },
      };
    case 'secret':
      return { ...type, body: { ...body, inner: namespaceResolvedType(body.inner, prefix) } };
    default:
      return type;
  }
}

function runtimeTypeContainsOwnedHandle(type: RuntimeTypeInfo): boolean {
  return type.tag === 'schema' && resolvedGraphContainsOwnedHandle(type.graph);
}

function resolvedGraphContainsOwnedHandle(graph: ResolvedGraph): boolean {
  return resolvedTypeContainsOwnedHandle(graph.root, graph, new Set());
}

function resolvedTypeContainsOwnedHandle(
  type: ResolvedType,
  graph: ResolvedGraph,
  seenRefs: Set<string>,
): boolean {
  const body = type.body;
  switch (body.tag) {
    case 'secret':
    case 'quota-token':
      return true;
    case 'list':
    case 'option':
      return resolvedTypeContainsOwnedHandle(body.element, graph, seenRefs);
    case 'map':
      return (
        resolvedTypeContainsOwnedHandle(body.key, graph, seenRefs) ||
        resolvedTypeContainsOwnedHandle(body.value, graph, seenRefs)
      );
    case 'tuple':
      return body.elements.some((element) =>
        resolvedTypeContainsOwnedHandle(element, graph, seenRefs),
      );
    case 'record':
      return body.fields.some((field) =>
        resolvedTypeContainsOwnedHandle(field.type, graph, seenRefs),
      );
    case 'variant':
      return body.cases.some(
        (c) =>
          c.payload !== undefined && resolvedTypeContainsOwnedHandle(c.payload, graph, seenRefs),
      );
    case 'result':
      return (
        (body.ok !== undefined && resolvedTypeContainsOwnedHandle(body.ok, graph, seenRefs)) ||
        (body.err !== undefined && resolvedTypeContainsOwnedHandle(body.err, graph, seenRefs))
      );
    case 'ref': {
      if (seenRefs.has(body.id)) return false;
      const def = graph.defs.get(body.id);
      if (def === undefined) return false;
      seenRefs.add(body.id);
      const result = resolvedTypeContainsOwnedHandle(def, graph, seenRefs);
      seenRefs.delete(body.id);
      return result;
    }
    default:
      return false;
  }
}

// ============================================================
// Output (method return)
// ============================================================

/** Encode a method's return value per its `RuntimeOutput`. `unit` → `undefined`. */
export function encodeOutput(returnValue: any, output: RuntimeOutput): SchemaValue | undefined {
  if (output.tag === 'unit') {
    return undefined;
  }
  return serializeRuntimeValue(returnValue, output.type);
}

/** Decode a method's return `SchemaValue` per its `RuntimeOutput`. `unit` → `undefined`. */
export function decodeOutput(value: SchemaValue | undefined, output: RuntimeOutput): any {
  if (output.tag === 'unit') {
    return undefined;
  }
  if (value === undefined) {
    throw new Error('Expected a return value for a non-unit method output, got none');
  }
  return deserializeRuntimeValue('returnValue', value, output.type);
}

/**
 * Fused encode of a method return value directly into the wire
 * `schema-value-tree`. Semantics match {@link encodeOutput} + the wire codec.
 * Plain `schema` outputs use the single-pass fused encoder; rich / multimodal
 * outputs fall back to the two-step path.
 */
export function encodeOutputToWit(
  returnValue: any,
  output: RuntimeOutput,
): SchemaValueTree | undefined {
  if (output.tag === 'unit') {
    return undefined;
  }
  const type = output.type;
  if (type.tag === 'schema') {
    const codec = getGraphCodec(type.graph);
    return codec ? codec.encode(returnValue) : serializeGraphToWit(returnValue, type.graph);
  }
  return schemaValueToWit(serializeRuntimeValue(returnValue, type));
}

/**
 * Fused decode of a method return value directly from the wire
 * `schema-value-tree`. Semantics match the wire codec + {@link decodeOutput}.
 * Plain `schema` outputs use the single-pass fused decoder; rich / multimodal
 * outputs fall back to the two-step path.
 */
export function decodeOutputFromWit(
  value: SchemaValueTree | undefined,
  output: RuntimeOutput,
): any {
  if (output.tag === 'unit') {
    return undefined;
  }
  if (value === undefined) {
    throw new Error('Expected a return value for a non-unit method output, got none');
  }
  preflightBoundaryDecode(value.valueNodes, value.root);

  const type = output.type;
  if (type.tag === 'schema') {
    const codec = getGraphCodec(type.graph);
    return codec ? codec.decode(value) : deserializeGraphFromWit(value, type.graph);
  }
  const clone = cloneWitValueTree(value);
  const result = deserializeRuntimeValue('returnValue', schemaValueFromWit(clone), type);
  consumeOwnedHandlesFromClone(value, clone);
  return result;
}

function preflightBoundaryDecode(valueNodes: SchemaValueTree['valueNodes'], root: SchemaValueTree['root']): void {
  try {
    preflightWitValueTree(valueNodes, root);
  } catch (e) {
    drainUnconsumedQuotaHandles(valueNodes);
    throw e;
  }
}

export function cloneWitValueTree(value: SchemaValueTree): SchemaValueTree {
  return {
    root: value.root,
    valueNodes: value.valueNodes.map((node) => ({ ...node }) as typeof node),
  };
}

export function consumeOwnedHandlesFromClone(original: SchemaValueTree, clone: SchemaValueTree): void {
  for (let i = 0; i < clone.valueNodes.length; i++) {
    const cloned = clone.valueNodes[i] as { tag: string; val?: unknown };
    if (
      (cloned.tag === 'secret-value' || cloned.tag === 'quota-token-handle') &&
      cloned.val === undefined
    ) {
      (original.valueNodes[i] as { val?: unknown }).val = undefined;
    }
  }
}
