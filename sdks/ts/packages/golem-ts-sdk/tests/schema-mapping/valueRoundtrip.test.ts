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

// Slice 2: TypeScript value <-> SchemaValue round-trip coverage through the new
// `serialize` / `deserialize` codec, mirroring the legacy `value.mapping.test.ts`
// (which exercised the same agent type metadata through the WitValue codec).

import { describe, it } from 'vitest';
import * as fc from 'fast-check';
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
  fetchTypeFromBarAgent,
  pickField,
} from '../testUtils';
import { TestInterfaceType } from '../testTypes';
import {
  interfaceArb,
  mapArb,
  objectArb,
  listComplexArb,
  tupleComplexArb,
  tupleArb,
  unionComplexArb,
  unionWithOnlyLiteralsArb,
  unionWithLiteralArb,
  resultTypeExactArb,
  resultTypeNonExactArb,
  resultTypeNonExact2Arb,
  taggedUnionArb,
} from '../arbitraries';
import { roundtripPair, roundtripValue } from './helpers';

describe('Slice 2 value round-trip (serialize <-> deserialize)', () => {
  it('interface type', () => {
    const type = getTestInterfaceType();
    fc.assert(
      fc.property(interfaceArb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('promise (unwrapped) type', () => {
    const type = getPromiseType();
    fc.assert(
      fc.property(fc.string(), (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('object type', () => {
    const type = getTestObjectType();
    fc.assert(
      fc.property(objectArb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('map type', () => {
    const type = getTestMapType();
    fc.assert(
      fc.property(mapArb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('list of object type', () => {
    const type = getTestListOfObjectType();
    fc.assert(
      fc.property(listComplexArb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('tuple types', () => {
    const simpleType = getTupleType();
    const complexType = getTupleComplexType();
    fc.assert(
      fc.property(tupleArb, tupleComplexArb, (tupleData, tupleComplexData) => {
        roundtripPair(tupleData, simpleType);
        roundtripPair(tupleComplexData, complexType);
      }),
    );
  });

  it('complex union type', () => {
    const complexType = getUnionComplexType();
    fc.assert(
      fc.property(unionComplexArb, (data) => {
        roundtripPair(data, complexType);
      }),
    );
  });

  it('union of only literals (enum)', () => {
    const type = getUnionWithOnlyLiterals();
    fc.assert(
      fc.property(unionWithOnlyLiteralsArb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('union that contains literals', () => {
    const type = getUnionWithLiterals();
    fc.assert(
      fc.property(unionWithLiteralArb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('tagged union type', () => {
    const type = fetchTypeFromBarAgent('TaggedUnion');
    fc.assert(
      fc.property(taggedUnionArb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('inbuilt result (exact tags)', () => {
    const type = getResultTypeExact();
    fc.assert(
      fc.property(resultTypeExactArb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('custom result with single value name', () => {
    const type = fetchTypeFromBarAgent('ResultTypeNonExact');
    fc.assert(
      fc.property(resultTypeNonExactArb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('custom result with distinct ok/err value names', () => {
    const type = fetchTypeFromBarAgent('ResultTypeNonExact2');
    fc.assert(
      fc.property(resultTypeNonExact2Arb, (data) => {
        roundtripPair(data, type);
      }),
    );
  });

  it('preserves values with only required properties (excluding optional)', () => {
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
        d: { a: '', b: 0, c: false },
        e: '',
        f: [],
        g: [],
        h: ['', 0, false],
        i: ['', 0, { a: '', b: 0, c: false }],
        j: new Map<string, number>(),
        k: { n: 0 },
      },
      unionComplexProp: 1,
      numberProp: 0,
      objectProp: { a: '', b: 0, c: false },
      stringProp: '',
      trueProp: true,
      tupleObjectProp: ['', 0, { a: '', b: 0, c: false }],
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
      objectPropInlined: { a: '', b: 0, c: false },
      unionPropInlined: 'foo',
    };

    roundtripPair(defaultData, getTestInterfaceType());
  });

  it('preserves values including optional properties', () => {
    const withOptionalValues: TestInterfaceType = {
      bigintProp: 5n,
      booleanProp: true,
      falseProp: false,
      listObjectProp: [],
      listProp: ['a', 'b'],
      mapProp: new Map<string, number>([['k', 1]]),
      nestedProp: { n: 3 },
      numberProp: 7,
      objectProp: { a: 'x', b: 1, c: true },
      stringProp: 'hello',
      trueProp: true,
      tupleObjectProp: ['t', 9, { a: 'q', b: 2, c: false }],
      tupleProp: ['z', 4, true],
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
        d: { a: '', b: 0, c: false },
        e: '',
        f: [],
        g: [],
        h: ['', 0, false],
        i: ['', 0, { a: '', b: 0, c: false }],
        j: new Map<string, number>(),
        k: { n: 0 },
      },
      objectPropInlined: { a: '', b: 0, c: false },
      unionPropInlined: 'foo',
    };

    roundtripPair(withOptionalValues, getTestInterfaceType());
  });

  it('preserves union property with complex object variant', () => {
    const withComplexUnion: TestInterfaceType = {
      bigintProp: 0n,
      booleanProp: false,
      falseProp: false,
      listObjectProp: [],
      listProp: [],
      mapProp: new Map<string, number>(),
      nestedProp: { n: 0 },
      numberProp: 0,
      objectProp: { a: '', b: 0, c: false },
      stringProp: '',
      trueProp: true,
      tupleObjectProp: ['', 0, { a: '', b: 0, c: false }],
      tupleProp: ['', 0, false],
      unionProp: { a: 'test', b: 42, c: true },
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
        d: { a: '', b: 0, c: false },
        e: '',
        f: [],
        g: [],
        h: ['', 0, false],
        i: ['', 0, { a: '', b: 0, c: false }],
        j: new Map<string, number>(),
        k: { n: 0 },
      },
      objectPropInlined: { a: '', b: 0, c: false },
      unionPropInlined: 'foo',
    };

    roundtripPair(withComplexUnion, getTestInterfaceType());
  });
});

describe('Slice 2 value codec edge cases', () => {
  it('64-bit integers round-trip as bigint (full u64 range)', () => {
    const bigintType = pickField(getTestInterfaceType()[0], 'bigintProp');
    roundtripValue(0n, bigintType);
    roundtripValue(18446744073709551615n, bigintType);
  });

  it('typed arrays round-trip preserving their element kind', () => {
    const type = getTestInterfaceType();
    roundtripValue(new Uint8Array([0, 127, 255]), pickField(type[0], 'uint8ArrayProp'));
    roundtripValue(new Int16Array([-5, 0, 5]), pickField(type[0], 'int16ArrayProp'));
    roundtripValue(new BigInt64Array([-1n, 0n, 1n]), pickField(type[0], 'int64ArrayProp'));
    roundtripValue(new Float64Array([1.5, -2.25]), pickField(type[0], 'float64ArrayProp'));
  });
});
