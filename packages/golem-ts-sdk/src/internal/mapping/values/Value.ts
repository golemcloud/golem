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

import { Type, Symbol, Node } from '@golemcloud/golem-ts-types-core';
import * as Option from '../../../newTypes/option';
import {
  missingValueForKey,
  typeMismatchIn,
  typeMismatchOut,
  unhandledTypeError,
  unionTypeMatchError,
} from './errors';
import * as Either from '../../../newTypes/either';

export type Value =
  | { kind: 'bool'; value: boolean }
  | { kind: 'u8'; value: number }
  | { kind: 'u16'; value: number }
  | { kind: 'u32'; value: number }
  | { kind: 'u64'; value: bigint }
  | { kind: 's8'; value: number }
  | { kind: 's16'; value: number }
  | { kind: 's32'; value: number }
  | { kind: 's64'; value: bigint }
  | { kind: 'f32'; value: number }
  | { kind: 'f64'; value: number }
  | { kind: 'char'; value: string }
  | { kind: 'string'; value: string }
  | { kind: 'list'; value: Value[] }
  | { kind: 'tuple'; value: Value[] }
  | { kind: 'record'; value: Value[] }
  | { kind: 'variant'; caseIdx: number; caseValue?: Value }
  | { kind: 'enum'; value: number }
  | { kind: 'flags'; value: boolean[] }
  | { kind: 'option'; value?: Value }
  | { kind: 'result'; value: { ok?: Value; err?: Value } }
  | { kind: 'handle'; uri: string; resourceId: bigint };

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
      return { kind: 'enum', value: node.val };

    case 'flags-value':
      return { kind: 'flags', value: node.val };

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
        return { kind: 'option', value: undefined };
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
      return { kind: 'u8', value: node.val };
    case 'prim-u16':
      return { kind: 'u16', value: node.val };
    case 'prim-u32':
      return { kind: 'u32', value: node.val };
    case 'prim-u64':
      return { kind: 'u64', value: node.val };
    case 'prim-s8':
      return { kind: 's8', value: node.val };
    case 'prim-s16':
      return { kind: 's16', value: node.val };
    case 'prim-s32':
      return { kind: 's32', value: node.val };
    case 'prim-s64':
      return { kind: 's64', value: node.val };
    case 'prim-float32':
      return { kind: 'f32', value: node.val };
    case 'prim-float64':
      return { kind: 'f64', value: node.val };
    case 'prim-char':
      return { kind: 'char', value: node.val };
    case 'prim-bool':
      return { kind: 'bool', value: node.val };
    case 'prim-string':
      return { kind: 'string', value: node.val };

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
  nodes.push({ tag: 'placeholder', val: undefined } as any);

  switch (value.kind) {
    case 'record': {
      const recordIndices = value.value.map((v) => buildNodes(v, nodes));
      nodes[idx] = { tag: 'record-value', val: recordIndices };
      return idx;
    }

    case 'variant': {
      if (value.caseValue !== undefined) {
        const innerIdx = buildNodes(value.caseValue, nodes);
        nodes[idx] = { tag: 'variant-value', val: [value.caseIdx, innerIdx] };
      } else {
        nodes[idx] = { tag: 'variant-value', val: [value.caseIdx, undefined] };
      }
      return idx;
    }

    case 'enum':
      nodes[idx] = { tag: 'enum-value', val: value.value };
      return idx;

    case 'flags':
      nodes[idx] = { tag: 'flags-value', val: value.value };
      return idx;

    case 'tuple': {
      const tupleIndices = value.value.map((v) => buildNodes(v, nodes));
      nodes[idx] = { tag: 'tuple-value', val: tupleIndices };
      return idx;
    }

    case 'list': {
      const listIndices = value.value.map((v) => buildNodes(v, nodes));
      nodes[idx] = { tag: 'list-value', val: listIndices };
      return idx;
    }

    case 'option': {
      if (value.value !== undefined) {
        const innerIdx = buildNodes(value.value, nodes);
        nodes[idx] = { tag: 'option-value', val: innerIdx };
      } else {
        nodes[idx] = { tag: 'option-value', val: undefined };
      }
      return idx;
    }

    case 'result': {
      if ('ok' in value.value) {
        const innerIdx =
          value.value.ok !== undefined
            ? buildNodes(value.value.ok, nodes)
            : undefined;
        nodes[idx] = { tag: 'result-value', val: { tag: 'ok', val: innerIdx } };
      } else {
        const innerIdx =
          value.value.err !== undefined
            ? buildNodes(value.value.err, nodes)
            : undefined;
        nodes[idx] = {
          tag: 'result-value',
          val: { tag: 'err', val: innerIdx },
        };
      }
      return idx;
    }

    case 'u8':
      nodes[idx] = { tag: 'prim-u8', val: value.value };
      return idx;
    case 'u16':
      nodes[idx] = { tag: 'prim-u16', val: value.value };
      return idx;
    case 'u32':
      nodes[idx] = { tag: 'prim-u32', val: value.value };
      return idx;
    case 'u64':
      nodes[idx] = { tag: 'prim-u64', val: value.value };
      return idx;
    case 's8':
      nodes[idx] = { tag: 'prim-s8', val: value.value };
      return idx;
    case 's16':
      nodes[idx] = { tag: 'prim-s16', val: value.value };
      return idx;
    case 's32':
      nodes[idx] = { tag: 'prim-s32', val: value.value };
      return idx;
    case 's64':
      nodes[idx] = { tag: 'prim-s64', val: value.value };
      return idx;
    case 'f32':
      nodes[idx] = { tag: 'prim-float32', val: value.value };
      return idx;
    case 'f64':
      nodes[idx] = { tag: 'prim-float64', val: value.value };
      return idx;
    case 'char':
      nodes[idx] = { tag: 'prim-char', val: value.value };
      return idx;
    case 'bool':
      nodes[idx] = { tag: 'prim-bool', val: value.value };
      return idx;
    case 'string':
      nodes[idx] = { tag: 'prim-string', val: value.value };
      return idx;

    case 'handle':
      nodes[idx] = {
        tag: 'handle',
        val: [{ value: value.uri }, value.resourceId],
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
  type: Type.Type,
): Either.Either<Value, string> {
  if (type.name === 'String') {
    throw new Error(
      unhandledTypeError(
        tsValue,
        Option.some(type.name),
        Option.some("Use 'string' instead of 'String' in type definitions"),
      ),
    );
  }

  switch (type.kind) {
    case 'boolean':
      return handleBooleanType(tsValue);

    case 'number':
      if (typeof tsValue === 'number') {
        return Either.right({ kind: 's32', value: tsValue });
      } else {
        return Either.left(typeMismatchIn(tsValue, type));
      }

    case 'string':
      if (typeof tsValue === 'string') {
        return Either.right({ kind: 'string', value: tsValue });
      } else {
        return Either.left(typeMismatchIn(tsValue, type));
      }

    case 'bigint':
      if (typeof tsValue === 'bigint' || typeof tsValue === 'number') {
        return Either.right({ kind: 'u64', value: tsValue as any });
      } else {
        return Either.left(typeMismatchIn(tsValue, type));
      }

    case 'null':
      return Either.right({ kind: 'tuple', value: [] });

    case 'undefined':
      return Either.right({ kind: 'tuple', value: [] });

    case 'void':
      return Either.right({ kind: 'tuple', value: [] });

    case 'array':
      switch (type.name) {
        case 'Int8Array':
          const int8Array = handleTypedArray(tsValue, Int8Array);

          return Either.map(int8Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 's8',
              value: item,
            })),
          }));

        case 'Int16Array':
          const int16Array = handleTypedArray(tsValue, Int16Array);

          return Either.map(int16Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 's16',
              value: item,
            })),
          }));

        case 'Int32Array':
          const int32Array = handleTypedArray(tsValue, Int32Array);

          return Either.map(int32Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 's32',
              value: item,
            })),
          }));

        case 'BigInt64Array':
          const int64Array = handleTypedArray(tsValue, BigInt64Array);

          return Either.map(int64Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 's64',
              value: item,
            })),
          }));

        case 'Uint8Array':
          const uint8Array = handleTypedArray(tsValue, Uint8Array);

          return Either.map(uint8Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 'u8',
              value: item,
            })),
          }));

        case 'Uint16Array':
          const uint16Array = handleTypedArray(tsValue, Uint16Array);

          return Either.map(uint16Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 'u16',
              value: item,
            })),
          }));

        case 'Uint32Array':
          const uint32Array = handleTypedArray(tsValue, Uint32Array);

          return Either.map(uint32Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 'u32',
              value: item,
            })),
          }));

        case 'BigUint64Array':
          const uint64Array = handleTypedArray(tsValue, BigUint64Array);

          return Either.map(uint64Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 'u64',
              value: item,
            })),
          }));

        case 'Float32Array':
          const float32Array = handleTypedArray(tsValue, Float32Array);

          return Either.map(float32Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 'f32',
              value: item,
            })),
          }));

        case 'Float64Array':
          const float64Array = handleTypedArray(tsValue, Float64Array);

          return Either.map(float64Array, (arr) => ({
            kind: 'list' as const,
            value: Array.from(arr).map((item) => ({
              kind: 'f32',
              value: item,
            })),
          }));
      }

      return handleArrayType(tsValue, type.element);

    case 'tuple':
      return handleTupleType(tsValue, type.elements);

    case 'union':
      return handleUnion(tsValue, type, type.unionTypes);

    case 'object':
      return handleObject(tsValue, type, type.properties);

    case 'class':
      return Either.left(
        unhandledTypeError(
          tsValue,
          Option.none(),
          Option.some('Classes are not supported'),
        ),
      );

    case 'interface':
      return handleObject(tsValue, type, type.properties);

    case 'promise':
      const inner = type.element;

      if (!inner) {
        return Either.left(
          unhandledTypeError(
            tsValue,
            Option.none(),
            Option.some('Unable to infer the type of promise'),
          ),
        );
      }
      return fromTsValue(tsValue, inner);

    case 'map':
      return handleKeyValuePairs(tsValue, type, type.key, type.value);

    case 'literal':
      if (type.name === 'true' || type.name === 'false') {
        return handleBooleanType(tsValue);
      } else {
        if (tsValue === type.name) {
          if (typeof tsValue === 'string') {
            return Either.right({ kind: 'string', value: tsValue });
          } else if (typeof tsValue === 'number') {
            return Either.right({ kind: 's32', value: tsValue });
          } else if (typeof tsValue === 'bigint') {
            return Either.right({ kind: 'u64', value: tsValue });
          } else {
            return Either.left(typeMismatchIn(tsValue, type));
          }
        } else {
          return Either.left(typeMismatchIn(tsValue, type));
        }
      }

    case 'alias':
      return Either.left(
        unhandledTypeError(tsValue, Option.none(), Option.none()),
      );

    case 'others':
      return Either.left(
        unhandledTypeError(
          tsValue,
          Option.some(type.name ?? 'anonymous'),
          Option.none(),
        ),
      );
  }
}

