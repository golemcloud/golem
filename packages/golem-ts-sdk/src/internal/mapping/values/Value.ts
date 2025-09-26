// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import { WitNode, WitValue } from 'golem:rpc/types@0.2.2';

import { Type } from '@golemcloud/golem-ts-types-core';
import * as Option from '../../../newTypes/option';
import {
  missingObjectKey,
  typeMismatchInSerialize,
  typeMismatchInDeserialize,
  unhandledTypeError,
  unionTypeMatchError,
  enumMismatchInSerialize,
} from './errors';
import * as Either from '../../../newTypes/either';
import {
  getTaggedUnion,
  getUnionOfLiterals,
  TaggedUnion,
} from '../types/taggedUnion';
import {
  AnalysedType,
  NameOptionTypePair,
  NameTypePair,
} from '../types/AnalysedType';
import { ts } from 'ts-morph';

export type Value =
  | {
      kind: 'bool';
      value: boolean;
    }
  | {
      kind: 'u8';
      value: number;
    }
  | {
      kind: 'u16';
      value: number;
    }
  | {
      kind: 'u32';
      value: number;
    }
  | {
      kind: 'u64';
      value: bigint;
    }
  | {
      kind: 's8';
      value: number;
    }
  | {
      kind: 's16';
      value: number;
    }
  | {
      kind: 's32';
      value: number;
    }
  | {
      kind: 's64';
      value: bigint;
    }
  | {
      kind: 'f32';
      value: number;
    }
  | {
      kind: 'f64';
      value: number;
    }
  | {
      kind: 'char';
      value: string;
    }
  | {
      kind: 'string';
      value: string;
    }
  | {
      kind: 'list';
      value: Value[];
    }
  | {
      kind: 'tuple';
      value: Value[];
    }
  | {
      kind: 'record';
      value: Value[];
    }
  | {
      kind: 'variant';
      caseIdx: number;
      caseValue?: Value;
    }
  | {
      kind: 'enum';
      value: number;
    }
  | {
      kind: 'flags';
      value: boolean[];
    }
  | {
      kind: 'option';
      value?: Value;
    }
  | {
      kind: 'result';
      value: {
        ok?: Value;
        err?: Value;
      };
    }
  | {
      kind: 'handle';
      uri: string;
      resourceId: bigint;
    };

export function fromWitValue(wit: WitValue): Value {
  if (!wit.nodes.length) throw new Error('Empty nodes in WitValue');
  return buildTree(wit.nodes[0], wit.nodes);
}

function buildTree(node: WitNode, nodes: WitNode[]): Value {
  switch (node.tag) {
    case 'record-value':
      return {
        kind: 'record',
        value: node.val.map((idx) => buildTree(nodes[idx], nodes)),
      };

    case 'variant-value': {
      const [caseIdx, maybeIndex] = node.val;
      if (maybeIndex !== undefined) {
        return {
          kind: 'variant',
          caseIdx,
          caseValue: buildTree(nodes[maybeIndex], nodes),
        };
      } else {
        return {
          kind: 'variant',
          caseIdx,
          caseValue: undefined,
        };
      }
    }

    case 'enum-value':
      return {
        kind: 'enum',
        value: node.val,
      };

    case 'flags-value':
      return {
        kind: 'flags',
        value: node.val,
      };

    case 'tuple-value':
      return {
        kind: 'tuple',
        value: node.val.map((idx) => buildTree(nodes[idx], nodes)),
      };

    case 'list-value':
      return {
        kind: 'list',
        value: node.val.map((idx) => buildTree(nodes[idx], nodes)),
      };

    case 'option-value':
      if (node.val === undefined) {
        return {
          kind: 'option',
          value: undefined,
        };
      }
      return {
        kind: 'option',
        value: buildTree(nodes[node.val], nodes),
      };

    case 'result-value': {
      const res = node.val;
      if (res.tag === 'ok') {
        return {
          kind: 'result',
          value: {
            ok:
              res.val !== undefined
                ? buildTree(nodes[res.val], nodes)
                : undefined,
          },
        };
      } else {
        return {
          kind: 'result',
          value: {
            err:
              res.val !== undefined
                ? buildTree(nodes[res.val], nodes)
                : undefined,
          },
        };
      }
    }

    case 'prim-u8':
      return {
        kind: 'u8',
        value: node.val,
      };
    case 'prim-u16':
      return {
        kind: 'u16',
        value: node.val,
      };
    case 'prim-u32':
      return {
        kind: 'u32',
        value: node.val,
      };
    case 'prim-u64':
      return {
        kind: 'u64',
        value: node.val,
      };
    case 'prim-s8':
      return {
        kind: 's8',
        value: node.val,
      };
    case 'prim-s16':
      return {
        kind: 's16',
        value: node.val,
      };
    case 'prim-s32':
      return {
        kind: 's32',
        value: node.val,
      };
    case 'prim-s64':
      return {
        kind: 's64',
        value: node.val,
      };
    case 'prim-float32':
      return {
        kind: 'f32',
        value: node.val,
      };
    case 'prim-float64':
      return {
        kind: 'f64',
        value: node.val,
      };
    case 'prim-char':
      return {
        kind: 'char',
        value: node.val,
      };
    case 'prim-bool':
      return {
        kind: 'bool',
        value: node.val,
      };
    case 'prim-string':
      return {
        kind: 'string',
        value: node.val,
      };

    case 'handle': {
      const [uri, resourceId] = node.val;
      return {
        kind: 'handle',
        uri: uri.value,
        resourceId,
      };
    }

    default:
      throw new Error(`Unhandled tag: ${(node as any).tag}`);
  }
}

