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

import { AnalysedType, NameOptionTypePair, NameTypePair, TypedArray } from '../types/analysedType';
import * as Either from '../../../newTypes/either';
import {
  customSerializationError,
  enumMismatchInSerialize,
  missingObjectKey,
  typeMismatchInSerialize,
  unhandledTypeError,
  unionTypeMatchError,
} from './errors';
import { TaggedTypeMetadata } from '../types/taggedUnion';
import { Value } from './Value';
import { BinaryReference, TextReference } from 'golem:agent/common@1.5.0';
import * as util from 'node:util';

/**
 * Converts a TypeScript value to a `Value` (one level before it becomes WitValue)
 * based on the provided AnalysedType.
 *
 * Serialization of a TypeScript mainly required at RPC boundary
 * as well as when a result of a method needs to be sent through to golem executor.
 *
 * @param tsValue The TypeScript value that exists as `unknown` type, which represents anything other than unstructured-text or unstructured-binary.
 * @param analysedType The expected AnalysedType of the typescript value. There is no `AnalysedType` as such for unstructured-text or unstructured-binary.
 */
export function serializeDefaultTsValue(
  tsValue: unknown,
  analysedType: AnalysedType,
): Either.Either<Value, string> {
  switch (analysedType.kind) {
    case 'flags':
      return Either.left(unhandledTypeError(tsValue, 'flags', undefined));
    case 'chr':
      return Either.left(unhandledTypeError(tsValue, 'char', undefined));
    case 'f32':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'f32',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'number'));
      }
    case 's64':
      if (typeof tsValue === 'bigint') {
        return Either.right({
          kind: 's64',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'number'));
      }
    case 'u32':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'u32',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'number'));
      }
    case 's32':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 's32',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'number'));
      }
    case 'u16':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'u16',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'number'));
      }
    case 's16':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 's16',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'number'));
      }
    case 'u8':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'u8',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'number'));
      }
    case 's8':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 's8',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'number'));
      }
    case 'handle':
      return Either.left(unhandledTypeError(tsValue, 'handle', undefined));
    case 'bool':
      return serializeBooleanTsValue(tsValue);

    case 'f64':
      if (typeof tsValue === 'number') {
        return Either.right({
          kind: 'f64',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'number'));
      }

    case 'u64':
      if (typeof tsValue === 'bigint' || typeof tsValue === 'number') {
        return Either.right({
          kind: 'u64',
          value: typeof tsValue === 'bigint' ? tsValue : BigInt(tsValue),
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'bigint'));
      }

    case 'string':
      if (typeof tsValue === 'string') {
        return Either.right({
          kind: 'string',
          value: tsValue,
        });
      } else {
        return Either.left(typeMismatchInSerialize(tsValue, 'string'));
      }

    case 'option':
      const innerType = analysedType.value.inner;

      if (tsValue === null || tsValue === undefined) {
        return Either.right({
          kind: 'option',
        });
      } else {
        return Either.map(serializeDefaultTsValue(tsValue, innerType), (v) => ({
          kind: 'option',
          value: v,
        }));
      }

    case 'list':
      const innerListType = analysedType.value.inner;
      const typedArray = analysedType.typedArray;

      if (typedArray) {
        switch (typedArray) {
          case 'u8':
            if (tsValue instanceof Uint8Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'Uint8Array'));
            }
          case 'u16':
            if (tsValue instanceof Uint16Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'Uint16Array'));
            }
          case 'u32':
            if (tsValue instanceof Uint32Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'Uint32Array'));
            }
          case 'big-u64':
            if (tsValue instanceof BigUint64Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'BigUint64Array'));
            }
          case 'i8':
            if (tsValue instanceof Int8Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'Int8Array'));
            }
          case 'i16':
            if (tsValue instanceof Int16Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'Int16Array'));
            }
          case 'i32':
            if (tsValue instanceof Int32Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'Int32Array'));
            }
          case 'big-i64':
            if (tsValue instanceof BigInt64Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'BigInt64Array'));
            }
          case 'f32':
            if (tsValue instanceof Float32Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'Float32Array'));
            }
          case 'f64':
            if (tsValue instanceof Float64Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) => serializeDefaultTsValue(item, innerListType)),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(typeMismatchInSerialize(tsValue, 'Float64Array'));
            }
        }
      }

      if (Array.isArray(tsValue)) {
        return Either.map(
          Either.all(tsValue.map((item) => serializeDefaultTsValue(item, innerListType))),
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
          return Either.left(typeMismatchInSerialize(tsValue, 'Map'));
        }

        const keyType = innerListType.value.items[0];

        const valueType = innerListType.value.items[1];

        return serializeKeyValuePairs(tsValue, innerListType, keyType, valueType);
      }

      return Either.left(typeMismatchInSerialize(tsValue, 'Array'));

    case 'tuple':
      const analysedTypeTupleElems = analysedType.value.items;

      if (analysedTypeTupleElems.length === 0) {
        if (tsValue === null || tsValue === undefined) {
          return Either.right({
            kind: 'tuple',
            value: [],
          });
        } else {
          return Either.left(
            typeMismatchInSerialize(tsValue, `Array of length ${analysedTypeTupleElems.length}`),
          );
        }
      }

      return serializeTupleTsValue(tsValue, analysedTypeTupleElems);

    case 'variant':
      const variantTypes = analysedType.value.cases;
      const taggedTypes = analysedType.taggedTypes;

      return serializeUnionTsValue(tsValue, taggedTypes, variantTypes);

    case 'enum':
      if (typeof tsValue === 'string' && analysedType.value.cases.includes(tsValue.toString())) {
        const value: Value = {
          kind: 'enum',
          value: analysedType.value.cases.indexOf(tsValue.toString()),
        };

        return Either.right(value);
      } else {
        return Either.left(enumMismatchInSerialize(analysedType.value.cases, tsValue));
      }

    case 'record':
      const nameTypePairs = analysedType.value.fields;

      return serializeObjectTsValue(tsValue, analysedType, nameTypePairs);

    case 'result':
      const okType = analysedType.value.ok;
      const errType = analysedType.value.err;

      if (typeof tsValue !== 'object' || tsValue === null) {
        return Either.left(typeMismatchInSerialize(tsValue, 'object'));
      }

      if (!('tag' in tsValue)) {
        return Either.left(missingObjectKey('tag', tsValue));
      }

      const resultValue = tsValue as Record<string, unknown>;

      switch (analysedType.resultType.tag) {
        case 'inbuilt':
          const keys = Object.keys(resultValue);

          if (!keys.includes('tag')) {
            return Either.left(missingObjectKey('tag', resultValue));
          }

          if (!keys.includes('val')) {
            return Either.left(missingObjectKey('val', resultValue));
          }

          if (resultValue['tag'] === 'ok') {
            if (!okType) {
              if (analysedType.resultType.okEmptyType) {
                return Either.right({
                  kind: 'result',
                  value: {
                    ok: undefined,
                  },
                });
              }

              return Either.left(customSerializationError('unresolved ok type'));
            }

            return Either.map(serializeDefaultTsValue(resultValue['val'], okType), (v) => ({
              kind: 'result',
              value: {
                ok: v,
              },
            }));
          }

          if (resultValue['tag'] === 'err') {
            if (!errType) {
              if (analysedType.resultType.errEmptyType) {
                return Either.right({
                  kind: 'result',
                  value: {
                    err: undefined,
                  },
                });
              }

              return Either.left(customSerializationError('unresolved err type'));
            }

            return Either.map(serializeDefaultTsValue(resultValue['val'], errType), (v) => ({
              kind: 'result',
              value: {
                err: v,
              },
            }));
          }

          return Either.left(typeMismatchInSerialize(resultValue, 'Result'));
        case 'custom':
          const okValueName = analysedType.resultType.okValueName;
          const errValueName = analysedType.resultType.errValueName;

          if (resultValue['tag'] === 'ok') {
            // If ok type exists, we ensure that we have ok value, else return error
            // If ok type doesn't exist, we set ok value to undefined
            if (okType) {
              if (!okValueName) {
                return Either.left(customSerializationError('unresolved key name for ok value'));
              }

              return Either.map(serializeDefaultTsValue(resultValue[okValueName], okType), (v) => ({
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
          } else if (resultValue['tag'] === 'err') {
            // If err type exists, we ensure that we have err value, else return error
            // If err type doesn't exist, we set err value to undefined
            if (errType) {
              if (!errValueName) {
                return Either.left(customSerializationError('unresolved key name for err value'));
              }

              return Either.map(
                serializeDefaultTsValue(resultValue[errValueName], errType),
                (v) => ({
                  kind: 'result',
                  value: {
                    err: v,
                  },
                }),
              );
            }

            return Either.right({
              kind: 'result',
              value: {
                err: undefined,
              },
            });
          } else {
            return Either.left(typeMismatchInSerialize(tsValue, 'object with tag property'));
          }
      }
  }
}

export function serializeBinaryReferenceTsValue(tsValue: unknown): Value {
  const binaryReference = serializeTsValueToBinaryReference(tsValue);

  switch (binaryReference.tag) {
    case 'url':
      return {
        kind: 'variant',
        caseIdx: 0,
        caseValue: { kind: 'string', value: binaryReference.val },
      };

    case 'inline':
      return {
        kind: 'variant',
        caseIdx: 1,
        caseValue: {
          kind: 'record',
          value: [
            {
              kind: 'list',
              value: Array.from(binaryReference.val.data).map((b) => ({
                kind: 'u8',
                value: b,
              })),
            },
            {
              kind: 'record',
              value: [
                {
                  kind: 'string',
                  value: binaryReference.val.binaryType.mimeType,
                },
              ],
            },
          ],
        },
      };
  }
}

export function serializeTextReferenceTsValue(tsValue: unknown): Value {
  const textReference: TextReference = serializeTsValueToTextReference(tsValue);

  switch (textReference.tag) {
    case 'url':
      return {
        kind: 'variant',
        caseIdx: 0,
        caseValue: { kind: 'string', value: textReference.val },
      };

    case 'inline':
      if (textReference.val.textType) {
        return {
          kind: 'variant',
          caseIdx: 1,
          caseValue: {
            kind: 'record',
            value: [
              { kind: 'string', value: textReference.val.data },
              {
                kind: 'option',
                value: {
                  kind: 'record',
                  value: [
                    {
                      kind: 'string',
                      value: textReference.val.textType.languageCode,
                    },
                  ],
                },
              },
            ],
          },
        };
      }

      return {
        kind: 'variant',
        caseIdx: 1,
        caseValue: {
          kind: 'record',
          value: [{ kind: 'string', value: textReference.val.data }, { kind: 'option' }],
        },
      };
  }
}

export function serializeTsValueToBinaryReference(tsValue: unknown): BinaryReference {
  if (typeof tsValue === 'object' && tsValue !== null) {
    const obj = tsValue as Record<string, unknown>;
    const keys = Object.keys(obj);

    if (!keys.includes('tag')) {
      throw new Error(
        `Unable to cast value ${util.format(
          tsValue,
        )} to UnstructuredBinary. Missing 'tag' property.`,
      );
    }

    const tag = obj['tag'];

    if (typeof tag === 'string' && tag === 'url') {
      if (keys.includes('val')) {
        return {
          tag: 'url',
          val: obj['val'] as string,
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
            data: obj['val'] as Uint8Array,
            binaryType: {
              mimeType: obj['mimeType'] as string,
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

export function serializeTsValueToTextReference(value: unknown): TextReference {
  if (typeof value === 'object' && value !== null) {
    const obj = value as Record<string, unknown>;
    const keys = Object.keys(obj);

    if (!keys.includes('tag')) {
      throw new Error(
        `Unable to cast value ${util.format(value)} to UnstructuredText. Missing 'tag' property.`,
      );
    }

    const tag = obj['tag'];

    if (typeof tag === 'string' && tag === 'url') {
      if (keys.includes('val')) {
        return {
          tag: 'url',
          val: obj['val'] as string,
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
              data: obj['val'] as string,
              textType: {
                languageCode: obj['languageCode'] as string,
              },
            },
          };
        } else {
          return {
            tag: 'inline',
            val: {
              data: obj['val'] as string,
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

function serializeBooleanTsValue(tsValue: unknown): Either.Either<Value, string> {
  if (typeof tsValue === 'boolean') {
    return Either.right({
      kind: 'bool',
      value: tsValue,
    });
  } else {
    return Either.left(typeMismatchInSerialize(tsValue, 'boolean'));
  }
}

function serializeKeyValuePairs(
  tsValue: unknown,
  analysedType: AnalysedType,
  keyAnalysedType: AnalysedType,
  valueAnalysedType: AnalysedType,
): Either.Either<Value, string> {
  if (!(tsValue instanceof Map)) {
    return Either.left(typeMismatchInSerialize(tsValue, 'Map'));
  }

  const values = Either.all(
    Array.from(tsValue.entries()).map(([key, value]) =>
      Either.zipWith(
        serializeDefaultTsValue(key, keyAnalysedType),
        serializeDefaultTsValue(value, valueAnalysedType),
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

function serializeObjectTsValue(
  tsValue: unknown,
  analysedType: AnalysedType,
  nameTypePairs: NameTypePair[],
): Either.Either<Value, string> {
  if (typeof tsValue !== 'object' || tsValue === null) {
    return Either.left(typeMismatchInSerialize(tsValue, 'object'));
  }
  const obj = tsValue as Record<string, unknown>;
  const values: Value[] = [];

  for (const prop of nameTypePairs) {
    const key = prop.name;

    const type = prop.typ;

    if (!Object.prototype.hasOwnProperty.call(obj, key)) {
      if (type.kind === 'option') {
        values.push({
          kind: 'option',
        });
        continue;
      }
    }

    const nameTypePair = nameTypePairs.find((nt) => nt.name === key);

    if (!nameTypePair) {
      return Either.left(customSerializationError('unresolved name-type pair'));
    }

    const fieldVal = serializeDefaultTsValue(obj[key], nameTypePair.typ);

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

function serializeUnionTsValue(
  tsValue: unknown,
  taggedTypes: TaggedTypeMetadata[],
  nameOptionTypePairs: NameOptionTypePair[],
): Either.Either<Value, string> {
  if (taggedTypes.length > 0) {
    return serializeTaggedUnionTsValue(tsValue, nameOptionTypePairs);
  }

  for (const [idx, variant] of nameOptionTypePairs.entries()) {
    const analysedType = variant.typ;

    if (!analysedType) {
      if (tsValue === variant.name) {
        const value: Value = {
          kind: 'variant',
          caseIdx: nameOptionTypePairs.findIndex((v) => v.name === variant.name),
        };

        return Either.right(value);
      }

      continue;
    }

    const matches = matchesType(tsValue, analysedType);

    if (matches) {
      const value: Value = {
        kind: 'variant',
        caseIdx: idx,
        caseValue: Either.getOrThrowWith(
          serializeDefaultTsValue(tsValue, analysedType),
          (error) => new Error(`Internal Error: ${error}`),
        ),
      };

      return Either.right(value);
    }
  }

  return Either.left(unionTypeMatchError(nameOptionTypePairs, tsValue));
}

function serializeTaggedUnionTsValue(
  tsValue: unknown,
  nameOptionTypePairs: NameOptionTypePair[],
): Either.Either<Value, string> {
  if (typeof tsValue !== 'object' || tsValue === null) {
    return Either.left(typeMismatchInSerialize(tsValue, 'object with tag property'));
  }
  const obj = tsValue as Record<string, unknown>;
  const keys = Object.keys(obj);

  if (!keys.includes('tag')) {
    return Either.left(missingObjectKey('tag', obj));
  }

  for (const nameOptionTypePair of nameOptionTypePairs) {
    const typeName = nameOptionTypePair.name;

    const typeOption = nameOptionTypePair.typ;

    if (obj['tag'] === typeName) {
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

      const innerValue = serializeDefaultTsValue(obj[valueKey], typeOption);

      return Either.map(innerValue, (result) => ({
        kind: 'variant',
        caseIdx: nameOptionTypePairs.findIndex((v) => v.name === typeName),
        caseValue: result,
      }));
    }
  }

  return Either.left(unionTypeMatchError(nameOptionTypePairs, tsValue));
}

function serializeTupleTsValue(
  tsValue: unknown,
  tupleElemTypes: AnalysedType[],
): Either.Either<Value, string> {
  if (!Array.isArray(tsValue)) {
    return Either.left(
      typeMismatchInSerialize(tsValue, `Array of length ${tupleElemTypes.length}`),
    );
  }

  return Either.map(
    Either.all(tsValue.map((item, idx) => serializeDefaultTsValue(item, tupleElemTypes[idx]))),
    (values) => ({
      kind: 'tuple',
      value: values,
    }),
  );
}

export function matchesType(value: unknown, type: AnalysedType): boolean {
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
      return value === undefined || value === null || matchesType(value, type.value.inner);

    case 'list':
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

    case 'tuple':
      return matchesTuple(value, type.value.items);

    case 'result':
      if (typeof value !== 'object' || value === null) return false;
      const resultObj = value as Record<string, unknown>;

      if ('ok' in resultObj) {
        if (resultObj['ok'] === undefined || resultObj['ok'] === null) {
          return type.value.ok === undefined;
        }
        if (!type.value.ok) return false;
        return matchesType(resultObj['ok'], type.value.ok);
      } else if ('err' in resultObj) {
        if (resultObj['err'] === undefined || resultObj['err'] === null) {
          return type.value.err === undefined;
        }
        if (!type.value.err) return false;
        return matchesType(resultObj['err'], type.value.err);
      } else {
        return false;
      }

    case 'enum':
      return typeof value === 'string' && type.value.cases.includes(value.toString());

    // A variant can be tagged union or simple union
    case 'variant':
      const nameAndOptions = type.value.cases;

      // There are two cases, if they are tagged types, or not
      if (typeof value === 'object' && value !== null) {
        const obj = value as Record<string, unknown>;
        const keys = Object.keys(obj);

        if (keys.includes('tag')) {
          const tagValue = obj['tag'];

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

            return matchesType(obj[valueKey], type);
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

        const result = matchesType(value, type);

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

function matchesTuple(value: unknown, tupleTypes: readonly AnalysedType[] | undefined): boolean {
  if (!Array.isArray(value)) return false;
  if (!tupleTypes) return false;
  if (value.length !== tupleTypes.length) return false;

  return value.every((v, idx) => matchesType(v, tupleTypes[idx]));
}

function matchesArray(
  value: unknown,
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

function handleObjectMatch(value: unknown, props: NameTypePair[]): boolean {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const obj = value as Record<string, unknown>;

  const valueKeys = Object.keys(obj);
  if (valueKeys.length !== props.length) return false;

  for (const prop of props) {
    const propName = prop.name;
    const propType = prop.typ; // analysed type record has to keep track of whether it's question mark or not
    const hasKey = Object.prototype.hasOwnProperty.call(obj, propName);

    let isOptional = propType.kind === 'option';

    if (!hasKey) {
      if (!isOptional) return false;
    } else {
      if (!matchesType(obj[propName], propType)) return false;
    }
  }

  return true;
}