function handleTypedArray<
  A extends
    | Uint8Array
    | Uint16Array
    | Uint32Array
    | BigUint64Array
    | Int8Array
    | Int16Array
    | Int32Array
    | BigInt64Array
    | Float32Array
    | Float64Array,
>(tsValue: unknown, ctor: { new (_: number): A }): Either.Either<A, string> {
  return tsValue instanceof ctor
    ? Either.right(tsValue)
    : Either.left(
        typeMismatchIn(tsValue, { kind: 'array', element: { kind: 'number' } }),
      );
}

function handleBooleanType(tsValue: any): Either.Either<Value, string> {
  if (typeof tsValue === 'boolean') {
    return Either.right({ kind: 'bool', value: tsValue });
  } else {
    return Either.left(typeMismatchIn(tsValue, { kind: 'boolean' }));
  }
}

function handleArrayType(
  tsValue: any,
  elementType: Type.Type,
): Either.Either<Value, string> {
  if (!Array.isArray(tsValue)) {
    return Either.left(
      typeMismatchIn(tsValue, { kind: 'array', element: elementType }),
    );
  }

  return Either.map(
    Either.all(tsValue.map((item) => fromTsValue(item, elementType))),
    (values) => ({ kind: 'list', value: values }),
  );
}

function handleTupleType(
  tsValue: any,
  types: Type.Type[],
): Either.Either<Value, string> {
  if (!Array.isArray(tsValue)) {
    return Either.left(
      typeMismatchIn(tsValue, { kind: 'tuple', elements: types }),
    );
  }

  return Either.map(
    Either.all(tsValue.map((item, idx) => fromTsValue(item, types[idx]))),
    (values) => ({ kind: 'tuple', value: values }),
  );
}

