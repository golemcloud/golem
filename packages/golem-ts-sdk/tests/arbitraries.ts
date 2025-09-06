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

import fc from 'fast-check';

import {
  ListComplexType,
  ListType,
  MapType,
  ObjectComplexType,
  ObjectType,
  TestInterfaceType,
  TupleComplexType,
  TupleType,
  UnionComplexType,
  UnionType,
} from './testTypes';

import { AgentClassName } from '../src';

const base = 'AssistantAgent';

const specialChars = [
  '$',
  '_',
  '0',
  '1',
  '2',
  '3',
  '4',
  '5',
  '6',
  '7',
  '8',
  '9',
];

//Ts class names can have $ and _ and digits
export const agentClassNameArb: fc.Arbitrary<AgentClassName> = fc
  .array(fc.constantFrom(...specialChars), { maxLength: 5 })
  .map((extraChars) => {
    const chars = base.split('');
    extraChars.forEach((c) => {
      const index = Math.floor(Math.random() * (chars.length + 1));
      chars.splice(index, 0, c);
    });
    return new AgentClassName(chars.join(''));
  });

export const mapArb: fc.Arbitrary<MapType> = fc
  .dictionary(fc.string(), fc.integer())
  .map((obj) => new Map(Object.entries(obj)));

export const objectArb: fc.Arbitrary<ObjectType> = fc.record({
  a: fc.string(),
  b: fc.integer(),
  c: fc.boolean(),
});

export const listArb: fc.Arbitrary<ListType> = fc.array(fc.string());

export const listComplexArb: fc.Arbitrary<ListComplexType> = fc.array(
  fc.record({
    a: fc.string(),
    b: fc.integer(),
    c: fc.boolean(),
  }),
);

export const unionArb: fc.Arbitrary<UnionType> = fc.oneof(
  fc.integer(),
  fc.string(),
  fc.boolean(),
  fc.record({
    a: fc.string(),
    b: fc.integer(),
    c: fc.boolean(),
  }),
);

export const tupleArb: fc.Arbitrary<TupleType> = fc.tuple(
  fc.string(),
  fc.integer(),
  fc.boolean(),
);

export const tupleComplexArb: fc.Arbitrary<TupleComplexType> = fc.tuple(
  fc.string(),
  fc.integer(),
  fc.record({
    a: fc.string(),
    b: fc.integer(),
    c: fc.boolean(),
  }),
);

export const objectComplexArb: fc.Arbitrary<ObjectComplexType> = fc.record({
  a: fc.string(),
  b: fc.integer(),
  c: fc.boolean(),
  d: objectArb,
  e: fc.oneof(fc.integer(), fc.string(), fc.boolean(), objectArb),
  f: listArb,
  g: listComplexArb,
  h: tupleArb,
  i: tupleComplexArb,
  j: mapArb,
  k: fc.record({ n: fc.integer() }),
});

export const unionComplexArb: fc.Arbitrary<UnionComplexType> = fc.oneof(
  fc.integer(),
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
    n: fc.integer(),
  }),
);

export const baseArb = fc.record({
  bigintProp: fc.bigInt(),
  booleanProp: fc.boolean(),
  falseProp: fc.constant(false),
  listObjectProp: listComplexArb,
  listProp: listArb,
  mapProp: mapArb,
  nestedProp: fc.record({ n: fc.integer() }),
  nullProp: fc.constant(null),
  numberProp: fc.integer(),
  objectProp: objectArb,
  objectComplexProp: objectComplexArb,
  stringProp: fc.string(),
  trueProp: fc.constant(true),
  tupleObjectProp: tupleComplexArb,
  tupleProp: tupleArb,
  unionProp: unionArb,
  unionComplexProp: unionComplexArb,
  uint8ArrayProp: fc.uint8Array({ minLength: 0, maxLength: 10 }),
  uint16ArrayProp: fc.uint16Array({ minLength: 0, maxLength: 10 }),
  uint32ArrayProp: fc.uint32Array({ minLength: 0, maxLength: 10 }),
  uint64ArrayProp: fc.bigUint64Array({ minLength: 0, maxLength: 10 }),
  int8ArrayProp: fc.int8Array({ minLength: 0, maxLength: 10 }),
  int16ArrayProp: fc.int16Array({ minLength: 0, maxLength: 10 }),
  int32ArrayProp: fc.int32Array({ minLength: 0, maxLength: 10 }),
  int64ArrayProp: fc.bigInt64Array({ minLength: 0, maxLength: 10 }),
  float32ArrayProp: fc.float32Array({ minLength: 0, maxLength: 10 }),
  float64ArrayProp: fc.float64Array({ minLength: 0, maxLength: 10 }),
  objectPropInlined: fc.record({
    a: fc.string(),
    b: fc.integer(),
    c: fc.boolean(),
  }),
  unionPropInlined: fc.oneof(fc.string(), fc.integer()),
});

const optionalPropArb = fc
  .option(fc.integer())
  .map((opt) =>
    opt === undefined || opt === null ? {} : { optionalProp: opt },
  );

export const interfaceArb: fc.Arbitrary<TestInterfaceType> = fc
  .tuple(baseArb, optionalPropArb)
  .map(
    ([base, optional]) =>
      ({
        ...base,
        ...optional,
      }) as TestInterfaceType,
  );
