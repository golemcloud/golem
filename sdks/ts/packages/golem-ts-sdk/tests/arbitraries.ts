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

import fc, { Arbitrary } from 'fast-check';

import {
  ListComplexType,
  ListType,
  MapType,
  ObjectComplexType,
  ObjectType,
  ObjectWithUnionWithUndefined1,
  ObjectWithUnionWithUndefined2,
  ObjectWithUnionWithUndefined3,
  ObjectWithUnionWithUndefined4,
  TestInterfaceType,
  TupleComplexType,
  TupleType,
  UnionComplexType,
  UnionWithLiterals,
  UnionType,
  TaggedUnion,
  UnionWithOnlyLiterals,
  ResultTypeExactBoth,
  ResultTypeNonExact,
  ResultTypeNonExact2,
} from './testTypes';

import { AgentClassName, UnstructuredBinary, UnstructuredText } from '../src';

export const uint8ArrayArb = fc.uint8Array({ minLength: 0, maxLength: 100 });

export const uint16ArrayArb = fc.uint16Array({ minLength: 0, maxLength: 100 });

export const uint32ArrayArb = fc.uint32Array({ minLength: 0, maxLength: 100 });

export const uint64ArrayArb = fc.bigUint64Array({
  minLength: 0,
  maxLength: 100,
});

export const int8ArrayArb = fc.int8Array({ minLength: 0, maxLength: 100 });

export const int16ArrayArb = fc.int16Array({ minLength: 0, maxLength: 100 });

export const int32ArrayArb = fc.int32Array({ minLength: 0, maxLength: 100 });

export const int64ArrayArb = fc.bigInt64Array({ minLength: 0, maxLength: 100 });

export const float32ArrayArb = fc.float32Array({
  minLength: 0,
  maxLength: 100,
});

export const float64ArrayArb = fc.float64Array({
  minLength: 0,
  maxLength: 100,
});

export const stringOrNumberOrNull = fc.oneof(
  fc.string(),
  fc.oneof(fc.integer(), fc.float()),
  fc.constant(null),
);

export const stringOrUndefined = fc.oneof(fc.string(), fc.constant(undefined));

export const stringOrNumberOrUndefined = fc.oneof(
  fc.string(),
  fc.oneof(fc.integer(), fc.float()),
  fc.constant(undefined),
);

export const objectWithUnionWithUndefined1Arb: Arbitrary<ObjectWithUnionWithUndefined1> = fc.record(
  {
    a: stringOrUndefined,
  },
);

export const objectWithUnionWithUndefined2Arb: Arbitrary<ObjectWithUnionWithUndefined2> = fc.record(
  {
    a: stringOrNumberOrUndefined,
  },
);

export const objectWithUnionWithUndefined3Arb: Arbitrary<ObjectWithUnionWithUndefined3> = fc.oneof(
  fc.record({}),
  fc.record({ a: stringOrNumberOrUndefined }),
);

export const objectWithUnionWithUndefined4Arb: Arbitrary<ObjectWithUnionWithUndefined4> = fc.oneof(
  fc.record({}),
  fc.record({ a: stringOrUndefined }),
);

export const unionWithLiteralArb: Arbitrary<UnionWithLiterals> = fc.oneof(
  fc.constantFrom<UnionWithLiterals>('a', 'b', 'c'),
  fc.record({ n: fc.integer() }),
);

export const resultTypeExactArb: Arbitrary<ResultTypeExactBoth> = fc.oneof(
  fc.record({ tag: fc.constant('ok'), val: fc.integer() }),
  fc.record({ tag: fc.constant('err'), val: fc.string() }),
);

export const resultTypeNonExactArb: Arbitrary<ResultTypeNonExact> = fc.oneof(
  fc.record({ tag: fc.constant('ok'), value: fc.integer() }),
  fc.record({ tag: fc.constant('err'), value: fc.string() }),
);

export const resultTypeNonExact2Arb: Arbitrary<ResultTypeNonExact2> = fc.oneof(
  fc.record({ tag: fc.constant('ok'), okValue: fc.integer() }),
  fc.record({ tag: fc.constant('err'), errValue: fc.string() }),
);