function handleKeyValuePairs(
  tsValue: any,
  mapType: Type.Type,
  keyType: Type.Type,
  valueType: Type.Type,
): Either.Either<Value, string> {
  if (!(tsValue instanceof Map)) {
    return Either.left(typeMismatchIn(tsValue, mapType));
  }

  const values = Either.all(
    Array.from(tsValue.entries()).map(([key, value]) =>
      Either.zipWith(
        fromTsValue(key, keyType),
        fromTsValue(value, valueType),
        (k, v) => ({ kind: 'tuple', value: [k, v] }) as Value,
      ),
    ),
  );

  return Either.map(values, (value) => ({ kind: 'list', value }));
}

function handleObject(
  tsValue: any,
  type: Type.Type,
  innerProperties: Symbol[],
): Either.Either<Value, string> {
  if (typeof tsValue !== 'object' || tsValue === null) {
    return Either.left(typeMismatchIn(tsValue, type));
  }
  const values: Value[] = [];

  for (const prop of innerProperties) {
    const key = prop.getName();

    const nodes: Node[] = prop.getDeclarations();
    const node = nodes[0];
    const propType = prop.getTypeAtLocation(node);

    if (!Object.prototype.hasOwnProperty.call(tsValue, key)) {
      if (Node.isPropertySignature(node) || Node.isPropertyDeclaration(node)) {
        if (node.hasQuestionToken()) {
          values.push({ kind: 'option' });
        } else if (propType.kind === 'string' && tsValue === '') {
          values.push({ kind: 'string', value: '' });
        } else if (propType.kind === 'number' && tsValue === 0) {
          values.push({ kind: 's32', value: 0 });
        } else if (propType.kind === 'boolean' && tsValue === false) {
          values.push({ kind: 'bool', value: false });
        } else {
          return Either.left(missingValueForKey(key, tsValue));
        }
        continue;
      }
    }

    const fieldVal = fromTsValue(tsValue[key], propType);

    if (Either.isLeft(fieldVal)) {
      return Either.left(fieldVal.val);
    }

    values.push(fieldVal.val);
  }

  return Either.right({ kind: 'record', value: values });
}

