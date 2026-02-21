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

import { describe, it, expect } from 'vitest';
import {
  getTestMapType,
  getTestInterfaceType,
  getTestObjectType,
  getTestListOfObjectType,
  getTupleComplexType,
  getTupleType,
  getUnionComplexType,
  getPromiseType,
  getUnionWithOnlyLiterals,
  getUnionWithLiterals,
  getResultTypeExact,
} from './testUtils';

import { TestInterfaceType } from './testTypes';
import * as Value from '../src/internal/mapping/values/Value';
import {
  interfaceArb,
  mapArb,
  objectArb,
  listComplexArb,
  tupleComplexArb,
  tupleArb,
  unionArb,
  unionComplexArb,
  unionWithOnlyLiteralsArb,
  unionWithLiteralArb,
  resultTypeExactArb,
} from './arbitraries';
import * as fc from 'fast-check';
import { Type } from '@golemcloud/golem-ts-types-core';
import * as EffectEither from '../src/newTypes/either';
import * as WitValue from '../src/internal/mapping/values/WitValue';
import { AnalysedType } from '../src/internal/mapping/types/analysedType';

describe('typescript value to wit value round-trip conversions', () => {
  it('should correctly perform round-trip conversion for arbitrary values of interface type', () => {
    fc.assert(
      fc.property(interfaceArb, (arbData) => {
        const type = getTestInterfaceType();
        runRoundTripTest(arbData, type);
      }),
    );
  });

  it('should correctly perform round-trip conversion for arbitrary values of promise type', () => {
    fc.assert(
      fc.property(fc.string(), (arbData) => {
        const type = getPromiseType();
        runRoundTripTest(arbData, type);
      }),
    );
  });

  it('should correctly perform round-trip conversion for arbitrary values of object type', () => {
    fc.assert(
      fc.property(objectArb, (arbData) => {
        const type = getTestObjectType();
        runRoundTripTest(arbData, type);
      }),
    );
  });

  it('should correctly perform round-trip conversion for arbitrary values of map type', () => {
    fc.assert(
      fc.property(mapArb, (arbData) => {
        const type = getTestMapType();
        runRoundTripTest(arbData, type);
      }),
    );
  });

  //
  it('should correctly perform round-trip conversion for arbitrary values of list of object type', () => {
    fc.assert(
      fc.property(listComplexArb, (arbData) => {
        const type = getTestListOfObjectType();
        runRoundTripTest(arbData, type);
      }),
    );
  });

  it('should correctly perform round-trip conversion for arbitrary values of complex tuple', () => {
    fc.assert(
      fc.property(tupleArb, tupleComplexArb, (tupleData, tupleComplexData) => {
        const simpleType = getTupleType();
        runRoundTripTest(tupleData, simpleType);

        const complexType = getTupleComplexType();
        runRoundTripTest(tupleComplexData, complexType);
      }),
    );
  });

  it('should correctly perform round-trip conversion for arbitrary values of union abcdefg', () => {
    fc.assert(
      fc.property(unionArb, unionComplexArb, (unionData, unionComplexData) => {
        // const simpleType = getUnionType();
        // runRoundTripTest(unionData, simpleType);

        const complexType = getUnionComplexType();
        runRoundTripTest(unionComplexData, complexType);
      }),
    );
  });

  it('should correctly perform round-trip conversion for arbitrary values of union types', () => {
    fc.assert(
      fc.property(unionWithOnlyLiteralsArb, (unionData) => {
        const unionWithOnlyLiterals = getUnionWithOnlyLiterals();
        runRoundTripTest(unionData, unionWithOnlyLiterals);
      }),
    );
  });

  it('should correctly perform round-trip conversion for arbitrary values of union that contains literals', () => {
    fc.assert(
      fc.property(unionWithLiteralArb, (unionData) => {
        const unionWithLiterals = getUnionWithLiterals();
        runRoundTripTest(unionData, unionWithLiterals);
      }),
    );
  });

  it('should correctly perform round-trip conversion for wit result', () => {
    fc.assert(
      fc.property(resultTypeExactArb, (resultValue) => {
        const resultTypeExact = getResultTypeExact();
        runRoundTripTest(resultValue, resultTypeExact);
      }),
    );
  });

  it('should preserve values with only required properties (excluding optional)', () => {
    const defaultData: TestInterfaceType = {
      bigintProp: 0n,
      booleanProp: false,
      falseProp: false,
      listObjectProp: [],
      listProp: [],
      mapProp: new Map<string, number>(),
      nestedProp: { n: 0 },
      objectComplexProp: {
        a: '',
        b: 0,
        c: false,
        d: {
          a: '',
          b: 0,
          c: false,
        },
        e: '',
        f: [],
        g: [],
        h: ['', 0, false],
        i: [
          '',
          0,
          {
            a: '',
            b: 0,
            c: false,
          },
        ],
        j: new Map<string, number>(),
        k: { n: 0 },
        // m: Either.left('failed')
      },
      unionComplexProp: 1,
      numberProp: 0,
      objectProp: {
        a: '',
        b: 0,
        c: false,
      },
      stringProp: '',
      trueProp: true,
      tupleObjectProp: [
        '',
        0,
        {
          a: '',
          b: 0,
          c: false,
        },
      ],
      tupleProp: ['', 0, false],
      unionProp: 1,
      uint8ArrayProp: new Uint8Array([1, 2, 3]),
      uint16ArrayProp: new Uint16Array([1, 2, 3]),
      uint32ArrayProp: new Uint32Array([1, 2, 3]),
      uint64ArrayProp: new BigUint64Array([1n, 2n, 3n]),
      int8ArrayProp: new Int8Array([1, 2, 3]),
      int16ArrayProp: new Int16Array([1, 2, 3]),
      int32ArrayProp: new Int32Array([1, 2, 3]),
      int64ArrayProp: new BigInt64Array([1n, 2n, 3n]),
      float32ArrayProp: new Float32Array([1.1, 2.2, 3.3]),
      float64ArrayProp: new Float64Array([1.1, 2.2, 3.3]),
      objectPropInlined: {
        a: '',
        b: 0,
        c: false,
      },
      unionPropInlined: 'foo',
    };

    const type = getTestInterfaceType();

    runRoundTripTest(defaultData, type);
  });

  it('should preserve values including optional properties', () => {
    const withOptionalValues: TestInterfaceType = {
      bigintProp: 0n,
      booleanProp: false,
      falseProp: false,
      listObjectProp: [],
      listProp: [],
      mapProp: new Map<string, number>(),
      nestedProp: { n: 0 },
      numberProp: 0,
      objectProp: {
        a: '',
        b: 0,
        c: false,
      },
      stringProp: '',
      trueProp: true,
      tupleObjectProp: [
        '',
        0,
        {
          a: '',
          b: 0,
          c: false,
        },
      ],
      tupleProp: ['', 0, false],
      unionProp: 1,
      optionalProp: 2,
      unionComplexProp: 1,
      uint8ArrayProp: new Uint8Array([1, 2, 3]),
      uint16ArrayProp: new Uint16Array([1, 2, 3]),
      uint32ArrayProp: new Uint32Array([1, 2, 3]),
      uint64ArrayProp: new BigUint64Array([1n, 2n, 3n]),
      int8ArrayProp: new Int8Array([1, 2, 3]),
      int16ArrayProp: new Int16Array([1, 2, 3]),
      int32ArrayProp: new Int32Array([1, 2, 3]),
      int64ArrayProp: new BigInt64Array([1n, 2n, 3n]),
      float32ArrayProp: new Float32Array([1.1, 2.2, 3.3]),
      float64ArrayProp: new Float64Array([1.1, 2.2, 3.3]),
      objectComplexProp: {
        a: '',
        b: 0,
        c: false,
        d: {
          a: '',
          b: 0,
          c: false,
        },
        e: '',
        f: [],
        g: [],
        h: ['', 0, false],
        i: [
          '',
          0,
          {
            a: '',
            b: 0,
            c: false,
          },
        ],
        j: new Map<string, number>(),
        k: { n: 0 },
      },
      objectPropInlined: {
        a: '',
        b: 0,
        c: false,
      },
      unionPropInlined: 'foo',
    };

    const type = getTestInterfaceType();

    runRoundTripTest(withOptionalValues, type);
  });

  it('should preserve union properties with complex object variants', () => {
    const withComplexUnionType: TestInterfaceType = {
      bigintProp: 0n,
      booleanProp: false,
      falseProp: false,
      listObjectProp: [],
      listProp: [],
      mapProp: new Map<string, number>(),
      nestedProp: { n: 0 },
      numberProp: 0,
      objectProp: {
        a: '',
        b: 0,
        c: false,
      },
      stringProp: '',
      trueProp: true,
      tupleObjectProp: [
        '',
        0,
        {
          a: '',
          b: 0,
          c: false,
        },
      ],
      tupleProp: ['', 0, false],
      unionProp: {
        a: 'test',
        b: 42,
        c: true,
      }, // Using an object as a union type
      optionalProp: 2,
      unionComplexProp: 1,
      uint8ArrayProp: new Uint8Array([1, 2, 3]),
      uint16ArrayProp: new Uint16Array([1, 2, 3]),
      uint32ArrayProp: new Uint32Array([1, 2, 3]),
      uint64ArrayProp: new BigUint64Array([1n, 2n, 3n]),
      int8ArrayProp: new Int8Array([1, 2, 3]),
      int16ArrayProp: new Int16Array([1, 2, 3]),
      int32ArrayProp: new Int32Array([1, 2, 3]),
      int64ArrayProp: new BigInt64Array([1n, 2n, 3n]),
      float32ArrayProp: new Float32Array([1.1, 2.2, 3.3]),
      float64ArrayProp: new Float64Array([1.1, 2.2, 3.3]),
      objectComplexProp: {
        a: '',
        b: 0,
        c: false,
        d: {
          a: '',
          b: 0,
          c: false,
        },
        e: '',
        f: [],
        g: [],
        h: ['', 0, false],
        i: [
          '',
          0,
          {
            a: '',
            b: 0,
            c: false,
          },
        ],
        j: new Map<string, number>(),
        k: { n: 0 },
      },
      objectPropInlined: {
        a: '',
        b: 0,
        c: false,
      },
      unionPropInlined: 'foo',
    };

    const type = getTestInterfaceType();

    runRoundTripTest(withComplexUnionType, type);
  });
});

function runRoundTripTest<T>(data: T, type: [AnalysedType, Type.Type]) {
  const witValueEither = WitValue.fromTsValueDefault(data, type[0]);

  const witValue = EffectEither.getOrElse(witValueEither, (err) => {
    throw new Error(err);
  });

  // Round trip wit-value -> value -> wit-value
  const value = Value.fromWitValue(witValue);

  const witValueReturned = Value.toWitValue(value);
  expect(witValueReturned).toEqual(witValue);

  // Round trip ts-value -> wit-value -> ts-value
  const tsValueReturned = WitValue.toTsValue(witValueReturned, type[0]);

  expect(tsValueReturned).toEqual(data);
}
