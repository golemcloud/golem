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


export type UnionWithLiterals = 'lit1' | 'lit2' | 'lit3' | boolean;

export type UnionWithOnlyLiterals = "foo" | "bar" | "baz";

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

export interface InterfaceType {
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
}

export type ResultLikeWithNoTag = {
    ok?: string;
    err?: string;
};

export type ResultLike = { tag: 'okay'; value: string; } | { tag: 'error'; value?: string; };

export type ResultLikeWithVoid  = { tag: 'ok', okVal: void } | { tag: 'err', errVal: void };

// Result exact doesn't work.
export type ResultExact = { tag: 'ok'; value: string; } | { tag: 'err'; value: string; };