function handleUnion(
  tsValue: any,
  type: Type.Type,
  possibleTypes: Type.Type[],
): Either.Either<Value, string> {
  const typeWithIndex = findTypeOfAny(tsValue, possibleTypes);

  if (!typeWithIndex) {
    return Either.left(unionTypeMatchError(tsValue, possibleTypes));
  } else {
    const innerType = typeWithIndex[0];

    return Either.map(fromTsValue(tsValue, innerType), (result) => {
      return {
        kind: 'variant',
        caseIdx: typeWithIndex[1],
        caseValue: result,
      };
    });
  }
}

function findTypeOfAny(
  value: any,
  typeList: readonly Type.Type[],
): [Type.Type, number] | undefined {
  for (let idx = 0; idx < typeList.length; idx++) {
    const type = typeList[idx];
    if (matchesType(value, type)) {
      return [type, idx];
    }
  }
  return undefined;
}

function matchesType(value: any, type: Type.Type): boolean {
  switch (type.kind) {
    case 'boolean':
      return typeof value === 'boolean';

    case 'number':
      return typeof value === 'number';

    case 'string':
      return typeof value === 'string';

    case 'bigint':
      return typeof value === 'bigint' || typeof value === 'number';

    case 'null':
      return value === null;

    case 'undefined':
      return value === undefined;

    case 'void':
      return value === undefined || value === null;

    case 'array':
      const elemType = type.element;

      return matchesArray(value, elemType);

    case 'tuple':
      return matchesTuple(value, type.elements);

    case 'union':
      return type.unionTypes.some((t) => matchesType(value, t));

    case 'object':
      return handleObjectMatch(value, type, type.properties);

    case 'class':
      return false;

    case 'interface':
      return handleObjectMatch(value, type, type.properties);

    case 'promise':
      return matchesType(value, type.element);

    case 'map':
      const keyType = type.key;
      const valType = type.value;

      if (!keyType || !valType) {
        return false;
      }
      if (!(value instanceof Map)) return false;

      return Array.from(value.entries()).every(
        ([k, v]) => matchesType(k, keyType) && matchesType(v, valType),
      );

    case 'literal':
      const name = type.name;
      if (name === 'true' || name === 'false') {
        return typeof value === 'boolean';
      } else {
        return value === type.name;
      }

    case 'alias':
      return false;

    case 'others':
      return false;
  }
}

