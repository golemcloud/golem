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

interface SimpleInterfaceType {
  n: number;
}

export type UnionOfLiterals = 'a' | 'b' | 'c' | true | false | null;

export type PromiseType = Promise<string>;

export type ObjectType = { a: string; b: number; c: boolean };

export type UnionType = number | string | boolean | ObjectType;

export type ListType = Array<string>;

export type ListComplexType = Array<ObjectType>;

export type TupleType = [string, number, boolean];

export type TupleComplexType = [string, number, ObjectType];

export type MapType = Map<string, number>;

export type BooleanType = boolean;

export type StringType = string;

export type NumberType = number;

export type UnionComplexType =
  | number
  | string
  | boolean
  | ObjectComplexType
  | UnionType
  | TupleType
  | TupleComplexType
  | SimpleInterfaceType
  | MapType
  | ListType
  | ListComplexType;

export type ObjectComplexType = {
  a: string;
  b: number;
  c: boolean;
  d: ObjectType;
  e: UnionType;
  f: ListType;
  g: ListComplexType;
  h: TupleType;
  i: TupleComplexType;
  j: MapType;
  k: SimpleInterfaceType;
};

export interface TestInterfaceType {
  numberProp: number;
  stringProp: string;
  booleanProp: boolean;
  bigintProp: bigint;
  nullProp: null;
  trueProp: true;
  falseProp: false;
  optionalProp?: number;
  nestedProp: SimpleInterfaceType;
  unionProp: UnionType;
  unionComplexProp: UnionComplexType;
  objectProp: ObjectType;
  objectComplexProp: ObjectComplexType;
  listProp: ListType;
  listObjectProp: ListComplexType;
  tupleProp: TupleType;
  tupleObjectProp: TupleComplexType;
  mapProp: MapType;
  uint8ArrayProp: Uint8Array;
  uint16ArrayProp: Uint16Array;
  uint32ArrayProp: Uint32Array;
  uint64ArrayProp: BigUint64Array;
  int8ArrayProp: Int8Array;
  int16ArrayProp: Int16Array;
  int32ArrayProp: Int32Array;
  int64ArrayProp: BigInt64Array;
  float32ArrayProp: Float32Array;
  float64ArrayProp: Float64Array;
  objectPropInlined: {
    a: string;
    b: number;
    c: boolean;
  };
  unionPropInlined: string | number;
  // recordProp: RecordType;
  // enumType: EnumTypeAlias;
  // enumTypeInlined: EnumType,

  // enumProp: EnumTypeAlias,
  // enumPropInlined: EnumTypeAlias,
}

export class FooBar {
  constructor(
    public name: string,
    public value: number,
  ) {
    this.name = name;
    this.value = value;
  }
}
