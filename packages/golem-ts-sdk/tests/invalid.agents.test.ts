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

// Interface type indirectly tests primitive types, union, list etc

import { describe, expect } from 'vitest';
import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import * as AnalysedType from '../src/internal/mapping/types/AnalysedType';

describe('Invalid types in agents', () => {
  const invalidAgent = TypeMetadata.getAll().get('InvalidAgent');

  it('invalid types in method inputs will return error', () => {
    const fun1Params = invalidAgent?.methods.get('fun1')?.methodParams;

    const dateType = AnalysedType.fromTsType(fun1Params?.get('date')!);

    const regExpType = AnalysedType.fromTsType(fun1Params?.get('regExp')!);

    const iteratorType = AnalysedType.fromTsType(fun1Params?.get('iterator')!);

    const iterableType = AnalysedType.fromTsType(fun1Params?.get('iterable')!);

    const asyncIteratorType = AnalysedType.fromTsType(
      fun1Params?.get('asyncIterator')!,
    );

    const asyncIterableType = AnalysedType.fromTsType(
      fun1Params?.get('asyncIterable')!,
    );

    const anyType = AnalysedType.fromTsType(fun1Params?.get('any')!);

    const stringType = AnalysedType.fromTsType(fun1Params?.get('string')!);

    const booleanType = AnalysedType.fromTsType(fun1Params?.get('boolean')!);

    const symbolType = AnalysedType.fromTsType(fun1Params?.get('symbol')!);

    const numberType = AnalysedType.fromTsType(fun1Params?.get('number')!);

    const bigintType = AnalysedType.fromTsType(fun1Params?.get('bigint')!);

    const nullType = AnalysedType.fromTsType(fun1Params?.get('nullParam')!);

    const undefinedType = AnalysedType.fromTsType(
      fun1Params?.get('undefined')!,
    );

    const voidType = AnalysedType.fromTsType(fun1Params?.get('voidParam')!);

    const unionWithNullType = AnalysedType.fromTsType(
      fun1Params?.get('unionWithNull')!,
    );

    const objectWithInvalidUnion1 = AnalysedType.fromTsType(
      fun1Params?.get('objectWithUndefinedUnion1')!,
    );

    const objectWithInvalidUnion2 = AnalysedType.fromTsType(
      fun1Params?.get('objectWithUndefinedUnion2')!,
    );

    expect(dateType.val).toBe(
      'Unsupported type `Date`. Use a `string` if possible',
    );

    expect(regExpType.val).toBe(
      'Unsupported type `RegExp`. Use a `string` if possible',
    );

    expect(iteratorType.val).toBe(
      'Unsupported type `Iterator`. Use `Array` type instead',
    );

    expect(iterableType.val).toBe(
      'Unsupported type `Iterable`. Use `Array` type instead',
    );

    expect(asyncIteratorType.val).toBe(
      'Unsupported type `Iterator`. Use `Array` type instead',
    );

    expect(asyncIterableType.val).toBe(
      'Unsupported type `AsyncIterator`. Use `Array` type instead',
    );

    expect(anyType.val).toBe(
      'Unsupported type `any`. Use a specific type instead',
    );

    expect(stringType.val).toBe(
      'Unsupported type `String`, use `string` instead',
    );

    expect(booleanType.val).toBe(
      'Unsupported type `Boolean`, use `boolean` instead',
    );

    expect(numberType.val).toBe(
      'Unsupported type `Number`, use `number` instead',
    );

    expect(symbolType.val).toBe(
      'Unsupported type `Symbol`, use `string` if possible',
    );

    expect(bigintType.val).toBe(
      'Unsupported type `BigInt`, use `bigint` instead',
    );

    expect(nullType.val).toBe('Unsupported type `null`');

    expect(undefinedType.val).toBe('Unsupported type `undefined`');

    expect(voidType.val).toBe('Unsupported type `void`');

    expect(unionWithNullType.val).toBe('Unsupported type `null`');

    expect(objectWithInvalidUnion1.val).toBe(
      'Parameter `a` has a union type with `undefined` as one of the variants. This is not supported. Consider changing `a:` to  `a?:` and remove undefined',
    );

    expect(objectWithInvalidUnion2.val).toBe(
      'Parameter `a` has a union type with `undefined` as one of the variants. This is not supported. Consider changing `a:` to  `a?:` and remove undefined',
    );
  });

  // Act as more of a regression test
  it('invalid types in method outputs will return error', () => {
    const fun2ReturnType = invalidAgent?.methods.get('fun2')?.returnType;
    const fun3ReturnType = invalidAgent?.methods.get('fun3')?.returnType;
    const fun4ReturnType = invalidAgent?.methods.get('fun4')?.returnType;
    const fun5ReturnType = invalidAgent?.methods.get('fun5')?.returnType;
    const fun6ReturnType = invalidAgent?.methods.get('fun6')?.returnType;
    const fun7ReturnType = invalidAgent?.methods.get('fun7')?.returnType;
    const fun8ReturnType = invalidAgent?.methods.get('fun8')?.returnType;
    const fun9ReturnType = invalidAgent?.methods.get('fun9')?.returnType;
    const fun10ReturnType = invalidAgent?.methods.get('fun10')?.returnType;
    const fun11ReturnType = invalidAgent?.methods.get('fun11')?.returnType;
    const fun12ReturnType = invalidAgent?.methods.get('fun12')?.returnType;

    const fun2Type = AnalysedType.fromTsType(fun2ReturnType!);
    const fun3Type = AnalysedType.fromTsType(fun3ReturnType!);
    const fun4Type = AnalysedType.fromTsType(fun4ReturnType!);
    const fun5Type = AnalysedType.fromTsType(fun5ReturnType!);
    const fun6Type = AnalysedType.fromTsType(fun6ReturnType!);
    const fun7Type = AnalysedType.fromTsType(fun7ReturnType!);
    const fun8Type = AnalysedType.fromTsType(fun8ReturnType!);
    const fun9Type = AnalysedType.fromTsType(fun9ReturnType!);
    const fun10Type = AnalysedType.fromTsType(fun10ReturnType!);
    const fun11Type = AnalysedType.fromTsType(fun11ReturnType!);
    const fun12Type = AnalysedType.fromTsType(fun12ReturnType!);

    expect(fun2Type.val).toBe(
      'Unsupported type `Date`. Use a `string` if possible',
    );

    expect(fun3Type.val).toBe(
      'Unsupported type `Iterator`. Use `Array` type instead',
    );

    expect(fun4Type.val).toBe(
      'Unsupported type `Iterable`. Use `Array` type instead',
    );

    expect(fun5Type.val).toBe(
      'Unsupported type `Iterator`. Use `Array` type instead',
    );

    expect(fun6Type.val).toBe(
      'Unsupported type `AsyncIterator`. Use `Array` type instead',
    );

    expect(fun7Type.val).toBe(
      'Unsupported type `any`. Use a specific type instead',
    );

    expect(fun8Type.val).toBe(
      'Unsupported type `String`, use `string` instead',
    );

    expect(fun9Type.val).toBe(
      'Unsupported type `Number`, use `number` instead',
    );

    expect(fun10Type.val).toBe(
      'Unsupported type `Boolean`, use `boolean` instead',
    );

    expect(fun11Type.val).toBe(
      'Unsupported type `Symbol`, use `string` if possible',
    );

    expect(fun12Type.val).toBe(
      'Unsupported type `BigInt`, use `bigint` instead',
    );
  });
});