function matchesTuple(
  value: any,
  tupleTypes: readonly Type.Type[] | undefined,
): boolean {
  if (!Array.isArray(value)) return false;
  if (!tupleTypes) return false;
  if (value.length !== tupleTypes.length) return false;

  return value.every((v, idx) => matchesType(v, tupleTypes[idx]));
}

function matchesArray(value: any, elementType: Type.Type): boolean {
  if (!Array.isArray(value)) return false;
  return value.every((item) => matchesType(item, elementType));
}

function handleObjectMatch(
  value: any,
  type: Type.Type,
  props: Symbol[],
): boolean {
  if (typeof value !== 'object' || value === null) return false;

  const valueKeys = Object.keys(value);
  if (valueKeys.length !== props.length) return false;

  for (const prop of props) {
    const propName = prop.getName();
    const hasKey = Object.prototype.hasOwnProperty.call(value, propName);

    const decl = prop.getDeclarations()[0];
    let isOptional = false;

    if (Node.isPropertySignature(decl)) {
      isOptional = decl.hasQuestionToken();
    } else if (Node.isPropertyDeclaration(decl)) {
      isOptional = decl.hasQuestionToken();
    }

    if (!hasKey) {
      if (!isOptional) return false;
    } else {
      const propType = prop.getTypeAtLocation(decl);
      if (!matchesType(value[propName], propType)) return false;
    }
  }

  return true;
}

