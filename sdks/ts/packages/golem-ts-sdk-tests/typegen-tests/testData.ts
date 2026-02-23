import { Principal as AliasedSdkPrincipal } from "@golemcloud/golem-ts-sdk";

interface SimpleInterfaceType {
  n: number;
}

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

export type MultimodalType<T> = T[];

export type UnionComplexType =
  | number
  | string
  | boolean
  | ObjectComplexType
  | UnionType
  | TupleType
  | TupleComplexType
  | SimpleInterfaceType;

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

export class FooBar {
  constructor(
    public name: string,
    public value: number,
  ) {
    this.name = name;
    this.value = value;
  }
}

export type EitherX = {
  ok?: string;
  err?: string;
};

export type EitherY =
  | {
      tag: 'ok';
      val: string;
    }
  | {
      tag: 'err';
      val: string;
    };

export type EitherZ =
  | {
      tag: 'ok';
      val: string;
    }
  | {
      tag: 'err';
      val?: string;
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
}

export type RecordType = Record<string, number>;

export type MyCode = string;

export type ObjectWithTypeParameter<C extends MyCode[] = []> = { a: C };
export type UnionWithTypeParameter<C extends MyCode[] = []> = { a: C } | { b: C };

type RecursiveType = {
  more: RecursiveType | undefined;
};

class MyAgent {
  constructor(readonly testInterfaceType: TestInterfaceType) {
    this.testInterfaceType = testInterfaceType;
  }

  async getWeather(
    complexType: ObjectComplexType,
    unionType: UnionType,
    unionComplexType: UnionComplexType,
    numberType: NumberType,
    stringType: StringType,
    booleanType: BooleanType,
    mapType: MapType,
    tupleComplexType: TupleComplexType,
    tupleType: TupleType,
    listComplexType: ListComplexType,
    objectType: ObjectType,
    unionWithLiteral: 'foo' | 'bar' | 1 | true | false,
    objectWithLiteral: { tag: 'inline'; val: string },
    classType: FooBar,
    recordType: Record<string, number>,
    recordTypeAliased: RecordType,
    voidType: void,
    undefinedType: undefined,
    nullType: null,
    eitherXType: EitherX,
    eitherYType: EitherY,
    eitherZType: EitherZ,
    literallyObject: Object,
    recursiveType: RecursiveType,
    objectWithTypeParameter: ObjectWithTypeParameter<['en', 'de']>,
    unionWithTypeParameter: UnionWithTypeParameter<['en', 'de']>,
    multimodal: MultimodalType<string | boolean>,
  ): PromiseType {
    return Promise.resolve(`Weather for ${location} is sunny!`);
  }

  async methodWithOptionalQMark(required: string, optional?: number): Promise<string> {
    return Promise.resolve('test');
  }

  async methodWithOptionalUnion(required: string, optional: number | undefined): Promise<string> {
    return Promise.resolve('test');
  }

  // type-gen does not track private functions. This can be made configurable though
  private async getWeather2(object: Object): PromiseType {
    return Promise.resolve(`Weather in is sunny!`);
  }
}

class Principal { }

class PrincipalAgent {
  constructor(readonly principal: AliasedSdkPrincipal, readonly otherPrincipal: Principal) {
  }
}
