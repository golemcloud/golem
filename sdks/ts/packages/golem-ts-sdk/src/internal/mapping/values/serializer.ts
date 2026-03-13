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

import { AnalysedType, NameOptionTypePair, NameTypePair, TypedArray } from '../types/analysedType';
import {
  customSerializationError,
  enumMismatchInSerialize,
  missingObjectKey,
  typeMismatchInSerialize,
  unhandledTypeError,
  unionTypeMatchError,
} from './errors';
import { TaggedTypeMetadata } from '../types/taggedUnion';
import { BinaryReference, TextReference } from 'golem:agent/common@1.5.0';
import * as util from 'node:util';
import { WitNodeBuilder } from './WitNodeBuilder';

export function serializeTsValueToBinaryReference(tsValue: any): BinaryReference {
  if (typeof tsValue === 'object' && tsValue !== null) {
    const keys = Object.keys(tsValue);

    if (!keys.includes('tag')) {
      throw new Error(
        `Unable to cast value ${util.format(
          tsValue,
        )} to UnstructuredBinary. Missing 'tag' property.`,
      );
    }

    const tag = tsValue['tag'];

    if (typeof tag === 'string' && tag === 'url') {
      if (keys.includes('val')) {
        return {
          tag: 'url',
          val: tsValue['val'],
        };
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            tsValue,
          )} to UnstructuredBinary. Missing 'val' property for tag 'url'.`,
        );
      }
    }

    if (typeof tag === 'string' && tag === 'inline') {
      if (keys.includes('val') && keys.includes('mimeType')) {
        return {
          tag: 'inline',
          val: {
            data: tsValue['val'],
            binaryType: {
              mimeType: tsValue['mimeType'],
            },
          },
        };
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            tsValue,
          )} to UnstructuredBinary. Missing 'val' property for tag 'inline'.`,
        );
      }
    }

    throw new Error(
      `Unable to cast value ${util.format(
        tsValue,
      )} to UnstructuredBinary. Invalid 'tag' property: ${tag}. Expected 'url' or 'inline'.`,
    );
  }

  throw new Error(
    `Unable to cast value ${util.format(
      tsValue,
    )} to UnstructuredBinary. Expected an object with 'tag' and 'val' properties.`,
  );
}

