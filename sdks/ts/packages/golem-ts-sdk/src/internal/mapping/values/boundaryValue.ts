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
import { SchemaValue, schemaValueFromWit, schemaValueToWit, v } from '../../schema-model';
import {
  createWireDecoder,
  createWireEncoder,
  deserializeGraph,
  deserializeGraphFromWit,
  serializeGraph,
  serializeGraphToWit,
} from './schemaValue';
import {
  MultimodalCase,
  RuntimeOutput,
  RuntimeParam,
  RuntimeTypeInfo,
} from '../../typeInfoInternal';
import {
  unstructuredBinaryFromValue,
  unstructuredBinaryToValue,
  unstructuredTextFromValue,
  unstructuredTextToValue,
} from '../../schema/rich';
import { sdkPrincipalFromHost } from '../../../principal';
import { QuotaToken } from '../../../host/quota';
import { Config } from '../../../agentConfig';

// ============================================================
// Per-value serialize / deserialize
// ============================================================

export function serializeRuntimeValue(value: any, type: RuntimeTypeInfo): SchemaValue {
  switch (type.tag) {
    case 'schema': {
      const toSerialize =
        type.tsType.kind === 'quota-token' ? (value as QuotaToken)._toRecord() : value;
      return serializeGraph(toSerialize, type.graph);
    }
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
    case 'schema': {
      const result = deserializeGraph(value, type.graph);
      return type.tsType.kind === 'quota-token' ? QuotaToken._fromRecord(result) : result;
    }
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

  const enc = createWireEncoder();
  const fieldIndices = userParams.map((param, i) => {
    const type = param.type as Extract<RuntimeTypeInfo, { tag: 'schema' }>;
    const value = i < args.length ? args[i] : undefined;
    const toSerialize =
      type.tsType.kind === 'quota-token' ? (value as QuotaToken)._toRecord() : value;
    return enc.emitGraph(toSerialize, type.graph);
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
  const consumesField = (p: RuntimeParam) => p.type.tag !== 'principal' && p.type.tag !== 'config';
  if (!params.filter(consumesField).every((p) => p.type.tag === 'schema')) {
    return decodeInputRecord(schemaValueFromWit(input), params, principal);
  }

  const dec = createWireDecoder(input.valueNodes);
  const fieldIndices = dec.recordFieldIndices(input.root);
  let fieldIndex = 0;

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
    const result = dec.readGraph(fieldIndices[fieldIndex++], schemaType.graph);
    return schemaType.tsType.kind === 'quota-token' ? QuotaToken._fromRecord(result) : result;
  });

  if (fieldIndex !== fieldIndices.length) {
    throw new Error(
      `Unexpected extra arguments: expected ${fieldIndex}, got ${fieldIndices.length}`,
    );
  }

  return args;
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
    const toSerialize =
      type.tsType.kind === 'quota-token' ? (returnValue as QuotaToken)._toRecord() : returnValue;
    return serializeGraphToWit(toSerialize, type.graph);
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
  const type = output.type;
  if (type.tag === 'schema') {
    const result = deserializeGraphFromWit(value, type.graph);
    return type.tsType.kind === 'quota-token' ? QuotaToken._fromRecord(result) : result;
  }
  return deserializeRuntimeValue('returnValue', schemaValueFromWit(value), type);
}
