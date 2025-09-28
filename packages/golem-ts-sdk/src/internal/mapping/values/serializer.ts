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

import {
  AnalysedType,
  NameOptionTypePair,
  NameTypePair,
} from '../types/AnalysedType';
import * as Either from '../../../newTypes/either';
import * as Option from '../../../newTypes/option';
import {
  enumMismatchInSerialize,
  missingObjectKey,
  typeMismatchInSerialize,
  unhandledTypeError,
  unionTypeMatchError,
} from './errors';
import { TaggedTypeMetadata } from '../types/taggedUnion';
import { Value } from './Value';

export function serialize(
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
        return Either.map(serialize(tsValue, innerType), (v) => ({
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
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
          case 'u16':
            if (tsValue instanceof Uint16Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
          case 'u32':
            if (tsValue instanceof Uint32Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
          case 'big-u64':
            if (tsValue instanceof BigUint64Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
          case 'i8':
            if (tsValue instanceof Int8Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
          case 'i16':
            if (tsValue instanceof Int16Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
          case 'i32':
            if (tsValue instanceof Int32Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
          case 'big-i64':
            if (tsValue instanceof BigInt64Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
          case 'f32':
            if (tsValue instanceof Float32Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
          case 'f64':
            if (tsValue instanceof Float64Array) {
              return Either.map(
                Either.all(
                  Array.from(tsValue).map((item) =>
                    serialize(item, innerListType),
                  ),
                ),
                (values) => ({
                  kind: 'list',
                  value: values,
                }),
              );
            } else {
              return Either.left(
                typeMismatchInSerialize(tsValue, analysedType),
              );
            }
        }
      }

      if (Array.isArray(tsValue)) {
        return Either.map(
          Either.all(tsValue.map((item) => serialize(item, innerListType))),
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
      const taggedTypes = analysedType.taggedTypes;

      return handleVariant(tsValue, taggedTypes, variantTypes);

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

      if (typeof tsValue !== 'object' || tsValue === null) {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }

      if (!('tag' in tsValue)) {
        return Either.left(typeMismatchInSerialize(tsValue, analysedType));
      }

      const okValueName = analysedType.okValueName;
      const errValueName = analysedType.errValueName;

      if (tsValue['tag'] === 'ok') {
        if (okType) {
          if (!okValueName) {
            return Either.left(typeMismatchInSerialize(tsValue, analysedType));
          }

          return Either.map(serialize(tsValue[okValueName], okType), (v) => ({
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
      } else if (typeof tsValue === 'object' && tsValue['tag'] === 'err') {
        if (errType) {
          if (!errValueName) {
            return Either.left(typeMismatchInSerialize(tsValue, analysedType));
          }

          return Either.map(serialize(tsValue[errValueName], errType), (v) => ({
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
        serialize(key, keyAnalysedType),
        serialize(value, valueAnalysedType),
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

    const fieldVal = serialize(tsValue[key], nameTypePair.typ);

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
  taggedTypes: TaggedTypeMetadata[],
  nameOptionTypePairs: NameOptionTypePair[],
): Either.Either<Value, string> {
  if (taggedTypes.length > 0) {
    return handleTaggedTypedUnion(tsValue, nameOptionTypePairs);
  }

  for (const [idx, variant] of nameOptionTypePairs.entries()) {
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

    if (matches) {
      const value: Value = {
        kind: 'variant',
        caseIdx: idx,
        caseValue: Either.getOrThrowWith(
          serialize(tsValue, analysedType),
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
        taggedTypes: [],
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

      const innerValue = serialize(tsValue[valueKey], typeOption);

      return Either.map(innerValue, (result) => ({
        kind: 'variant',
        caseIdx: nameOptionTypePairs.findIndex((v) => v.name === typeName),
        caseValue: result,
      }));
    }
  }

  return Either.left(unionTypeMatchError(nameOptionTypePairs, tsValue));
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
        emptyType: undefined,
      }),
    );
  }

  return Either.map(
    Either.all(tsValue.map((item, idx) => serialize(item, analysedTypes[idx]))),
    (values) => ({
      kind: 'tuple',
      value: values,
    }),
  );
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
  if (typeof value !== 'object' && value !== 'interface') {
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