export function serializeTsValueToTextReference(value: any): TextReference {
  if (typeof value === 'object' && value !== null) {
    const keys = Object.keys(value);

    if (!keys.includes('tag')) {
      throw new Error(
        `Unable to cast value ${util.format(value)} to UnstructuredText. Missing 'tag' property.`,
      );
    }

    const tag = value['tag'];

    if (typeof tag === 'string' && tag === 'url') {
      if (keys.includes('val')) {
        return {
          tag: 'url',
          val: value['val'],
        };
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            value,
          )} to UnstructuredText. Missing 'val' property for tag 'url'.`,
        );
      }
    }

    if (typeof tag === 'string' && tag === 'inline') {
      if (keys.includes('val')) {
        if (keys.includes('languageCode')) {
          return {
            tag: 'inline',
            val: {
              data: value['val'],
              textType: {
                languageCode: value['languageCode'],
              },
            },
          };
        } else {
          return {
            tag: 'inline',
            val: {
              data: value['val'],
            },
          };
        }
      } else {
        throw new Error(
          `Unable to cast value ${util.format(
            value,
          )} to UnstructuredText. Missing 'val' property for tag 'inline'.`,
        );
      }
    }

    throw new Error(
      `Unable to cast value ${util.format(
        value,
      )} to UnstructuredText. Invalid 'tag' property: ${tag}. Expected 'url' or 'inline'.`,
    );
  }

  throw new Error(
    `Unable to cast value ${util.format(
      value,
    )} to UnstructuredText. Expected an object with 'tag' and 'val' properties.`,
  );
}

export function matchesType(value: any, type: AnalysedType): boolean {
  const valueType = typeof value;

  switch (type.kind) {
    case 'bool':
      return valueType === 'boolean';

    case 'f64':
    case 'f32':
    case 's32':
    case 's16':
    case 's8':
    case 'u32':
    case 'u16':
    case 'u8':
      return valueType === 'number';

    case 's64':
      return valueType === 'bigint';

    case 'u64':
      return valueType === 'number' || valueType === 'bigint';

    case 'string':
      return valueType === 'string';

    case 'option':
      return value === undefined || value === null || matchesType(value, type.value.inner);

    case 'list': {
      const isTypedArray = type.typedArray;
      const elemType = type.value.inner;
      const result = matchesArray(value, elemType, isTypedArray);

      if (result) {
        return true;
      }

      // It indicates a map then
      if (elemType.kind === 'tuple' && elemType.value.items.length === 2) {
        if (value instanceof Map) {
          return Array.from(value.entries()).every(
            ([k, v]) =>
              matchesType(k, elemType.value.items[0]) && matchesType(v, elemType.value.items[1]),
          );
        }
      }

      return false;
    }

    case 'tuple':
      return matchesTuple(value, type.value.items);

    case 'result':
      if (valueType !== 'object' || value === null) return false;

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
      return valueType === 'string' && type.value.cases.includes(value.toString());

    // A variant can be tagged union or simple union
    case 'variant': {
      if (value == null) return false;

      const nameAndOptions = type.value.cases;

      // There are two cases, if they are tagged types, or not
      if (valueType === 'object') {
        const keys = Object.keys(value);

        if (keys.includes('tag')) {
          const tagValue = value['tag'];

          if (typeof tagValue === 'string') {
            const matchedCase = nameAndOptions.find(
              (nameType) => nameType.name === tagValue.toString(),
            );

            if (!matchedCase) {
              return false;
            }

            const caseType = matchedCase.typ;

            if (!caseType) {
              return keys.length === 1;
            }

            const valueKey = keys.find((k) => k !== 'tag');

            if (!valueKey) {
              return false;
            }

            return matchesType(value[valueKey], caseType);
          }
        }
      }

      for (const unionType of nameAndOptions) {
        const caseTy = unionType.typ;
        const name = unionType.name;

        if (!caseTy) {
          if (valueType === 'string' && value === name) {
            return true;
          }
          continue;
        }

        if (matchesType(value, caseTy)) {
          return true;
        }
      }

      return false;
    }

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

function matchesTuple(value: any, tupleTypes: readonly AnalysedType[] | undefined): boolean {
  if (!Array.isArray(value)) return false;
  if (!tupleTypes) return false;
  if (value.length !== tupleTypes.length) return false;

  return value.every((v, idx) => matchesType(v, tupleTypes[idx]));
}

function matchesArray(
  value: any,
  elementType: AnalysedType,
  typedArray: TypedArray | undefined,
): boolean {
  if (typedArray) {
    switch (typedArray) {
      case 'u8':
        return value instanceof Uint8Array;
      case 'u16':
        return value instanceof Uint16Array;
      case 'u32':
        return value instanceof Uint32Array;
      case 'big-u64':
        return value instanceof BigUint64Array;
      case 'i8':
        return value instanceof Int8Array;
      case 'i16':
        return value instanceof Int16Array;
      case 'i32':
        return value instanceof Int32Array;
      case 'big-i64':
        return value instanceof BigInt64Array;
      case 'f32':
        return value instanceof Float32Array;
      case 'f64':
        return value instanceof Float64Array;
    }
  }

  if (!Array.isArray(value)) return false;

  return value.every((item) => matchesType(item, elementType));
}

function handleObjectMatch(value: any, props: NameTypePair[]): boolean {
  if (typeof value !== 'object' || value === null) {
    return false;
  }

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

// ─── New direct WitNode serialization ───────────────────────────────────────

export function serializeToWitNodes(
  tsValue: any,
  analysedType: AnalysedType,
  builder: WitNodeBuilder,
): number {
  switch (analysedType.kind) {
    case 'flags':
      throw new Error(unhandledTypeError(tsValue, 'flags', undefined));
    case 'chr':
      throw new Error(unhandledTypeError(tsValue, 'char', undefined));
    case 'handle':
      throw new Error(unhandledTypeError(tsValue, 'handle', undefined));

    case 'bool':
      if (typeof tsValue !== 'boolean')
        throw new Error(typeMismatchInSerialize(tsValue, 'boolean'));
      return builder.bool(tsValue);

    case 'f32':
      if (typeof tsValue !== 'number') throw new Error(typeMismatchInSerialize(tsValue, 'number'));
      return builder.f32(tsValue);
    case 'f64':
      if (typeof tsValue !== 'number') throw new Error(typeMismatchInSerialize(tsValue, 'number'));
      return builder.f64(tsValue);

    case 'u8':
      if (typeof tsValue !== 'number') throw new Error(typeMismatchInSerialize(tsValue, 'number'));
      return builder.u8(tsValue);
    case 'u16':
      if (typeof tsValue !== 'number') throw new Error(typeMismatchInSerialize(tsValue, 'number'));
      return builder.u16(tsValue);
    case 'u32':
      if (typeof tsValue !== 'number') throw new Error(typeMismatchInSerialize(tsValue, 'number'));
      return builder.u32(tsValue);
    case 's8':
      if (typeof tsValue !== 'number') throw new Error(typeMismatchInSerialize(tsValue, 'number'));
      return builder.s8(tsValue);
    case 's16':
      if (typeof tsValue !== 'number') throw new Error(typeMismatchInSerialize(tsValue, 'number'));
      return builder.s16(tsValue);
    case 's32':
      if (typeof tsValue !== 'number') throw new Error(typeMismatchInSerialize(tsValue, 'number'));
      return builder.s32(tsValue);

    case 'u64':
      if (typeof tsValue === 'bigint') {
        return builder.u64(tsValue);
      } else if (typeof tsValue === 'number') {
        return builder.u64(BigInt(tsValue));
      } else {
        throw new Error(typeMismatchInSerialize(tsValue, 'bigint'));
      }

    case 's64':
      if (typeof tsValue !== 'bigint') throw new Error(typeMismatchInSerialize(tsValue, 'number'));
      return builder.s64(tsValue);

    case 'string':
      if (typeof tsValue !== 'string') throw new Error(typeMismatchInSerialize(tsValue, 'string'));
      return builder.string(tsValue);

    case 'option': {
      const innerType = analysedType.value.inner;
      if (tsValue === null || tsValue === undefined) {
        return builder.optionNone();
      }
      const optIdx = builder.addOptionSome();
      builder.finishChild(optIdx, serializeToWitNodes(tsValue, innerType, builder));
      return optIdx;
    }

    case 'list':
      return serializeListToWitNodes(tsValue, analysedType, builder);

    case 'tuple': {
      const tupleElems = analysedType.value.items;
      if (tupleElems.length === 0) {
        if (tsValue === null || tsValue === undefined) {
          return builder.addTuple();
        } else {
          throw new Error(typeMismatchInSerialize(tsValue, `Array of length ${tupleElems.length}`));
        }
      }
      return serializeTupleToWitNodes(tsValue, tupleElems, builder);
    }

    case 'variant': {
      const variantTypes = analysedType.value.cases;
      const taggedTypes = analysedType.taggedTypes;
      return serializeUnionToWitNodes(tsValue, taggedTypes, variantTypes, builder);
    }

    case 'enum': {
      if (typeof tsValue === 'string') {
        const caseIdx = analysedType.value.cases.indexOf(tsValue);
        if (caseIdx !== -1) {
          return builder.enumValue(caseIdx);
        }
      }
      throw new Error(enumMismatchInSerialize(analysedType.value.cases, tsValue));
    }

    case 'record':
      return serializeObjectToWitNodes(tsValue, analysedType.value.fields, builder);

    case 'result': {
      const okType = analysedType.value.ok;
      const errType = analysedType.value.err;

      if (typeof tsValue !== 'object' || tsValue === null) {
        throw new Error(typeMismatchInSerialize(tsValue, 'object'));
      }
      if (!('tag' in tsValue)) {
        throw new Error(missingObjectKey('tag', tsValue));
      }

      switch (analysedType.resultType.tag) {
        case 'inbuilt': {
          const keys = Object.keys(tsValue);
          if (!keys.includes('tag')) throw new Error(missingObjectKey('tag', tsValue));
          if (!keys.includes('val')) throw new Error(missingObjectKey('val', tsValue));

          if (tsValue['tag'] === 'ok') {
            if (!okType) {
              if (analysedType.resultType.okEmptyType) {
                return builder.resultOkUnit();
              }
              throw new Error(customSerializationError('unresolved ok type'));
            }
            const resIdx = builder.addResultOk();
            builder.finishChild(resIdx, serializeToWitNodes(tsValue['val'], okType, builder));
            return resIdx;
          }

          if (tsValue['tag'] === 'err') {
            if (!errType) {
              if (analysedType.resultType.errEmptyType) {
                return builder.resultErrUnit();
              }
              throw new Error(customSerializationError('unresolved err type'));
            }
            const resIdx = builder.addResultErr();
            builder.finishChild(resIdx, serializeToWitNodes(tsValue['val'], errType, builder));
            return resIdx;
          }

          throw new Error(typeMismatchInSerialize(tsValue, 'Result'));
        }

        case 'custom': {
          const okValueName = analysedType.resultType.okValueName;
          const errValueName = analysedType.resultType.errValueName;

          if (tsValue['tag'] === 'ok') {
            if (okType) {
              if (!okValueName) {
                throw new Error(customSerializationError('unresolved key name for ok value'));
              }
              const resIdx = builder.addResultOk();
              builder.finishChild(
                resIdx,
                serializeToWitNodes(tsValue[okValueName], okType, builder),
              );
              return resIdx;
            }
            return builder.resultOkUnit();
          } else if (typeof tsValue === 'object' && tsValue['tag'] === 'err') {
            if (errType) {
              if (!errValueName) {
                throw new Error(customSerializationError('unresolved key name for err value'));
              }
              const resIdx = builder.addResultErr();
              builder.finishChild(
                resIdx,
                serializeToWitNodes(tsValue[errValueName], errType, builder),
              );
              return resIdx;
            }
            return builder.resultErrUnit();
          } else {
            throw new Error(typeMismatchInSerialize(tsValue, 'object with tag property'));
          }
        }
      }
    }
  }
}

function serializeListToWitNodes(
  tsValue: any,
  analysedType: AnalysedType & { kind: 'list' },
  builder: WitNodeBuilder,
): number {
  const innerListType = analysedType.value.inner;
  const typedArray = analysedType.typedArray;

  if (typedArray) {
    switch (typedArray) {
      case 'u8': {
        if (!(tsValue instanceof Uint8Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'Uint8Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.u8(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
      case 'u16': {
        if (!(tsValue instanceof Uint16Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'Uint16Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.u16(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
      case 'u32': {
        if (!(tsValue instanceof Uint32Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'Uint32Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.u32(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
      case 'big-u64': {
        if (!(tsValue instanceof BigUint64Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'BigUint64Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.u64(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
      case 'i8': {
        if (!(tsValue instanceof Int8Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'Int8Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.s8(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
      case 'i16': {
        if (!(tsValue instanceof Int16Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'Int16Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.s16(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
      case 'i32': {
        if (!(tsValue instanceof Int32Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'Int32Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.s32(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
      case 'big-i64': {
        if (!(tsValue instanceof BigInt64Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'BigInt64Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.s64(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
      case 'f32': {
        if (!(tsValue instanceof Float32Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'Float32Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.f32(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
      case 'f64': {
        if (!(tsValue instanceof Float64Array))
          throw new Error(typeMismatchInSerialize(tsValue, 'Float64Array'));
        const listIdx = builder.addList();
        const indices: number[] = new Array(tsValue.length);
        for (let i = 0; i < tsValue.length; i++) indices[i] = builder.f64(tsValue[i]);
        builder.finishSeq(listIdx, indices);
        return listIdx;
      }
    }
  }

  if (Array.isArray(tsValue)) {
    const listIdx = builder.addList();
    const childIndices: number[] = new Array(tsValue.length);
    for (let i = 0; i < tsValue.length; i++) {
      childIndices[i] = serializeToWitNodes(tsValue[i], innerListType, builder);
    }
    builder.finishSeq(listIdx, childIndices);
    return listIdx;
  }

  if (tsValue instanceof Map) {
    if (
      !innerListType ||
      innerListType.kind !== 'tuple' ||
      innerListType.value.items.length !== 2
    ) {
      throw new Error(typeMismatchInSerialize(tsValue, 'Map'));
    }
    const keyType = innerListType.value.items[0];
    const valueType = innerListType.value.items[1];
    return serializeKeyValuePairsToWitNodes(tsValue, keyType, valueType, builder);
  }

  throw new Error(typeMismatchInSerialize(tsValue, 'Array'));
}

function serializeKeyValuePairsToWitNodes(
  tsValue: Map<any, any>,
  keyType: AnalysedType,
  valueType: AnalysedType,
  builder: WitNodeBuilder,
): number {
  const listIdx = builder.addList();
  const tupleIndices: number[] = new Array(tsValue.size);

  let i = 0;
  for (const [key, value] of tsValue) {
    const tupleIdx = builder.addTuple();
    const keyResult = serializeToWitNodes(key, keyType, builder);
    const valueResult = serializeToWitNodes(value, valueType, builder);
    builder.finishSeq(tupleIdx, [keyResult, valueResult]);
    tupleIndices[i++] = tupleIdx;
  }

  builder.finishSeq(listIdx, tupleIndices);
  return listIdx;
}

function serializeTupleToWitNodes(
  tsValue: any,
  tupleElemTypes: AnalysedType[],
  builder: WitNodeBuilder,
): number {
  if (!Array.isArray(tsValue) || tsValue.length !== tupleElemTypes.length) {
    throw new Error(typeMismatchInSerialize(tsValue, `Array of length ${tupleElemTypes.length}`));
  }

  const tupleIdx = builder.addTuple();
  const childIndices: number[] = new Array(tsValue.length);
  for (let i = 0; i < tsValue.length; i++) {
    childIndices[i] = serializeToWitNodes(tsValue[i], tupleElemTypes[i], builder);
  }
  builder.finishSeq(tupleIdx, childIndices);
  return tupleIdx;
}

function serializeUnionToWitNodes(
  tsValue: any,
  taggedTypes: TaggedTypeMetadata[],
  nameOptionTypePairs: NameOptionTypePair[],
  builder: WitNodeBuilder,
): number {
  if (taggedTypes.length > 0) {
    return serializeTaggedUnionToWitNodes(tsValue, nameOptionTypePairs, builder);
  }

  for (let idx = 0; idx < nameOptionTypePairs.length; idx++) {
    const variant = nameOptionTypePairs[idx];
    const analysedType = variant.typ;

    if (!analysedType) {
      if (tsValue === variant.name) {
        return builder.variantUnit(idx);
      }
      continue;
    }

    if (matchesType(tsValue, analysedType)) {
      const varIdx = builder.addVariant(idx);
      builder.finishChild(varIdx, serializeToWitNodes(tsValue, analysedType, builder));
      return varIdx;
    }
  }

  throw new Error(unionTypeMatchError(nameOptionTypePairs, tsValue));
}

function serializeTaggedUnionToWitNodes(
  tsValue: any,
  nameOptionTypePairs: NameOptionTypePair[],
  builder: WitNodeBuilder,
): number {
  if (typeof tsValue !== 'object' || tsValue === null) {
    throw new Error(typeMismatchInSerialize(tsValue, 'object with tag property'));
  }

  const keys = Object.keys(tsValue);

  if (!keys.includes('tag')) {
    throw new Error(missingObjectKey('tag', tsValue));
  }

  for (let caseIdx = 0; caseIdx < nameOptionTypePairs.length; caseIdx++) {
    const nameOptionTypePair = nameOptionTypePairs[caseIdx];
    const typeName = nameOptionTypePair.name;
    const typeOption = nameOptionTypePair.typ;

    if (tsValue['tag'] === typeName) {
      if (!typeOption) {
        return builder.variantUnit(caseIdx);
      }

      const valueKey = keys.find((k) => k !== 'tag');
      if (!valueKey) {
        throw new Error(`Missing value correspond to the tag ${typeName}`);
      }

      const varIdx = builder.addVariant(caseIdx);
      builder.finishChild(varIdx, serializeToWitNodes(tsValue[valueKey], typeOption, builder));
      return varIdx;
    }
  }

  throw new Error(unionTypeMatchError(nameOptionTypePairs, tsValue));
}

function serializeObjectToWitNodes(
  tsValue: any,
  nameTypePairs: NameTypePair[],
  builder: WitNodeBuilder,
): number {
  if (typeof tsValue !== 'object' || tsValue === null) {
    throw new Error(typeMismatchInSerialize(tsValue, 'object'));
  }

  const recIdx = builder.addRecord();
  const childIndices: number[] = [];

  for (const prop of nameTypePairs) {
    const key = prop.name;
    const type = prop.typ;

    if (!Object.prototype.hasOwnProperty.call(tsValue, key)) {
      if (tsValue === '' && type.kind === 'string') {
        childIndices.push(builder.string(''));
      }

      if (tsValue === '0' && type.kind === 'f64') {
        childIndices.push(builder.f64(0));
      }

      if (tsValue === '0' && type.kind === 'u64') {
        childIndices.push(builder.u64(0n));
      }

      if (tsValue === false && type.kind === 'bool') {
        childIndices.push(builder.bool(false));
      }

      if (type.kind === 'option') {
        childIndices.push(builder.optionNone());
        continue;
      }
    }

    childIndices.push(serializeToWitNodes(tsValue[key], type, builder));
  }

  builder.finishSeq(recIdx, childIndices);
  return recIdx;
}

export function serializeBinaryReferenceToWitNodes(tsValue: any, builder: WitNodeBuilder): number {
  const binaryReference = serializeTsValueToBinaryReference(tsValue);

  switch (binaryReference.tag) {
    case 'url': {
      const varIdx = builder.addVariant(0);
      const urlIdx = builder.string(binaryReference.val);
      builder.finishChild(varIdx, urlIdx);
      return varIdx;
    }
    case 'inline': {
      const varIdx = builder.addVariant(1);
      const inlineRecordIdx = builder.addRecord();
      const listIdx = builder.addList();
      const dataIndices: number[] = new Array(binaryReference.val.data.length);
      for (let i = 0; i < binaryReference.val.data.length; i++) {
        dataIndices[i] = builder.u8(binaryReference.val.data[i]);
      }
      builder.finishSeq(listIdx, dataIndices);
      const binaryTypeRecordIdx = builder.addRecord();
      const mimeTypeIdx = builder.string(binaryReference.val.binaryType.mimeType);
      builder.finishSeq(binaryTypeRecordIdx, [mimeTypeIdx]);
      builder.finishSeq(inlineRecordIdx, [listIdx, binaryTypeRecordIdx]);
      builder.finishChild(varIdx, inlineRecordIdx);
      return varIdx;
    }
  }
}

export function serializeTextReferenceToWitNodes(tsValue: any, builder: WitNodeBuilder): number {
  const textReference: TextReference = serializeTsValueToTextReference(tsValue);

  switch (textReference.tag) {
    case 'url': {
      const varIdx = builder.addVariant(0);
      const urlIdx = builder.string(textReference.val);
      builder.finishChild(varIdx, urlIdx);
      return varIdx;
    }
    case 'inline': {
      const varIdx = builder.addVariant(1);
      const inlineRecordIdx = builder.addRecord();
      const dataIdx = builder.string(textReference.val.data);
      let textTypeIdx: number;
      if (textReference.val.textType) {
        const optIdx = builder.addOptionSome();
        const textTypeRecordIdx = builder.addRecord();
        const langCodeIdx = builder.string(textReference.val.textType.languageCode);
        builder.finishSeq(textTypeRecordIdx, [langCodeIdx]);
        builder.finishChild(optIdx, textTypeRecordIdx);
        textTypeIdx = optIdx;
      } else {
        textTypeIdx = builder.optionNone();
      }
      builder.finishSeq(inlineRecordIdx, [dataIdx, textTypeIdx]);
      builder.finishChild(varIdx, inlineRecordIdx);
      return varIdx;
    }
  }
}
