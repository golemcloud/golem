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

export type TaggedUnion =
  | { tag: 'a'; val: string }
  | { tag: 'b'; val: number }
  | { tag: 'c'; val: boolean }
  | { tag: 'd'; val: UnionType }
  | { tag: 'e'; val: ObjectType }
  | { tag: 'f'; val: ListType }
  | { tag: 'g'; val: TupleType }
  | { tag: 'h'; val: SimpleInterfaceType }
  | { tag: 'i' }
  | { tag: 'j' };

export type UnionWithOnlyLiterals = 'foo' | 'bar' | 'baz';

export type UnionWithLiterals = 'a' | 'b' | 'c' | { n: number };

// user defined result type, with exact shape of wit result
export type ResultTypeExactBoth = { tag: 'ok'; val: number } | { tag: 'err'; val: string };

// user defined result type, with `value` as field names
export type ResultTypeNonExact = { tag: 'ok'; value: number } | { tag: 'err'; value: string };

// user defined result type, with custom field names
export type ResultTypeNonExact2 = { tag: 'ok'; okValue: number } | { tag: 'err'; errValue: string };

// User defined result type, with void types in ok and err channels
export type ResultTypeNonExact3 = { tag: 'ok'; okValue: void } | { tag: 'err'; errValue: void };

export type ResultTypeInvalid1 = { tag: 'ok'; okValOpt?: number } | { tag: 'err'; errVal: string };

export type ResultTypeInvalid2 = { tag: 'ok'; okVal: number } | { tag: 'err'; errVal?: string };

export type ResultTypeInvalid3 = { tag: 'ok'; okVal?: number } | { tag: 'err'; errVal?: string };

export type PromiseType = Promise<string>;

export type ObjectType = {
  a: string;
  b: number;
  c: boolean;
};

export type ObjectWithUnionWithUndefined1 = {
  a: string | undefined;
};

export type ObjectWithUnionWithUndefined2 = {
  a: string | number | undefined;
};

export type ObjectWithUnionWithUndefined3 = {
  a?: string | number | undefined;
};

export type ObjectWithUnionWithUndefined4 = {
  a?: string | undefined;
};

export type ObjectWithOption = {
  a?: string;
};

export interface InterfaceWithUnionWithUndefined1 {
  a: string | undefined;
}

export interface InterfaceWithUnionWithUndefined2 {
  a: string | number | undefined;
}

export interface InterfaceWithUnionWithUndefined3 {
  a?: string | number | undefined;
}

export interface InterfaceWithUnionWithUndefined4 {
  a?: string | undefined;
}

export interface InterfaceWithOption {
  a?: string;
}

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

export type EitherX = {
  ok?: string;
  err?: string;
};

export type EitherY =
  | {
      tag: 'okay';
      val: string;
    }
  | {
      tag: 'error';
      val: string;
    };

export type EitherZ =
  | {
      tag: 'okay';
      val: string;
    }
  | {
      tag: 'error';
      val?: string;
    };

export type RecursiveType = {
  more: RecursiveType | undefined;
};
