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

  it('should reject RegExp parameters and suggest using string', () => {
    const fun1Params = invalidAgent?.methods.get('fun1')?.methodParams;

    const dateType =
      AnalysedType.fromTsType(fun1Params?.get('date')!);

    const regExpType =
      AnalysedType.fromTsType(fun1Params?.get('regExp')!);


    const iteratorType =
      AnalysedType.fromTsType(fun1Params?.get('iterator')!);

    const iterableType =
      AnalysedType.fromTsType(fun1Params?.get('iterable')!);

    const asyncIteratorType =
      AnalysedType.fromTsType(fun1Params?.get('asyncIterator')!);

    const asyncIterableType =
      AnalysedType.fromTsType(fun1Params?.get('asyncIterable')!);

    const anyType =
      AnalysedType.fromTsType(fun1Params?.get('any')!);


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

  });
});