export function toTsValue(value: Value, type: Type.Type): any {
  const name = type.name;

  if (value.kind === 'option') {
    const caseValue = value.value;
    if (!caseValue) {
      return undefined;
    }

    return toTsValue(caseValue, type);
  }

  switch (type.kind) {
    case 'boolean':
      if (value.kind === 'bool') {
        return value.value;
      } else {
        throw new Error(typeMismatchOut(value, 'boolean'));
      }

    case 'number':
      return convertToNumber(value);

    case 'string':
      if (value.kind === 'string') {
        return value.value;
      } else {
        throw new Error(typeMismatchOut(value, 'string'));
      }

    case 'bigint':
      return convertToBigInt(value);

    case 'null':
      if (value.kind === 'tuple' && value.value.length === 0) {
        return null;
      } else {
        throw new Error(typeMismatchOut(value, 'null'));
      }

    case 'undefined':
      if (value.kind === 'tuple' && value.value.length === 0) {
        return undefined;
      } else {
        throw new Error(typeMismatchOut(value, 'undefined'));
      }

    case 'array':
      switch (type.name) {
        case 'Uint8Array':
          if (value.kind === 'list') {
            return new Uint8Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchOut(value, 'Uint8Array'));
          }
        case 'Uint8ClampedArray':
          if (value.kind === 'list') {
            return new Uint8ClampedArray(
              value.value.map((v) => convertToNumber(v)),
            );
          } else {
            throw new Error(typeMismatchOut(value, 'Uint8ClampedArray'));
          }
        case 'Int8Array':
          if (value.kind === 'list') {
            return new Int8Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchOut(value, 'Int8Array'));
          }

        case 'Int16Array':
          if (value.kind === 'list') {
            return new Int16Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchOut(value, 'Int16Array'));
          }
        case 'Uint16Array':
          if (value.kind === 'list') {
            return new Uint16Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchOut(value, 'Uint16Array'));
          }
        case 'Int32Array':
          if (value.kind === 'list') {
            return new Int32Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchOut(value, 'Int32Array'));
          }
        case 'Uint32Array':
          if (value.kind === 'list') {
            return new Uint32Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchOut(value, 'Uint32Array'));
          }
        case 'Float32Array':
          if (value.kind === 'list') {
            return new Float32Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchOut(value, 'Float32Array'));
          }
        case 'Float64Array':
          if (value.kind === 'list') {
            return new Float64Array(value.value.map((v) => convertToNumber(v)));
          } else {
            throw new Error(typeMismatchOut(value, 'Float64Array'));
          }
        case 'BigInt64Array':
          if (value.kind === 'list') {
            return new BigInt64Array(
              value.value.map((v) => convertToBigInt(v)),
            );
          } else {
            throw new Error(typeMismatchOut(value, 'BigInt64Array'));
          }
        case 'BigUint64Array':
          if (value.kind === 'list') {
            return new BigUint64Array(
              value.value.map((v) => convertToBigInt(v)),
            );
          } else {
            throw new Error(typeMismatchOut(value, 'BigUint64Array'));
          }
      }

      if (value.kind === 'list') {
        const elemType = type.element;

        if (!elemType) {
          throw new Error(`Unable to infer the type of Array`);
        }
        return value.value.map((item: Value) => toTsValue(item, elemType));
      } else {
        throw new Error(typeMismatchOut(value, 'array'));
      }

    case 'tuple':
      const typeArg = type.elements;
      if (value.kind === 'tuple') {
        return value.value.map((item: Value, idx: number) =>
          toTsValue(item, typeArg[idx]),
        );
      } else {
        throw new Error(typeMismatchOut(value, 'tuple'));
      }

    case 'union':
      if (value.kind === 'variant') {
        const caseValue = value.caseValue;
        if (!caseValue) {
          throw new Error(`Expected union, got ${value}`);
        }

        const unionTypes = type.unionTypes;
        const matchingType = unionTypes[value.caseIdx];

        return toTsValue(caseValue, matchingType);
      } else {
        throw new Error(typeMismatchOut(value, 'union'));
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
        throw new Error(typeMismatchOut(value, 'object'));
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
        throw new Error(typeMismatchOut(value, 'interface'));
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
        throw new Error(typeMismatchOut(value, 'Map'));
      }

    case 'literal':
      const literalValue = type.name;
      if (
        value.kind === 'bool' &&
        (literalValue === 'true' || literalValue === 'false')
      ) {
        return value.value;
      } else {
        throw new Error(typeMismatchOut(value, 'boolean'));
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
    throw new Error(`Unable to convert the ${JSON.stringify(value)} to number`);
  }
}

function convertToBigInt(value: Value): any {
  if (value.kind === 'u64' || value.kind === 's64') {
    return value.value;
  } else {
    throw new Error(typeMismatchOut(value, 'bigint'));
  }
}
