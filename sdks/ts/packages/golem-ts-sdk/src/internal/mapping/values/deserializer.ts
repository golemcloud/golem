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

import { WitNode } from 'golem:core/types@1.5.0';
import { typeMismatchInDeserialize } from './errors';
import { AnalysedType } from '../types/analysedType';
import { Result } from '../../../host/result';

export function deserializeNodes(nodes: WitNode[], index: number, analysedType: AnalysedType): any {
  const n = nodes[index];
  const tag = n.tag;

  // Handle empty record → null/undefined/void
  if (
    tag === 'record-value' &&
    (n.val as number[]).length === 0 &&
    analysedType.kind === 'tuple'
  ) {
    if (analysedType.emptyType) {
      switch (analysedType.emptyType) {
        case 'null':
          return null;
        case 'void':
        case 'undefined':
          return undefined;
      }
    }
  }

  // Handle option
  if (tag === 'option-value') {
    if (n.val === undefined) {
      // None
      if (analysedType.kind === 'option') {
        if (analysedType.emptyType === 'null') return null;
        return undefined;
      }
      return undefined;
    }
    // Some
    const innerType = analysedType.kind === 'option' ? analysedType.value.inner : analysedType;
    return deserializeNodes(nodes, n.val as number, innerType);
  }

  // Handle enum
  if (analysedType.kind === 'enum') {
    if (tag !== 'enum-value') throw new Error(typeMismatchInDeserialize(tag, 'enum'));
    return analysedType.value.cases[n.val as number];
  }

  switch (analysedType.kind) {
    case 'bool': {
      if (tag !== 'prim-bool') throw new Error(typeMismatchInDeserialize(tag, 'boolean'));
      return n.val;
    }

    case 'u64':
      if (analysedType.isBigInt) return convertToBigInt(nodes, index);
      return convertToNumber(nodes, index);

    case 's64':
      if (analysedType.isBigInt) return convertToBigInt(nodes, index);
      return convertToNumber(nodes, index);

    case 's8':
    case 'u8':
    case 's16':
    case 'u16':
    case 's32':
    case 'u32':
    case 'f32':
    case 'f64':
      return convertToNumber(nodes, index);

    case 'string': {
      if (tag !== 'prim-string') throw new Error(typeMismatchInDeserialize(tag, 'string'));
      return n.val;
    }

    case 'list': {
      if (tag !== 'list-value') {
        const expectedType = analysedType.typedArray ?? (analysedType.mapType ? 'map' : 'array');
        throw new Error(typeMismatchInDeserialize(tag, expectedType));
      }
      const childIndices = n.val as number[];
      const len = childIndices.length;
      const typedArray = analysedType.typedArray;

      if (typedArray) {
        switch (typedArray) {
          case 'u8': {
            const arr = new Uint8Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToNumber(nodes, childIndices[i]);
            return arr;
          }
          case 'u16': {
            const arr = new Uint16Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToNumber(nodes, childIndices[i]);
            return arr;
          }
          case 'u32': {
            const arr = new Uint32Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToNumber(nodes, childIndices[i]);
            return arr;
          }
          case 'big-u64': {
            const arr = new BigUint64Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToBigInt(nodes, childIndices[i]);
            return arr;
          }
          case 'i8': {
            const arr = new Int8Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToNumber(nodes, childIndices[i]);
            return arr;
          }
          case 'i16': {
            const arr = new Int16Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToNumber(nodes, childIndices[i]);
            return arr;
          }
          case 'i32': {
            const arr = new Int32Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToNumber(nodes, childIndices[i]);
            return arr;
          }
          case 'big-i64': {
            const arr = new BigInt64Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToBigInt(nodes, childIndices[i]);
            return arr;
          }
          case 'f32': {
            const arr = new Float32Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToNumber(nodes, childIndices[i]);
            return arr;
          }
          case 'f64': {
            const arr = new Float64Array(len);
            for (let i = 0; i < len; i++) arr[i] = convertToNumber(nodes, childIndices[i]);
            return arr;
          }
        }
      }

      // Map type
      if (analysedType.mapType) {
        const elemType = analysedType.value.inner;
        if (!elemType || elemType.kind !== 'tuple' || elemType.value.items.length !== 2) {
          throw new Error(`Unable to infer the type of Map`);
        }

        const keyType = elemType.value.items[0];
        const valueType = elemType.value.items[1];
        const map = new Map();

        for (let i = 0; i < len; i++) {
          const tupleIdx = childIndices[i];
          const tupleNode = nodes[tupleIdx];
          if (tupleNode.tag !== 'tuple-value') {
            throw new Error(typeMismatchInDeserialize(tupleNode.tag, 'map'));
          }
          const tupleChildren = tupleNode.val as number[];
          if (tupleChildren.length < 2) {
            throw new Error(typeMismatchInDeserialize(tupleNode.tag, 'map'));
          }
          const k = deserializeNodes(nodes, tupleChildren[0], keyType);
          const v = deserializeNodes(nodes, tupleChildren[1], valueType);
          map.set(k, v);
        }

        return map;
      }

      // Regular list
      const elemType = analysedType.value.inner;
      if (!elemType) throw new Error(`Unable to infer the type of Array`);
      const result = new Array(len);
      for (let i = 0; i < len; i++) {
        result[i] = deserializeNodes(nodes, childIndices[i], elemType);
      }
      return result;
    }

    case 'tuple': {
      const emptyType = analysedType.emptyType;

      if (tag === 'tuple-value') {
        const tupleChildren = n.val as number[];
        const tupleLen = tupleChildren.length;

        if (emptyType) {
          switch (emptyType) {
            case 'null':
              if (tupleLen === 0) return null;
              throw new Error(`Unable to infer the type of Array`);
            case 'void':
            case 'undefined':
              if (tupleLen === 0) return undefined;
              throw new Error(`Unable to infer the type of Array`);
          }
        }

        if (tupleLen !== analysedType.value.items.length) {
          throw new Error(typeMismatchInDeserialize(tag, 'tuple'));
        }

        const result: any[] = new Array(tupleLen);
        for (let i = 0; i < tupleLen; i++) {
          result[i] = deserializeNodes(nodes, tupleChildren[i], analysedType.value.items[i]);
        }
        return result;
      }

      throw new Error(typeMismatchInDeserialize(tag, 'tuple'));
    }

    case 'result': {
      if (tag !== 'result-value') throw new Error(typeMismatchInDeserialize(tag, 'result'));
      const resVal = n.val as { tag: 'ok' | 'err'; val?: number };
      const resTag = resVal.tag;
      const innerIdx = resVal.val;

      switch (analysedType.resultType.tag) {
        case 'inbuilt': {
          const inbuiltOkType = analysedType.value.ok;
          const inbuiltErrType = analysedType.value.err;

          if (inbuiltOkType && resTag === 'ok' && innerIdx !== undefined) {
            return Result.ok(deserializeNodes(nodes, innerIdx, inbuiltOkType));
          }

          if (inbuiltErrType && resTag === 'err' && innerIdx !== undefined) {
            return Result.err(deserializeNodes(nodes, innerIdx, inbuiltErrType));
          }

          if (resTag === 'ok' && innerIdx === undefined && analysedType.resultType.okEmptyType) {
            switch (analysedType.resultType.okEmptyType) {
              case 'null':
                return Result.ok(null);
              case 'void':
              case 'undefined':
                return Result.ok(undefined);
            }
          }

          if (resTag === 'err' && innerIdx === undefined && analysedType.resultType.errEmptyType) {
            switch (analysedType.resultType.errEmptyType) {
              case 'null':
                return Result.err(null);
              case 'void':
              case 'undefined':
                return Result.err(undefined);
            }
          }

          throw new Error(typeMismatchInDeserialize(tag, 'result'));
        }

        case 'custom': {
          const okName = analysedType.resultType.okValueName;
          const errName = analysedType.resultType.errValueName;
          const okType = analysedType.value.ok;
          const errType = analysedType.value.err;

          if (okName && errName && okType && errType) {
            if (resTag === 'ok' && innerIdx !== undefined) {
              return { tag: 'ok', [okName]: deserializeNodes(nodes, innerIdx, okType) };
            }
            if (resTag === 'err' && innerIdx !== undefined) {
              return { tag: 'err', [errName]: deserializeNodes(nodes, innerIdx, errType) };
            }
          }

          if (okName && okType && !errType) {
            if (resTag === 'ok' && innerIdx !== undefined) {
              return { tag: 'ok', [okName]: deserializeNodes(nodes, innerIdx, okType) };
            } else {
              return { tag: 'err' };
            }
          }

          if (errName && errType && !okType) {
            if (resTag === 'err' && innerIdx !== undefined) {
              return { tag: 'err', [errName]: deserializeNodes(nodes, innerIdx, errType) };
            } else {
              return { tag: 'ok' };
            }
          }

          if (okName && !okType && resTag === 'ok') {
            if (innerIdx === undefined) {
              return { tag: 'ok', [okName]: undefined };
            }
          }

          if (errName && !errType && resTag === 'err') {
            if (innerIdx === undefined) {
              return { tag: 'err', [errName]: undefined };
            }
          }

          throw new Error(typeMismatchInDeserialize(tag, 'result'));
        }
      }
    }

    case 'variant': {
      if (tag !== 'variant-value') throw new Error(typeMismatchInDeserialize(tag, 'variant'));
      const [caseIdx, maybeChildIdx] = n.val as [number, number | undefined];

      const taggedMetadata = analysedType.taggedTypes;
      const variants = analysedType.value.cases;

      if (taggedMetadata.length > 0) {
        const caseType = variants[caseIdx];
        const tagValue = caseType.name;
        const valueType = caseType.typ;

        if (valueType) {
          if (maybeChildIdx === undefined) {
            if (valueType.kind === 'option') {
              return { tag: tagValue };
            }
            throw new Error(typeMismatchInDeserialize(tag, 'union'));
          }

          const result = deserializeNodes(nodes, maybeChildIdx, valueType);

          const metadata = analysedType.taggedTypes.find(
            (lit) => lit.tagLiteralName === tagValue,
          )?.valueType;

          if (!metadata) {
            throw new Error(typeMismatchInDeserialize(tag, 'union'));
          }

          return { tag: tagValue, [metadata[0]]: result };
        } else {
          return { tag: tagValue };
        }
      }

      const variantCase = variants[caseIdx];
      const type = variantCase.typ;

      if (!type) {
        return variantCase.name;
      }

      if (maybeChildIdx === undefined) {
        throw new Error(typeMismatchInDeserialize(tag, 'union'));
      }

      return deserializeNodes(nodes, maybeChildIdx, type);
    }

    case 'record': {
      if (tag !== 'record-value') throw new Error(typeMismatchInDeserialize(tag, 'object'));
      const childIndices = n.val as number[];
      const fields = analysedType.value.fields;
      const obj: Record<string, any> = {};
      for (let i = 0; i < fields.length; i++) {
        obj[fields[i].name] = deserializeNodes(nodes, childIndices[i], fields[i].typ);
      }
      return obj;
    }
  }
}

function convertToNumber(nodes: WitNode[], index: number): number {
  const n = nodes[index];
  switch (n.tag) {
    case 'prim-u8':
    case 'prim-u16':
    case 'prim-u32':
    case 'prim-s8':
    case 'prim-s16':
    case 'prim-s32':
    case 'prim-float32':
    case 'prim-float64':
      return n.val as number;
    case 'prim-u64':
    case 'prim-s64':
      return Number(n.val);
    default:
      throw new Error(typeMismatchInDeserialize(n.tag, 'number'));
  }
}

function convertToBigInt(nodes: WitNode[], index: number): bigint {
  const n = nodes[index];
  if (n.tag === 'prim-u64' || n.tag === 'prim-s64') return n.val as bigint;
  throw new Error(typeMismatchInDeserialize(n.tag, 'bigint'));
}
