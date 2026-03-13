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

import { typeMismatchInDeserialize } from './errors';
import { AnalysedType } from '../types/analysedType';
import { Result } from '../../../host/result';
import { WitNodeExtractor } from './WitNodeExtractor';

export function deserializeFromExtractor(extractor: WitNodeExtractor, analysedType: AnalysedType): any {
  // Handle empty record → null/undefined/void
  if (
    extractor.isRecord() &&
    extractor.recordLength() === 0 &&
    analysedType.kind === 'tuple'
  ) {
    // record-value with 0 children used as empty tuple (host compatibility)
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
  if (extractor.isOption()) {
    const optResult = extractor.option();
    if (optResult === undefined) {
      throw new Error(typeMismatchInDeserialize(extractor.tag(), 'option'));
    }
    if (optResult === null) {
      // None
      if (analysedType.kind === 'option') {
        if (analysedType.emptyType === 'null') return null;
        return undefined;
      }
      return undefined;
    }
    // Some
    const innerType = analysedType.kind === 'option' ? analysedType.value.inner : analysedType;
    return deserializeFromExtractor(optResult, innerType);
  }

  // Handle enum
  if (analysedType.kind === 'enum') {
    const enumIdx = extractor.enumValue();
    if (enumIdx === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'enum'));
    return analysedType.value.cases[enumIdx];
  }

  switch (analysedType.kind) {
    case 'bool': {
      const val = extractor.bool();
      if (val === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'boolean'));
      return val;
    }

    case 'u64':
      if (analysedType.isBigInt) return convertToBigIntFromExtractor(extractor);
      return convertToNumberFromExtractor(extractor);

    case 's64':
      if (analysedType.isBigInt) return convertToBigIntFromExtractor(extractor);
      return convertToNumberFromExtractor(extractor);

    case 's8':
    case 'u8':
    case 's16':
    case 'u16':
    case 's32':
    case 'u32':
    case 'f32':
    case 'f64':
      return convertToNumberFromExtractor(extractor);

    case 'string': {
      const val = extractor.string();
      if (val === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'string'));
      return val;
    }

    case 'list': {
      const typedArray = analysedType.typedArray;

      if (typedArray) {
        switch (typedArray) {
          case 'u8': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'Uint8Array'));
            const arr = new Uint8Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToNumberFromExtractor(elem);
            }
            return arr;
          }
          case 'u16': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'Uint16Array'));
            const arr = new Uint16Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToNumberFromExtractor(elem);
            }
            return arr;
          }
          case 'u32': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'Uint32Array'));
            const arr = new Uint32Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToNumberFromExtractor(elem);
            }
            return arr;
          }
          case 'big-u64': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'BigUint64Array'));
            const arr = new BigUint64Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToBigIntFromExtractor(elem);
            }
            return arr;
          }
          case 'i8': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'Int8Array'));
            const arr = new Int8Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToNumberFromExtractor(elem);
            }
            return arr;
          }
          case 'i16': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'Int16Array'));
            const arr = new Int16Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToNumberFromExtractor(elem);
            }
            return arr;
          }
          case 'i32': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'Int32Array'));
            const arr = new Int32Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToNumberFromExtractor(elem);
            }
            return arr;
          }
          case 'big-i64': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'BigInt64Array'));
            const arr = new BigInt64Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToBigIntFromExtractor(elem);
            }
            return arr;
          }
          case 'f32': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'Float32Array'));
            const arr = new Float32Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToNumberFromExtractor(elem);
            }
            return arr;
          }
          case 'f64': {
            const len = extractor.listLength();
            if (len === undefined) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'Float64Array'));
            const arr = new Float64Array(len);
            for (let i = 0; i < len; i++) {
              const elem = extractor.listElement(i)!;
              arr[i] = convertToNumberFromExtractor(elem);
            }
            return arr;
          }
        }
      }

      // Map type
      if (analysedType.mapType) {
        const elements = extractor.listElements();
        if (!elements) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'map'));

        const elemType = analysedType.value.inner;
        if (!elemType || elemType.kind !== 'tuple' || elemType.value.items.length !== 2) {
          throw new Error(`Unable to infer the type of Map`);
        }

        const keyType = elemType.value.items[0];
        const valueType = elemType.value.items[1];
        const map = new Map();

        for (const tupleExtractor of elements) {
          const keyExtractor = tupleExtractor.tupleElement(0);
          const valueExtractor = tupleExtractor.tupleElement(1);
          if (!keyExtractor || !valueExtractor) {
            throw new Error(typeMismatchInDeserialize(tupleExtractor.tag(), 'map'));
          }
          const k = deserializeFromExtractor(keyExtractor, keyType);
          const v = deserializeFromExtractor(valueExtractor, valueType);
          map.set(k, v);
        }

        return map;
      }

      // Regular list
      const elements = extractor.listElements();
      if (!elements) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'array'));

      const elemType = analysedType.value.inner;
      if (!elemType) throw new Error(`Unable to infer the type of Array`);
      return elements.map((e) => deserializeFromExtractor(e, elemType));
    }

    case 'tuple': {
      const emptyType = analysedType.emptyType;
      const tupleLen = extractor.tupleLength();

      if (tupleLen !== undefined) {
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
          throw new Error(typeMismatchInDeserialize(extractor.tag(), 'tuple'));
        }

        const result: any[] = new Array(tupleLen);
        for (let i = 0; i < tupleLen; i++) {
          const elemExtractor = extractor.tupleElement(i)!;
          result[i] = deserializeFromExtractor(elemExtractor, analysedType.value.items[i]);
        }
        return result;
      }

      throw new Error(typeMismatchInDeserialize(extractor.tag(), 'tuple'));
    }

    case 'result': {
      const res = extractor.result();
      if (!res) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'result'));

      switch (analysedType.resultType.tag) {
        case 'inbuilt': {
          const inbuiltOkType = analysedType.value.ok;
          const inbuiltErrType = analysedType.value.err;

          if (inbuiltOkType && res.tag === 'ok' && res.inner) {
            return Result.ok(deserializeFromExtractor(res.inner, inbuiltOkType));
          }

          if (inbuiltErrType && res.tag === 'err' && res.inner) {
            return Result.err(deserializeFromExtractor(res.inner, inbuiltErrType));
          }

          if (res.tag === 'ok' && !res.inner && analysedType.resultType.okEmptyType) {
            switch (analysedType.resultType.okEmptyType) {
              case 'null':
                return Result.ok(null);
              case 'void':
              case 'undefined':
                return Result.ok(undefined);
            }
          }

          if (res.tag === 'err' && !res.inner && analysedType.resultType.errEmptyType) {
            switch (analysedType.resultType.errEmptyType) {
              case 'null':
                return Result.err(null);
              case 'void':
              case 'undefined':
                return Result.err(undefined);
            }
          }

          throw new Error(typeMismatchInDeserialize(extractor.tag(), 'result'));
        }

        case 'custom': {
          const okName = analysedType.resultType.okValueName;
          const errName = analysedType.resultType.errValueName;
          const okType = analysedType.value.ok;
          const errType = analysedType.value.err;

          if (okName && errName && okType && errType) {
            if (res.tag === 'ok' && res.inner) {
              return { tag: 'ok', [okName]: deserializeFromExtractor(res.inner, okType) };
            }
            if (res.tag === 'err' && res.inner) {
              return { tag: 'err', [errName]: deserializeFromExtractor(res.inner, errType) };
            }
          }

          if (okName && okType && !errType) {
            if (res.tag === 'ok' && res.inner) {
              return { tag: 'ok', [okName]: deserializeFromExtractor(res.inner, okType) };
            } else {
              return { tag: 'err' };
            }
          }

          if (errName && errType && !okType) {
            if (res.tag === 'err' && res.inner) {
              return { tag: 'err', [errName]: deserializeFromExtractor(res.inner, errType) };
            } else {
              return { tag: 'ok' };
            }
          }

          if (okName && !okType && res.tag === 'ok') {
            if (!res.inner) {
              return { tag: 'ok', [okName]: undefined };
            }
          }

          if (errName && !errType && res.tag === 'err') {
            if (!res.inner) {
              return { tag: 'err', [errName]: undefined };
            }
          }

          throw new Error(typeMismatchInDeserialize(extractor.tag(), 'result'));
        }
      }
    }

    case 'variant': {
      const v = extractor.variant();
      if (!v) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'variant'));

      const taggedMetadata = analysedType.taggedTypes;
      const variants = analysedType.value.cases;

      if (taggedMetadata.length > 0) {
        const caseType = variants[v.caseIdx];
        const tagValue = caseType.name;
        const valueType = caseType.typ;

        if (valueType) {
          if (!v.inner) {
            if (valueType.kind === 'option') {
              return { tag: tagValue };
            }
            throw new Error(typeMismatchInDeserialize(extractor.tag(), 'union'));
          }

          const result = deserializeFromExtractor(v.inner, valueType);

          const metadata = analysedType.taggedTypes.find(
            (lit) => lit.tagLiteralName === tagValue,
          )?.valueType;

          if (!metadata) {
            throw new Error(typeMismatchInDeserialize(extractor.tag(), 'union'));
          }

          return { tag: tagValue, [metadata[0]]: result };
        } else {
          return { tag: tagValue };
        }
      }

      const variantCase = variants[v.caseIdx];
      const type = variantCase.typ;

      if (!type) {
        return variantCase.name;
      }

      if (!v.inner) {
        throw new Error(typeMismatchInDeserialize(extractor.tag(), 'union'));
      }

      return deserializeFromExtractor(v.inner, type);
    }

    case 'record': {
      const fields = analysedType.value.fields;
      const obj: Record<string, any> = {};
      for (let i = 0; i < fields.length; i++) {
        const child = extractor.field(i);
        if (!child) throw new Error(typeMismatchInDeserialize(extractor.tag(), 'object'));
        obj[fields[i].name] = deserializeFromExtractor(child, fields[i].typ);
      }
      return obj;
    }
  }
}

function convertToNumberFromExtractor(extractor: WitNodeExtractor): number {
  const val = extractor.number();
  if (val !== undefined) return val;
  throw new Error(typeMismatchInDeserialize(extractor.tag(), 'number'));
}

function convertToBigIntFromExtractor(extractor: WitNodeExtractor): bigint {
  const val = extractor.bigint();
  if (val !== undefined) return val;
  throw new Error(typeMismatchInDeserialize(extractor.tag(), 'bigint'));
}