export const unstructuredTextArb: Arbitrary<UnstructuredText> = fc.oneof(
  fc.record({ tag: fc.constant('url'), val: fc.string() }),
  fc.record({ tag: fc.constant('inline'), val: fc.string() }),
);

export const unstructuredTextWithLCArb: Arbitrary<UnstructuredText<['en', 'de']>> = fc.oneof(
  fc.record({ tag: fc.constant('url'), val: fc.string() }),
  fc.record({
    tag: fc.constant('inline'),
    val: fc.string(),
    languageCode: fc.oneof(fc.constant('en'), fc.constant('de')),
  }),
);

export const unstructuredBinaryWithMimeTypeArb: Arbitrary<
  UnstructuredBinary<['application/json']>
> = fc.oneof(
  fc.record({ tag: fc.constant('url'), val: fc.string() }),
  fc.record({
    tag: fc.constant('inline'),
    val: fc.uint8Array({ minLength: 0, maxLength: 100 }),
    mimeType: fc.constant('application/json'),
  }),
);

export const taggedUnionArb: Arbitrary<TaggedUnion> = fc.oneof(
  fc.record({ tag: fc.constant('a'), val: fc.string() }),
  fc.record({ tag: fc.constant('b'), val: fc.integer() }),
  fc.record({ tag: fc.constant('c'), val: fc.boolean() }),
  fc.record({
    tag: fc.constant('d'),
    val: fc.oneof(
      fc.integer(),
      fc.string(),
      fc.boolean(),
      fc.record({
        a: fc.string(),
        b: fc.integer(),
        c: fc.boolean(),
      }),
    ),
  }),
  fc.record({
    tag: fc.constant('e'),
    val: fc.record({
      a: fc.string(),
      b: fc.integer(),
      c: fc.boolean(),
    }),
  }),
  fc.record({
    tag: fc.constant('f'),
    val: fc.array(fc.string()),
  }),
  fc.record({
    tag: fc.constant('g'),
    val: fc.tuple(fc.string(), fc.integer(), fc.boolean()),
  }),
  fc.record({
    tag: fc.constant('i'),
  }),
  // fc.record({
  //   tag: fc.constant('j'),
  // }),
);

export const unionWithOnlyLiteralsArb: Arbitrary<UnionWithOnlyLiterals> =
  fc.constantFrom<UnionWithOnlyLiterals>('foo', 'bar', 'baz');

const base = 'AssistantAgent';