export function toWitValue(value: Value): WitValue {
  const nodes: WitNode[] = [];
  buildNodes(value, nodes);
  return { nodes };
}

function buildNodes(value: Value, nodes: WitNode[]): number {
  const idx = nodes.length;
  nodes.push({
    tag: 'placeholder',
    val: undefined,
  } as any);

  switch (value.kind) {
    case 'record': {
      const recordIndices = value.value.map((v) => buildNodes(v, nodes));
      nodes[idx] = {
        tag: 'record-value',
        val: recordIndices,
      };
      return idx;
    }

    case 'variant': {
      if (value.caseValue !== undefined) {
        const innerIdx = buildNodes(value.caseValue, nodes);
        nodes[idx] = {
          tag: 'variant-value',
          val: [value.caseIdx, innerIdx],
        };
      } else {
        nodes[idx] = {
          tag: 'variant-value',
          val: [value.caseIdx, undefined],
        };
      }
      return idx;
    }

    case 'enum':
      nodes[idx] = {
        tag: 'enum-value',
        val: value.value,
      };
      return idx;

    case 'flags':
      nodes[idx] = {
        tag: 'flags-value',
        val: value.value,
      };
      return idx;

    case 'tuple': {
      const tupleIndices = value.value.map((v) => buildNodes(v, nodes));
      nodes[idx] = {
        tag: 'tuple-value',
        val: tupleIndices,
      };
      return idx;
    }

    case 'list': {
      const listIndices = value.value.map((v) => buildNodes(v, nodes));
      nodes[idx] = {
        tag: 'list-value',
        val: listIndices,
      };
      return idx;
    }

    case 'option': {
      if (value.value !== undefined) {
        const innerIdx = buildNodes(value.value, nodes);
        nodes[idx] = {
          tag: 'option-value',
          val: innerIdx,
        };
      } else {
        nodes[idx] = {
          tag: 'option-value',
          val: undefined,
        };
      }
      return idx;
    }

    case 'result': {
      if ('ok' in value.value) {
        const innerIdx =
          value.value.ok !== undefined
            ? buildNodes(value.value.ok, nodes)
            : undefined;
        nodes[idx] = {
          tag: 'result-value',
          val: {
            tag: 'ok',
            val: innerIdx,
          },
        };
      } else {
        const innerIdx =
          value.value.err !== undefined
            ? buildNodes(value.value.err, nodes)
            : undefined;
        nodes[idx] = {
          tag: 'result-value',
          val: {
            tag: 'err',
            val: innerIdx,
          },
        };
      }
      return idx;
    }

    case 'u8':
      nodes[idx] = {
        tag: 'prim-u8',
        val: value.value,
      };
      return idx;
    case 'u16':
      nodes[idx] = {
        tag: 'prim-u16',
        val: value.value,
      };
      return idx;
    case 'u32':
      nodes[idx] = {
        tag: 'prim-u32',
        val: value.value,
      };
      return idx;
    case 'u64':
      nodes[idx] = {
        tag: 'prim-u64',
        val: value.value,
      };
      return idx;
    case 's8':
      nodes[idx] = {
        tag: 'prim-s8',
        val: value.value,
      };
      return idx;
    case 's16':
      nodes[idx] = {
        tag: 'prim-s16',
        val: value.value,
      };
      return idx;
    case 's32':
      nodes[idx] = {
        tag: 'prim-s32',
        val: value.value,
      };
      return idx;
    case 's64':
      nodes[idx] = {
        tag: 'prim-s64',
        val: value.value,
      };
      return idx;
    case 'f32':
      nodes[idx] = {
        tag: 'prim-float32',
        val: value.value,
      };
      return idx;
    case 'f64':
      nodes[idx] = {
        tag: 'prim-float64',
        val: value.value,
      };
      return idx;
    case 'char':
      nodes[idx] = {
        tag: 'prim-char',
        val: value.value,
      };
      return idx;
    case 'bool':
      nodes[idx] = {
        tag: 'prim-bool',
        val: value.value,
      };
      return idx;
    case 'string':
      nodes[idx] = {
        tag: 'prim-string',
        val: value.value,
      };
      return idx;

    case 'handle':
      nodes[idx] = {
        tag: 'handle',
        val: [
          {
            value: value.uri,
          },
          value.resourceId,
        ],
      };
      return idx;

    default:
      throw new Error(`Unhandled kind: ${(value as any).kind}`);
  }
}

