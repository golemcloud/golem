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

import { typeMismatchInDeserialize } from './errors';
import { Value } from './Value';
import { AnalysedType } from '../types/analysedType';
import { Result } from '../../../host/result';

/**
 * converts a Value to a TypeScript value, based on AnalysedType
 *
 * @param value
 * @param analysedType
 */
export function deserialize(value: Value, analysedType: AnalysedType): any {
  if (
    value.kind === 'record' &&
    value.value.length === 0 &&
    analysedType.kind === 'tuple' &&
    analysedType.emptyType === 'null'
  ) {
    return null;
  }

  if (
    value.kind === 'record' &&
    value.value.length === 0 &&
    analysedType.kind === 'tuple' &&
    analysedType.emptyType === 'undefined'
  ) {
    return undefined;
  }

  if (
    value.kind === 'record' &&
    value.value.length === 0 &&
    analysedType.kind === 'tuple' &&
    analysedType.emptyType === 'void'
  ) {
    return undefined;
  }

  if (value.kind === 'option') {
    const caseValue = value.value;
    if (!caseValue) {
      if (analysedType.kind === 'option') {
        if (analysedType.emptyType === 'null') {
          return null;
        }

        return undefined;
      }

      return undefined;
    }

    const innerType = analysedType.kind === 'option' ? analysedType.value.inner : analysedType;

    return deserialize(caseValue, innerType);
  }

  if (value.kind === 'enum' && analysedType.kind === 'enum') {
    return analysedType.value.cases[value.value];
  }

  switch (analysedType.kind) {
    case 'bool':
      if (value.kind === 'bool') {
        return value.value;
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'boolean'));
      }

    case 'u64':
      if (analysedType.isBigInt) {
        return convertToBigInt(value);
      }

      return convertToNumber(value);

    case 's64':
      if (analysedType.isBigInt) {
        return convertToBigInt(value);
      }

      return convertToNumber(value);

    case 's8':
    case 'u8':
    case 's16':
    case 'u16':
    case 's32':
    case 'u32':
    case 'f32':
    case 'f64':
      return convertToNumber(value);

    case 'string':
      if (value.kind === 'string') {
        return value.value;
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'string'));
      }

    case 'list':
      const typedArray = analysedType.typedArray;

      if (typedArray) {
        switch (typedArray) {
          case 'u8':
            if (value.kind === 'list') {
              return new Uint8Array(value.value.map((v) => convertToNumber(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'Uint8Array'));
            }
          case 'u16':
            if (value.kind === 'list') {
              return new Uint16Array(value.value.map((v) => convertToNumber(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'Uint16Array'));
            }

          case 'u32':
            if (value.kind === 'list') {
              return new Uint32Array(value.value.map((v) => convertToNumber(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'Uint32Array'));
            }
          case 'big-u64':
            if (value.kind === 'list') {
              return new BigUint64Array(value.value.map((v) => convertToBigInt(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'BigUint64Array'));
            }

          case 'i8':
            if (value.kind === 'list') {
              return new Int8Array(value.value.map((v) => convertToNumber(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'Int8Array'));
            }

          case 'i16':
            if (value.kind === 'list') {
              return new Int16Array(value.value.map((v) => convertToNumber(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'Int16Array'));
            }
          case 'i32':
            if (value.kind === 'list') {
              return new Int32Array(value.value.map((v) => convertToNumber(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'Int32Array'));
            }
          case 'big-i64':
            if (value.kind === 'list') {
              return new BigInt64Array(value.value.map((v) => convertToBigInt(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'BigInt64Array'));
            }
          case 'f32':
            if (value.kind === 'list') {
              return new Float32Array(value.value.map((v) => convertToNumber(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'Float32Array'));
            }
          case 'f64':
            if (value.kind === 'list') {
              return new Float64Array(value.value.map((v) => convertToNumber(v)));
            } else {
              throw new Error(typeMismatchInDeserialize(value, 'Float64Array'));
            }
        }
      }

      // If it's a map type
      if (analysedType.mapType) {
        if (value.kind === 'list') {
          const elemType = analysedType.value.inner;

          if (!elemType || elemType.kind !== 'tuple' || elemType.value.items.length !== 2) {
            throw new Error(`Unable to infer the type of Map`);
          }

          const keyType = elemType.value.items[0];

          const valueType = elemType.value.items[1];

          const map = new Map();

          for (const item of value.value) {
            if (item.kind !== 'tuple' || item.value.length !== 2) {
              throw new Error(typeMismatchInDeserialize(item, 'map'));
            }

            const k = deserialize(item.value[0], keyType);
            const v = deserialize(item.value[1], valueType);
            map.set(k, v);
          }

          return map;
        } else {
          throw new Error(typeMismatchInDeserialize(value, 'map'));
        }
      }

      if (value.kind === 'list') {
        const elemType = analysedType.value.inner;

        if (!elemType) {
          throw new Error(`Unable to infer the type of Array`);
        }
        return value.value.map((item: Value) => deserialize(item, elemType));
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'array'));
      }

    case 'tuple':
      if (value.kind === 'tuple') {
        const emptyType = analysedType.emptyType;
        if (emptyType) {
          switch (emptyType) {
            case 'null':
              if (value.value.length === 0) return null;
              throw new Error(`Unable to infer the type of Array`);
            case 'void':
              if (value.value.length === 0) return undefined;
              throw new Error(`Unable to infer the type of Array`);
            case 'undefined':
              if (value.value.length === 0) return undefined;
              throw new Error(`Unable to infer the type of Array`);
          }
        }

        if (value.kind === 'tuple') {
          if (value.value.length !== analysedType.value.items.length) {
            throw new Error(typeMismatchInDeserialize(value, 'tuple'));
          }

          return value.value.map((item: Value, idx: number) =>
            deserialize(item, analysedType.value.items[idx]),
          );
        } else {
          throw new Error(typeMismatchInDeserialize(value, 'tuple'));
        }
      }

      throw new Error(typeMismatchInDeserialize(value, 'tuple'));

    case 'result':
      if (value.kind === 'result') {
        switch (analysedType.resultType.tag) {
          case 'inbuilt':
            const inbuiltOkType = analysedType.value.ok;
            const inbuiltErrType = analysedType.value.err;

            if (inbuiltOkType && value.value.ok) {
              return Result.ok(deserialize(value.value.ok, inbuiltOkType));
            }

            if (inbuiltErrType && value.value.err) {
              return Result.err(deserialize(value.value.err, inbuiltErrType));
            }

            if ('ok' in value.value && analysedType.resultType.okEmptyType) {
              switch (analysedType.resultType.okEmptyType) {
                case 'null':
                  return Result.ok(null);
                case 'void':
                  return Result.ok(undefined);
                case 'undefined':
                  return Result.ok(undefined);
              }
            }

            if ('err' in value.value && analysedType.resultType.errEmptyType) {
              switch (analysedType.resultType.errEmptyType) {
                case 'null':
                  return Result.err(null);
                case 'void':
                  return Result.err(undefined);
                case 'undefined':
                  return Result.err(undefined);
              }
            }

            throw new Error(typeMismatchInDeserialize(value, 'result'));

          case 'custom':
            const okName = analysedType.resultType.okValueName;
            const errName = analysedType.resultType.errValueName;
            const okType = analysedType.value.ok;
            const errType = analysedType.value.err;

            // ok type and err type exists and therefore deserialize the value
            // explicitly using their types
            if (okName && errName && okType && errType) {
              if (value.value.ok) {
                return {
                  tag: 'ok',
                  [okName]: deserialize(value.value.ok, okType),
                };
              }

              if (value.value.err) {
                return {
                  tag: 'err',
                  [errName]: deserialize(value.value.err, errType),
                };
              }
            }

            // err type doesn't exist, but ok type exists
            // if ok value exists, deserialize it using ok type
            // otherwise return just tag 'err'
            if (okName && okType && !errType) {
              if (value.value.ok) {
                return {
                  tag: 'ok',
                  [okName]: deserialize(value.value.ok, okType),
                };
              } else {
                return {
                  tag: 'err',
                };
              }
            }

            // ok type doesn't exist, but err type exists
            // if err value exists, deserialize it using err type
            // otherwise return just tag 'ok'
            if (errName && errType && !okType) {
              if (value.value.err) {
                return {
                  tag: 'err',
                  [errName]: deserialize(value.value.err, errType),
                };
              } else {
                return {
                  tag: 'ok',
                };
              }
            }

            // ok value is either undefined or null, however the ok type is `void`
            if (okName && !okType && 'ok' in value.value) {
              if (value.value.ok === undefined || value.value.ok === null) {
                return {
                  tag: 'ok',
                  [okName]: value.value.ok,
                };
              }
            }

            // err value is either undefined or null, however the err type is `void`
            if (errName && !errType && 'err' in value.value) {
              if (value.value.err === undefined || value.value.err === null) {
                return {
                  tag: 'err',
                  [errName]: value.value.err,
                };
              }
            }
        }
      }

      throw new Error(typeMismatchInDeserialize(value, 'result'));

    case 'variant':
      if (value.kind === 'variant') {
        const taggedMetadata = analysedType.taggedTypes;

        const variants = analysedType.value.cases;

        if (taggedMetadata.length > 0) {
          const caseType = variants[value.caseIdx];
          const tagValue = caseType.name;
          const valueType = caseType.typ;

          if (valueType) {
            const caseValue = value.caseValue;

            if (!caseValue) {
              if (valueType.kind === 'option') {
                return { tag: tagValue };
              }

              throw new Error(typeMismatchInDeserialize(value, 'union'));
            }

            const result = deserialize(caseValue, valueType);

            const metadata = analysedType.taggedTypes.find(
              (lit) => lit.tagLiteralName === tagValue,
            )?.valueType;

            if (!metadata) {
              throw new Error(typeMismatchInDeserialize(value, 'union'));
            }

            return {
              tag: tagValue,
              [metadata[0]]: result,
            };
          } else {
            return { tag: tagValue };
          }
        }

        const result = variants[value.caseIdx];
        const type = result.typ;

        if (!type) {
          return result.name;
        }

        const v = value.caseValue;

        if (!v) {
          throw new Error(typeMismatchInDeserialize(value, 'union'));
        }

        return deserialize(v, type);
      }

      throw new Error(typeMismatchInDeserialize(value, 'variant'));

    case 'record':
      if (value.kind === 'record') {
        const fieldValues = value.value;
        const expectedTypeFields = analysedType.value.fields;
        return expectedTypeFields.reduce(
          (acc, field, idx) => {
            const name = field.name;
            const expectedFieldType = field.typ;

            acc[name] = deserialize(fieldValues[idx], expectedFieldType);
            return acc;
          },
          {} as Record<string, any>,
        );
      } else {
        throw new Error(typeMismatchInDeserialize(value, 'object'));
      }
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