const specialChars = ['$', '_', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9'];

//Ts class names can have $ and _ and digits
export const agentClassNameArb: fc.Arbitrary<AgentClassName> = fc
  .array(fc.constantFrom(...specialChars), {
    maxLength: 5,
  })
  .map((extraChars) => {
    const chars = base.split('');
    extraChars.forEach((c) => {
      const index = Math.floor(Math.random() * (chars.length + 1));
      chars.splice(index, 0, c);
    });
    return new AgentClassName(chars.join(''));
  });

export const mapArb: fc.Arbitrary<MapType> = fc
  .dictionary(fc.string(), fc.oneof(fc.integer(), fc.float()))
  .map((obj) => new Map(Object.entries(obj)));

export const objectArb: fc.Arbitrary<ObjectType> = fc.record({
  a: fc.string(),
  b: fc.oneof(fc.integer(), fc.float()),
  c: fc.boolean(),
});

export const listArb: fc.Arbitrary<ListType> = fc.array(fc.string());

export const listComplexArb: fc.Arbitrary<ListComplexType> = fc.array(
  fc.record({
    a: fc.string(),
    b: fc.oneof(fc.integer(), fc.float()),
    c: fc.boolean(),
  }),
);

export const unionArb: fc.Arbitrary<UnionType> = fc.oneof(
  fc.oneof(fc.integer(), fc.float()),
  fc.string(),
  fc.boolean(),
  fc.record({
    a: fc.string(),
    b: fc.oneof(fc.integer(), fc.float()),
    c: fc.boolean(),
  }),
);

export const tupleArb: fc.Arbitrary<TupleType> = fc.tuple(
  fc.string(),
  fc.oneof(fc.integer(), fc.float()),
  fc.boolean(),
);

export const tupleComplexArb: fc.Arbitrary<TupleComplexType> = fc.tuple(
  fc.string(),
  fc.oneof(fc.integer(), fc.float()),
  fc.record({
    a: fc.string(),
    b: fc.oneof(fc.integer(), fc.float()),
    c: fc.boolean(),
  }),
);

export const objectComplexArb: fc.Arbitrary<ObjectComplexType> = fc.record({
  a: fc.string(),
  b: fc.oneof(fc.integer(), fc.float()),
  c: fc.boolean(),
  d: objectArb,
  e: fc.oneof(fc.oneof(fc.integer(), fc.float()), fc.string(), fc.boolean(), objectArb),
  f: listArb,
  g: listComplexArb,
  h: tupleArb,
  i: tupleComplexArb,
  j: mapArb,
  k: fc.record({
    n: fc.oneof(fc.integer(), fc.float()),
  }),
});

export const unionComplexArb: fc.Arbitrary<UnionComplexType> = fc.oneof(
  fc.oneof(fc.integer(), fc.float()),
  fc.string(),
  fc.boolean(),
  objectComplexArb,
  unionArb,
  listArb,
  listComplexArb,
  tupleArb,
  tupleComplexArb,
  mapArb,
  fc.record({
    n: fc.oneof(fc.integer(), fc.float()),
  }),
);

export const baseArb = fc.record({
  bigintProp: fc.bigInt(),
  booleanProp: fc.boolean(),
  falseProp: fc.constant(false),
  listObjectProp: listComplexArb,
  listProp: listArb,
  mapProp: mapArb,
  nestedProp: fc.record({
    n: fc.oneof(fc.integer(), fc.float()),
  }),
  numberProp: fc.oneof(fc.integer(), fc.float()),
  objectProp: objectArb,
  objectComplexProp: objectComplexArb,
  stringProp: fc.string(),
  trueProp: fc.constant(true),
  tupleObjectProp: tupleComplexArb,
  tupleProp: tupleArb,
  unionProp: unionArb,
  unionComplexProp: unionComplexArb,
  uint8ArrayProp: fc.uint8Array({
    minLength: 0,
    maxLength: 10,
  }),
  uint16ArrayProp: fc.uint16Array({
    minLength: 0,
    maxLength: 10,
  }),
  uint32ArrayProp: fc.uint32Array({
    minLength: 0,
    maxLength: 10,
  }),
  uint64ArrayProp: fc.bigUint64Array({
    minLength: 0,
    maxLength: 10,
  }),
  int8ArrayProp: fc.int8Array({
    minLength: 0,
    maxLength: 10,
  }),
  int16ArrayProp: fc.int16Array({
    minLength: 0,
    maxLength: 10,
  }),
  int32ArrayProp: fc.int32Array({
    minLength: 0,
    maxLength: 10,
  }),
  int64ArrayProp: fc.bigInt64Array({
    minLength: 0,
    maxLength: 10,
  }),
  float32ArrayProp: fc.float32Array({
    minLength: 0,
    maxLength: 10,
  }),
  float64ArrayProp: fc.float64Array({
    minLength: 0,
    maxLength: 10,
  }),
  objectPropInlined: fc.record({
    a: fc.string(),
    b: fc.oneof(fc.integer(), fc.float()),
    c: fc.boolean(),
  }),
  unionPropInlined: fc.oneof(fc.string(), fc.oneof(fc.integer(), fc.float())),
});

const optionalPropArb = fc
  .option(fc.oneof(fc.integer(), fc.float()))
  .map((opt) => (opt === undefined || opt === null ? {} : { optionalProp: opt }));

export const interfaceArb: fc.Arbitrary<TestInterfaceType> = fc.tuple(baseArb, optionalPropArb).map(
  ([base, optional]) =>
    ({
      ...base,
      ...optional,
    }) as TestInterfaceType,
);