// Note that we take `type: Type` instead of `type: AnalysedType`(because at this point `AnalysedType` of the `tsValue` is also available)
// as `Type` holds more information, and can be used to determine the error messages for wrong `tsValue` more accurately.
export function fromTsValue(
  tsValue: any,
  analysedType: AnalysedType,
): Either.Either<Value, string> {
  return fromTsValueInternal(tsValue, analysedType);
}

function fromTsValueInternal(
  tsValue: any,
  analysedType: AnalysedType,
): Either.Either<Value, string> {
  switch (analysedType.kind) {
    case 'flags':
      return Either.left(
        unhandledTypeError(tsValue, Option.some('flags'), Option.none()),
      );
    case 'chr':
      return Either.left(
        unhandledTypeError(tsValue, Option.some('char'), Option.none()),
      );
    case 'f32':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'f32',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }
    case 's64':
      if (typeof tsValue === 'bigint') {
        return Either.right({
          kind: 's64',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }
    case 'u32':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'u32',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }
    case 's32':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 's32',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }
    case 'u16':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'u16',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }
    case 's16':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 's16',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }
    case 'u8':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'u8',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }
    case 's8':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 's8',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }
    case 'handle':
      return Either.left(
        unhandledTypeError(tsValue, Option.some('handle'), Option.none()),
      );
    case 'bool':
      return handleBooleanType(tsValue);

    case 'f64':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'f64',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }

    case 'u64':
      if (typeof tsValue === 'bigint' || typeof tsValue === 'number') {
        return Either.right({
          kind: 'u64',
          value: tsValue as any,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }

    case 'string':
      if (typeof tsValue === 'string') {
        return Either.right({
          kind: 'string',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }

    case 'option':
      const innerType = analysedType.value.inner;

      if (tsValue === null || tsValue === undefined) {
        return Either.right({
          kind: 'option',
        });
      } else {
        return Either.map(fromTsValue(tsValue, innerType), (v) => ({
          kind: 'option',
          value: v,
        }));
      }

    case 'list':
      const innerListType = analysedType.value.inner;

      if (Array.isArray(tsValue)) {
        return Either.map(
          Either.all(tsValue.map((item) => fromTsValue(item, innerListType))),
          (values) => ({
            kind: 'list',
            value: values,
          }),
        );
      }

      // If not an array, it can also be a map
      if (tsValue instanceof Map) {
        if (
          !innerListType ||
          innerListType.kind !== 'tuple' ||
          innerListType.value.items.length !== 2
        ) {
          return Either.left(typeMismatchInSerialize(tsValue, analysedType));
        }

        const keyType = innerListType.value.items[0];

        const valueType = innerListType.value.items[1];

        return handleKeyValuePairs(tsValue, innerListType, keyType, valueType);
      }

      return Either.left(typeMismatchInSerialize(tsValue, analysedType));

    case 'tuple':
      const analysedTypeTupleElems = analysedType.value.items;

      if (analysedTypeTupleElems.length === 0) {
        if (tsValue === null || tsValue === undefined) {
          return Either.right({
            kind: 'tuple',
            value: [],
          });
        } else {
          return Either.left(typeMismatchInSerialize(tsValue, analysedType));
        }
      }

      return handleTupleType(tsValue, analysedTypeTupleElems);

    case 'variant':
      const variantTypes = analysedType.value.cases;
      const isTaggedType = analysedType.taggedTypes;

      return handleVariant(tsValue, isTaggedType, variantTypes);

    case 'enum':
      if (
        typeof tsValue === 'string' &&
        analysedType.value.cases.includes(tsValue.toString())
      ) {
        const value: Value = {
          kind: 'enum',
          value: analysedType.value.cases.indexOf(tsValue.toString()),
        };

        return Either.right(value);
      } else {
        return Either.left(
          enumMismatchInSerialize(analysedType.value.cases, tsValue),
        );
      }

    case 'record':
      const nameTypePairs = analysedType.value.fields;

      return handleObject(tsValue, analysedType, nameTypePairs);

    case 'result':
      const okType = analysedType.value.ok;
      const errType = analysedType.value.err;

      if (typeof tsValue === 'object' && tsValue !== null && 'ok' in tsValue) {
        if (okType) {
          return Either.map(fromTsValue(tsValue.ok, okType), (v) => ({
            kind: 'result',
            value: {
              ok: v,
            },
          }));
        }

        return Either.right({
          kind: 'result',
          value: {
            ok: undefined,
          },
        });
      } else if (
        typeof tsValue === 'object' &&
        tsValue !== null &&
        'err' in tsValue
      ) {
        if (errType) {
          return Either.map(fromTsValue(tsValue.err, errType), (v) => ({
            kind: 'result',
            value: {
              err: v,
            },
          }));
        }

        return Either.right({
          kind: 'result',
          value: {
            err: undefined,
          },
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }
  }
}

function handleBooleanType(tsValue: any): Either.Either<Value, string> {
  if (typeof tsValue === 'boolean') {
    return Either.right({
      kind: 'bool',
      value: tsValue,
    });
  } else {
    return Either.left(
      typeMismatchInSerialize(tsValue, {
        kind: 'bool',
      }),
    );
  }
}

function handleTupleType(
  tsValue: any,
  analysedTypes: AnalysedType[],
): Either.Either<Value, string> {
  if (!Array.isArray(tsValue)) {
    return Either.left(
      typeMismatchInSerialize(tsValue, {
        kind: 'tuple',
        value: {
          name: undefined,
          owner: undefined,
          items: analysedTypes,
        },
      }),
    );
  }

  return Either.map(
    Either.all(
      tsValue.map((item, idx) => fromTsValue(item, analysedTypes[idx])),
    ),
    (values) => ({
      kind: 'tuple',
      value: values,
    }),
  );
}

function handleKeyValuePairs(
  tsValue: any,
  analysedType: AnalysedType,
  keyAnalysedType: AnalysedType,
  valueAnalysedType: AnalysedType,
): Either.Either<Value, string> {
  if (!(tsValue instanceof Map)) {
    return Either.left(typeMismatchInSerialize(tsValue, analysedType));
  }

  const values = Either.all(
    Array.from(tsValue.entries()).map(([key, value]) =>
      Either.zipWith(
        fromTsValue(key, keyAnalysedType),
        fromTsValue(value, valueAnalysedType),
        (k, v) =>
          ({
            kind: 'tuple',
            value: [k, v],
          }) as Value,
      ),
    ),
  );

  return Either.map(values, (value) => ({
    kind: 'list',
    value,
  }));
}

function handleObject(
  tsValue: any,
  analysedType: AnalysedType,
  nameTypePairs: NameTypePair[],
): Either.Either<Value, string> {
  if (typeof tsValue !== 'object' || tsValue === null) {
    return Either.left(typeMismatchInSerialize(tsValue, analysedType));
  }
  const values: Value[] = [];

  for (const prop of nameTypePairs) {
    const key = prop.name;

    const type = prop.typ;

    if (!Object.prototype.hasOwnProperty.call(tsValue, key)) {
      if (tsValue === '' && type.kind === 'string') {
        values.push({
          kind: 'string',
          value: '',
        });
      }

      if (tsValue === '0' && type.kind === 'f64') {
        values.push({
          kind: 'f64',
          value: 0,
        });
      }

      if (tsValue === '0' && type.kind === 'u64') {
        values.push({
          kind: 'u64',
          value: 0n,
        });
      }

      if (tsValue === false && type.kind === 'bool') {
        values.push({
          kind: 'bool',
          value: false,
        });
      }

      if (type.kind === 'option') {
        values.push({
          kind: 'option',
        });
        continue;
      }
    }

    const nameTypePair = nameTypePairs.find((nt) => nt.name === key);

    if (!nameTypePair) {
      return Either.left(typeMismatchInSerialize(tsValue, type));
    }

    const fieldVal = fromTsValue(tsValue[key], nameTypePair.typ);

    if (Either.isLeft(fieldVal)) {
      return Either.left(fieldVal.val);
    }

    values.push(fieldVal.val);
  }

  return Either.right({
    kind: 'record',
    value: values,
  });
}

function handleVariant(
  tsValue: any,
  isTaggedType: boolean,
  nameOptionTypePairs: NameOptionTypePair[],
): Either.Either<Value, string> {
  if (isTaggedType) {
    return handleTaggedTypedUnion(tsValue, nameOptionTypePairs);
  }

  for (const variant of nameOptionTypePairs) {
    const analysedType = variant.typ;

    if (!analysedType) {
      if (tsValue === variant.name) {
        const value: Value = {
          kind: 'variant',
          caseIdx: nameOptionTypePairs.findIndex(
            (v) => v.name === variant.name,
          ),
        };

        return Either.right(value);
      }

      continue;
    }

    const matches = matchesType(tsValue, analysedType);

    const index = nameOptionTypePairs.findIndex((v) => v.name === variant.name);

    if (matches) {
      const value: Value = {
        kind: 'variant',
        caseIdx: index,
        caseValue: Either.getOrThrowWith(
          fromTsValue(tsValue, analysedType),
          (error) => new Error(`Internal Error: ${error}`),
        ),
      };

      return Either.right(value);
    }
  }

  return Either.left(unionTypeMatchError(nameOptionTypePairs, tsValue));
}

function handleTaggedTypedUnion(
  tsValue: any,
  nameOptionTypePairs: NameOptionTypePair[],
): Either.Either<Value, string> {
  const keys = Object.keys(tsValue);

  if (!keys.includes('tag')) {
    return Either.left(missingObjectKey('tag', tsValue));
  }

  if (typeof tsValue !== 'object' || tsValue === null) {
    return Either.left(
      typeMismatchInSerialize(tsValue, {
        taggedTypes: true,
        kind: 'variant',
        value: {
          cases: nameOptionTypePairs,
          name: undefined,
          owner: undefined,
        },
      }),
    );
  }

  for (const nameOptionTypePair of nameOptionTypePairs) {
    const typeName = nameOptionTypePair.name;

    const typeOption = nameOptionTypePair.typ;

    if (tsValue['tag'] === typeName) {
      // Handle only tag names
      if (!typeOption) {
        const value: Value = {
          kind: 'variant',
          caseIdx: nameOptionTypePairs.findIndex((v) => v.name === typeName),
        };

        return Either.right(value);
      }

      // There is no type involved
      const valueKey = keys.find((k) => k !== 'tag');

      if (!valueKey) {
        return Either.left(`Missing value correspond to the tag ${typeName}`);
      }

      const innerValue = fromTsValue(tsValue[valueKey], typeOption);

      return Either.map(innerValue, (result) => ({
        kind: 'variant',
        caseIdx: nameOptionTypePairs.findIndex((v) => v.name === typeName),
        caseValue: result,
      }));
    }
  }

  return Either.left(unionTypeMatchError(nameOptionTypePairs, tsValue));
}

function matchesType(value: any, type: AnalysedType): boolean {
  switch (type.kind) {
    case 'bool':
      return typeof value === 'boolean';

    case 'f64':
      return typeof value === 'number' || typeof value === 'bigint';

    case 'f32':
      return typeof value === 'number';

    case 's64':
      return typeof value === 'number';

    case 's32':
      return typeof value === 'number';

    case 's16':
      return typeof value === 'number';

    case 's8':
      return typeof value === 'number';

    case 'u64':
      return typeof value === 'number';

    case 'u32':
      return typeof value === 'number';

    case 'u16':
      return typeof value === 'number';

    case 'u8':
      return typeof value === 'number';

    case 'string':
      return typeof value === 'string';

    case 'option':
      return (
        value === undefined ||
        value === null ||
        matchesType(value, type.value.inner)
      );

    case 'list':
      const elemType = type.value.inner;
      const result = matchesArray(value, elemType);

      if (result) {
        return true;
      }

      // It indicates a map then
      if (elemType.kind === 'tuple' && elemType.value.items.length === 2) {
        if (value instanceof Map) {
          return Array.from(value.entries()).every(
            ([k, v]) =>
              matchesType(k, elemType.value.items[0]) &&
              matchesType(v, elemType.value.items[1]),
          );
        }
      }

      return false;

    case 'tuple':
      return matchesTuple(value, type.value.items);

    case 'result':
      if (typeof value !== 'object' || value === null) return false;

      if ('ok' in value) {
        if (value['ok'] === undefined || value['ok'] === null) {
          return type.value.ok === undefined;
        }
        if (!type.value.ok) return false;
        return matchesType(value['ok'], type.value.ok);
      } else if ('err' in value) {
        if (value['err'] === undefined || value['err'] === null) {
          return type.value.err === undefined;
        }
        if (!type.value.err) return false;
        return matchesType(value['err'], type.value.err);
      } else {
        return false;
      }

    case 'enum':
      return (
        typeof value === 'string' && type.value.cases.includes(value.toString())
      );

    // A variant can be tagged union or simple union
    case 'variant':
      const nameAndOptions = type.value.cases;

      // There are two cases, if they are tagged types, or not
      if (typeof value === 'object') {
        const keys = Object.keys(value);

        if (keys.includes('tag')) {
          const tagValue = value['tag'];

          if (typeof tagValue === 'string') {
            const valueType = nameAndOptions.find(
              (nameType) => nameType.name === tagValue.toString(),
            );

            if (!valueType) {
              return false;
            }

            const type = valueType.typ;

            if (!type) {
              return keys.length === 1;
            }

            const valueKey = keys.find((k) => k !== 'tag');

            if (!valueKey) {
              return false;
            }

            return matchesType(value[valueKey], type);
          }
        }
      }

      for (const unionType of nameAndOptions) {
        const type = unionType.typ;
        const name = unionType.name;

        if (!type) {
          if (typeof value === 'string' && value === name) {
            return true;
          }
          continue;
        }

        // we don't care the name otherwise as they may be generated names

        const result = matchesType(value.type, value.value);

        if (result) {
          return true;
        }
      }

      return false;

    // A record in analysed type can correspond to map or object
    // or interface
    case 'record':
      // try handle-object-match
      return handleObjectMatch(value, type.value.fields);

    case 'flags':
      return false;
    case 'chr':
      return false;
    case 'handle':
      return false;
  }
}

function matchesTuple(
  value: any,
  tupleTypes: readonly AnalysedType[] | undefined,
): boolean {
  if (!Array.isArray(value)) return false;
  if (!tupleTypes) return false;
  if (value.length !== tupleTypes.length) return false;

  return value.every((v, idx) => matchesType(v, tupleTypes[idx]));
}

function matchesArray(value: any, elementType: AnalysedType): boolean {
  if (!Array.isArray(value)) return false;
  return value.every((item) => matchesType(item, elementType));
}

function handleObjectMatch(value: any, props: NameTypePair[]): boolean {
  if (typeof value !== 'object' || value !== 'interface' || value === null)
    return false;

  const valueKeys = Object.keys(value);
  if (valueKeys.length !== props.length) return false;

  for (const prop of props) {
    const propName = prop.name;
    const propType = prop.typ; // analysed type record has to keep track of whether it's question mark or not
    const hasKey = Object.prototype.hasOwnProperty.call(value, propName);

    let isOptional = propType.kind === 'option';

    if (!hasKey) {
      if (!isOptional) return false;
    } else {
      if (!matchesType(value[propName], propType)) return false;
    }
  }

  return true;
}

export function toTsValue(value: Value, type: Type.Type): any {
  const name = type.name;

  if (
    value.kind === 'record' &&
    value.value.length === 0 &&
    type.kind === 'null'
  ) {
    return null;
  }

  if (
    value.kind === 'record' &&
    value.value.length === 0 &&
    type.kind === 'undefined'
  ) {
    return undefined;
  }

  if (
    value.kind === 'record' &&
    value.value.length === 0 &&
    type.kind === 'void'
  ) {
    return undefined;
  }

  if (value.kind === 'option') {
    const caseValue = value.value;
    if (!caseValue) {
      // Select between undefined and null

      if (type.kind === 'null') return null;
      if (type.kind === 'undefined' || type.kind === 'void') return undefined;
      if (type.kind === 'union') {
        const unionKinds = type.unionTypes.map((t) => t.kind);

        if (unionKinds.includes('null')) {
          return null;
        }

        if (unionKinds.includes('undefined')) {
          return undefined;
        }
      }

      return undefined;
    }

    return toTsValue(caseValue, type);
  }

  if (value.kind === 'enum' && type.kind === 'union') {
    const unionOfLiterals = Either.getOrThrowWith(
      getUnionOfLiterals(type.unionTypes),
      (error) => new Error(`Internal Error: ${error}`),
    );

    if (Option.isSome(unionOfLiterals)) {
      return unionOfLiterals.val.literals[value.value];
    } else {
      throw new Error(typeMismatchInDeserialize(value, 'enum'));
    }
  }

  if (value.kind === 'result' && type.kind === 'union') {
    const taggedUnion = Either.getOrThrowWith(
      getTaggedUnion(type.unionTypes),
      (error) => new Error(`Internal Error: ${error}`),
    );

    if (Option.isSome(taggedUnion)) {
      const tags = TaggedUnion.getTaggedTypes(taggedUnion.val);

      const okOrErrTag = 'ok' in value.value ? 'ok' : 'err';

      const taggedTypeMetadata = tags.find(
        (tag) => tag.tagLiteralName === okOrErrTag,
      );

      if (!taggedTypeMetadata) {
        throw new Error(typeMismatchInDeserialize(value, 'result'));
      }

      const tagName = taggedTypeMetadata.tagLiteralName;
      const innerType = taggedTypeMetadata.valueType;

      if (innerType.tag === 'some') {
        if (!value.value) {
          if (innerType.val[1].optional) {
            return { tag: tagName };
          }

          throw new Error(
            `Expected value for the tag 1 '${tagName}' of the union type '${name}'`,
          );
        }

        const okOrErrValue = value.value[okOrErrTag];

        if (!okOrErrValue) {
          if (innerType.val[1].optional) {
            return { tag: tagName };
          }

          throw new Error(
            `Expected value for the tag 2 '${tagName}' of the union type '${name}'`,
          );
        }

        return {
          tag: tagName,
          [innerType.val[0]]: toTsValue(okOrErrValue, innerType.val[1]),
        };
      }

      return {
        tag: tagName,
      };
    }
  }

  if (value.kind === 'variant' && type.kind === 'union') {
    const taggedUnion = Either.getOrThrowWith(
      getTaggedUnion(type.unionTypes),
      (error) => new Error(`Internal Error: ${error}`),
    );

    if (Option.isSome(taggedUnion)) {
      const tags = TaggedUnion.getTaggedTypes(taggedUnion.val);

      const taggedTypeMetadata = tags[value.caseIdx];

      const tagName = taggedTypeMetadata.tagLiteralName;
      const innerType = taggedTypeMetadata.valueType;

      if (innerType.tag === 'some') {
        if (!value.caseValue) {
          if (innerType.val[1].optional) {
            return { tag: tagName };
          }

          throw new Error(
            `Expected value for the tag 3 '${tagName}' of the union type '${name}'`,
          );
        }

        return {
          tag: tagName,
          [innerType.val[0]]: toTsValue(value.caseValue, innerType.val[1]),
        };
      }

      return {
        tag: tagName,
      };
    }
  }

  switch (type.kind) {
    case 'boolean':
      if (value.kind === 'bool') {
        return value.value;
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'boolean'));
      }

    case 'number':
      return convertToNumber(value);

    case 'string':
      if (value.kind === 'string') {
        return value.value;
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'string'));
      }

    case 'bigint':
      return convertToBigInt(value);

    // This shouldn't happen as null would be always value.kind === optional
    case 'null':
      if (value.kind === 'tuple' && value.value.length === 0) {
        return null;
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'null'));
      }

    // This shouldn't happen as optional would be always value.kind === optional
    case 'undefined':
      if (value.kind === 'tuple' && value.value.length === 0) {
        return undefined;
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'undefined'));
      }

    case 'array':
      switch (type.name) {
        case 'Uint8Array':
          if (value.kind === 'list') {
            return new Uint8Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'Uint8Array'));
          }
        case 'Uint8ClampedArray':
          if (value.kind === 'list') {
            return new Uint8ClampedArray(
              value.value.map((v) => convertToNumber(v)),
            );
          } else {
            throw new Error(
              typeMismatchInDeserialize(value, 'Uint8ClampedArray'),
            );
          }
        case 'Int8Array':
          if (value.kind === 'list') {
            return new Int8Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'Int8Array'));
          }

        case 'Int16Array':
          if (value.kind === 'list') {
            return new Int16Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'Int16Array'));
          }
        case 'Uint16Array':
          if (value.kind === 'list') {
            return new Uint16Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'Uint16Array'));
          }
        case 'Int32Array':
          if (value.kind === 'list') {
            return new Int32Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'Int32Array'));
          }
        case 'Uint32Array':
          if (value.kind === 'list') {
            return new Uint32Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'Uint32Array'));
          }
        case 'Float32Array':
          if (value.kind === 'list') {
            return new Float32Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'Float32Array'));
          }
        case 'Float64Array':
          if (value.kind === 'list') {
            return new Float64Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'Float64Array'));
          }
        case 'BigInt64Array':
          if (value.kind === 'list') {
            return new BigInt64Array(
              value.value.map((v) => convertToBigInt(v)),
            );
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'BigInt64Array'));
          }
        case 'BigUint64Array':
          if (value.kind === 'list') {
            return new BigUint64Array(
              value.value.map((v) => convertToBigInt(v)),
            );
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'BigUint64Array'));
          }
      }

      if (value.kind === 'list') {
        const elemType = type.element;

        if (!elemType) {
          throw new Error(`Unable to infer the type of Array`);
        }
        return value.value.map((item: Value) => toTsValue(item, elemType));
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'array'));
      }

    case 'tuple':
      const typeArg = type.elements;
      if (value.kind === 'tuple') {
        return value.value.map((item: Value, idx: number) =>
          toTsValue(item, typeArg[idx]),
        );
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'tuple'));
      }

    case 'union':
      const unionOfLiterals = Either.getOrThrowWith(
        getUnionOfLiterals(type.unionTypes),
        (error) => new Error(`Internal Error: ${error}`),
      );

      if (Option.isSome(unionOfLiterals)) {
        if (value.kind === 'enum') {
          return unionOfLiterals.val.literals[value.value];
        } else {
          throw new Error(typeMismatchInDeserialize(value, 'enum'));
        }
      }

      const taggedUnions = Either.getOrThrowWith(
        getTaggedUnion(type.unionTypes),
        (error) => new Error(`Internal Error: ${error}`),
      );

      const filteredUnionTypes: Type.Type[] = type.unionTypes.filter(
        (t) => t.kind !== 'undefined' && t.kind !== 'null' && t.kind !== 'void',
      );

      // This implies this optional value
      if (filteredUnionTypes.length !== type.unionTypes.length) {
        if (filteredUnionTypes.length === 1) {
          return toTsValue(value, filteredUnionTypes[0]);
        }
      }

      if (value.kind === 'variant') {
        const caseValue = value.caseValue;

        if (Option.isSome(taggedUnions)) {
          const tags = TaggedUnion.getTaggedTypes(taggedUnions.val);
          const taggedTypeMetadata = tags[value.caseIdx];

          const tagName = taggedTypeMetadata.tagLiteralName;
          const innerType = taggedTypeMetadata.valueType;

          if (Option.isNone(innerType)) {
            return tagName;
          } else {
            const innerTypeVal = innerType.val;
            if (!caseValue) {
              throw new Error(typeMismatchInDeserialize(value, 'union'));
            }
            return toTsValue(caseValue, innerTypeVal[1]);
          }
        }

        const matchingType = filteredUnionTypes[value.caseIdx];

        if (!caseValue) {
          if (matchingType.kind === 'literal') {
            return matchingType.literalValue;
          } else {
            throw new Error(typeMismatchInDeserialize(value, 'union'));
          }
        }

        return toTsValue(caseValue, matchingType);
      } else if (value.kind === 'result') {
        const caseValue = value.value;

        if (Option.isSome(taggedUnions)) {
          const tags = TaggedUnion.getTaggedTypes(taggedUnions.val);

          const okOrErr = 'ok' in caseValue ? 'ok' : 'err';

          const taggedTypeMetadata = tags.find(
            (tag) => tag.tagLiteralName === okOrErr,
          );

          if (!taggedTypeMetadata) {
            throw new Error(typeMismatchInDeserialize(value, 'result'));
          }

          const tagName = taggedTypeMetadata.tagLiteralName;
          const innerType = taggedTypeMetadata.valueType;

          if (Option.isNone(innerType)) {
            return tagName;
          } else {
            const innerTypeVal = innerType.val;
            const okOrErrvalue = caseValue[okOrErr];

            if (!okOrErrvalue) {
              throw new Error(
                `Expected value for the tag 4 '${tagName}' of the union type '${name}'`,
              );
            }

            return toTsValue(okOrErrvalue, innerTypeVal[1]);
          }
        }

        throw new Error(typeMismatchInDeserialize(value, 'union'));
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'union'));
      }

    case 'object':
      if (value.kind === 'record') {
        const fieldValues = value.value;
        const expectedTypeFields = type.properties;
        return expectedTypeFields.reduce(
          (acc, field, idx) => {
            const name = field.getName();
            const expectedFieldType = field.getTypeAtLocation(
              field.getDeclarations()[0],
            );
            acc[name] = toTsValue(fieldValues[idx], expectedFieldType);
            return acc;
          },
          {} as Record<string, any>,
        );
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'object'));
      }

    case 'class':
      throw new Error(
        unhandledTypeError(
          value,
          Option.some(name ?? 'anonymous'),
          Option.none(),
        ),
      );

    case 'interface':
      if (value.kind === 'record') {
        const fieldValues = value.value;
        const expectedTypeFields = type.properties;
        return expectedTypeFields.reduce(
          (acc, field, idx) => {
            const name = field.getName();
            const expectedFieldType = field.getTypeAtLocation(
              field.getDeclarations()[0],
            );
            acc[name] = toTsValue(fieldValues[idx], expectedFieldType);
            return acc;
          },
          {} as Record<string, any>,
        );
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'interface'));
      }

    case 'promise':
      const innerType = type.element;
      if (!innerType) {
        throw new Error(
          `Internal Error: Expected Promise to have one type argument`,
        );
      }
      return toTsValue(value, innerType);

    case 'map':
      if (value.kind === 'list') {
        const entries: [any, any][] = value.value.map((item: Value) => {
          if (item.kind !== 'tuple' || item.value.length !== 2) {
            throw new Error(
              `Internal Error: Expected tuple of two items, got ${item}`,
            );
          }

          return [
            toTsValue(item.value[0], type.key),
            toTsValue(item.value[1], type.value),
          ] as [any, any];
        });
        return new Map(entries);
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'Map'));
      }

    case 'literal':
      const literalValue = type.literalValue;
      if (
        value.kind === 'bool' ||
        value.kind === 'string' ||
        value.kind === 'f64' ||
        value.kind === 's32'
      ) {
        return value.value;
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'boolean'));
      }

    case 'alias':
      throw new Error(
        unhandledTypeError(
          value,
          Option.some(name ?? 'anonymous'),
          Option.none(),
        ),
      );

    case 'others':
      throw new Error(
        unhandledTypeError(
          value,
          Option.some(name ?? 'anonymous'),
          Option.none(),
        ),
      );

    case 'unresolved-type':
      throw new Error(
        `Failed to resolve type for \`${type.text}\`: ${type.error}`,
      );
  }
}

function convertToNumber(value: Value): any {
  if (
    value.kind === 'f64' ||
    value.kind === 'u8' ||
    value.kind === 'u16' ||
    value.kind === 'u32' ||
    value.kind === 'u64' ||
    value.kind === 's8' ||
    value.kind === 's16' ||
    value.kind === 's32' ||
    value.kind === 's64' ||
    value.kind === 'f32'
  ) {
    return value.value;
  } else {
    throw new Error();
  }
}

function convertToBigInt(value: Value): any {
  if (value.kind === 'u64' || value.kind === 's64') {
    return value.value;
  } else {
    throw new Error(typeMismatchInDeserialize(value, 'bigint'));
  }
}
