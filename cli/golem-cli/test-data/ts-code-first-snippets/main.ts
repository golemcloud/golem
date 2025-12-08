import {
    BaseAgent,
    Result,
    agent,
    UnstructuredText,
    UnstructuredBinary,
    WithRemoteMethods,
    MultimodalAdvanced
} from '@golemcloud/golem-ts-sdk';

import * as Types from './model';
import {
    ObjectWithUnionWithUndefined1,
    ObjectWithUnionWithUndefined2,
    ObjectWithUnionWithUndefined3,
    ObjectWithUnionWithUndefined4,
    UnionWithLiterals,
    UnionType,
    TaggedUnion,
    InterfaceType,
    ObjectType,
    ObjectComplexType,
    UnionComplexType,
    NumberType,
    StringType,
    BooleanType,
    MapType,
    TupleComplexType,
    TupleType,
    ListComplexType,
    ResultLikeWithNoTag,
    ResultLike,
    ResultExact,
    UnionWithOnlyLiterals,
} from './model';


export type InputText = { val: string, tag: "text" };
export type InputImage = { val: Uint8Array; tag: "image" };

@agent()
class FooAgent extends BaseAgent {
    readonly barAgent: WithRemoteMethods<BarAgent>;

    constructor(readonly input: string) {
        super();

        const interfaceType: InterfaceType = {
            bigintProp: 0n,
            booleanProp: false,
            falseProp: false,
            float32ArrayProp: new Float32Array(),
            float64ArrayProp: new Float64Array(),
            int16ArrayProp: new Int16Array(),
            int32ArrayProp: new Int32Array(),
            int64ArrayProp: new BigInt64Array(),
            int8ArrayProp: new Int8Array(),
            listObjectProp: [{a: "foo", b: 1, c: true}],
            listProp: ["foo", "bar"],
            mapProp: new Map<string, number>(),
            nestedProp: {
                n: 1
            },
            numberProp: 0,
            objectComplexProp: {
                a: "foo",
                b: 1,
                c: true,
                d: {a: "foo", b: 1, c: true},
                e: 1,
                f: ["foo"],
                g: [{a: "foo", b: 1, c: true}],
                h: ["foo", 1, false],
                i: ["foo", 1, {a: "foo", b: 1, c: true}],
                j: new Map(),
                k: {n: 1}
            },
            objectProp: {a: "foo", b: 1, c: true},
            objectPropInlined: {a: "", b: 0, c: false},
            optionalProp: 0,
            stringProp: "",
            trueProp: true,
            tupleObjectProp: ["", 0, {a: "foo", b: 1, c: true}],
            tupleProp: ["", 0, false],
            uint16ArrayProp: new Uint16Array(),
            uint32ArrayProp: new Uint32Array(),
            uint64ArrayProp: new BigUint64Array(),
            uint8ArrayProp: new Uint8Array(),
            unionComplexProp: 1,
            unionProp: true,
            unionPropInlined: "afsal"
        }

        this.barAgent = BarAgent.get("foooo", 1);
    }

    async funAll(
        complexType: Types.ObjectComplexType,
        unionType: Types.UnionType,
        unionComplexType: Types.UnionComplexType,
        numberType: Types.NumberType,
        stringType: Types.StringType,
        booleanType: Types.BooleanType,
        mapType: Types.MapType,
        tupleComplexType: Types.TupleComplexType,
        tupleType: Types.TupleType,
        listComplexType: Types.ListComplexType,
        objectType: Types.ObjectType,
        resultLike: ResultLike,
        resultLikeWithNoTag: ResultLikeWithNoTag,
        unionWithNull: string | number | null,
        objectWithUnionWithUndefined1: ObjectWithUnionWithUndefined1,
        objectWithUnionWithUndefined2: ObjectWithUnionWithUndefined2,
        objectWithUnionWithUndefined3: ObjectWithUnionWithUndefined3,
        objectWithUnionWithUndefined4: ObjectWithUnionWithUndefined4,
        optionalStringType: string | undefined,
        optionalUnionType: UnionType | undefined,
        taggedUnionType: TaggedUnion,
    ): Types.PromiseType {
        return await this.barAgent.funAll(
            complexType,
            unionType,
            unionComplexType,
            numberType,
            stringType,
            booleanType,
            mapType,
            tupleComplexType,
            tupleType,
            listComplexType,
            objectType,
            resultLike,
            resultLikeWithNoTag,
            unionWithNull,
            objectWithUnionWithUndefined1,
            objectWithUnionWithUndefined2,
            objectWithUnionWithUndefined3,
            objectWithUnionWithUndefined4,
            optionalStringType,
            optionalUnionType,
            taggedUnionType,
        )
    }

    async funOptional(param1: string | number | null,
                      param2: ObjectWithUnionWithUndefined1,
                      param3: ObjectWithUnionWithUndefined2,
                      param4: ObjectWithUnionWithUndefined3,
                      param5: ObjectWithUnionWithUndefined4,
                      param6: string | undefined,
                      param7: UnionType | undefined,) {

        return this.barAgent.funOptional(
            param1,
            param2,
            param3,
            param4,
            param5,
            param6,
            param7,)
    }

    async funOptionalQMark(param1: string, param2?: number, param3?: string) {
        return this.barAgent.funOptionalQMark(
            param1, param2, param3
        )
    }

    async funObjectComplexType(text: ObjectComplexType): Promise<ObjectComplexType> {
        return await this.barAgent.funObjectComplexType(text);
    }


    async funUnionType(unionType: UnionType): Promise<UnionType> {
        return await this.barAgent.funUnionType(unionType);
    }

    // // Doesn't work when directly called
    async funUnionComplexType(unionComplexType: UnionComplexType): Promise<Types.UnionComplexType> {
        return await this.barAgent.funUnionComplexType(unionComplexType);
    }

    async funNumber(numberType: NumberType): Promise<NumberType> {
        return await this.barAgent.funNumber(numberType);
    }


    async funString(stringType: StringType): Promise<Types.StringType> {
        return await this.barAgent.funString(stringType);
    }


    async funBoolean(booleanType: BooleanType): Promise<Types.BooleanType> {
        return await this.barAgent.funBoolean(booleanType);
    }


    async funMap(mapType: MapType): Promise<Types.MapType> {
        return await this.barAgent.funText(mapType);
    }

    async funTaggedUnion(taggedUnionType: TaggedUnion): Promise<TaggedUnion> {
        return await this.barAgent.funTaggedUnion(taggedUnionType);
    }


    async funTupleComplexType(complexType: TupleComplexType): Promise<Types.TupleComplexType> {
        return await this.barAgent.funTupleComplexType(complexType);
    }


    async funTupleType(tupleType: TupleType): Promise<Types.TupleType> {
        return await this.barAgent.funTupleType(tupleType);
    }


    async funListComplexType(listComplexType: ListComplexType): Promise<Types.ListComplexType> {
        return await this.barAgent.funListComplexType(listComplexType);
    }


    async funObjectType(objectType: ObjectType): Promise<ObjectType> {
        return await this.barAgent.funObjectType(objectType);
    }


    async funUnionWithLiterals(unionWithLiterals: UnionWithLiterals): Promise<Types.UnionWithLiterals> {
        return await this.barAgent.funUnionWithLiterals(unionWithLiterals);
    }


    async funUnionWithOnlyLiterals(unionWithLiterals: UnionWithOnlyLiterals): Promise<Types.UnionWithOnlyLiterals> {
        return await this.barAgent.funUnionWithOnlyLiterals(unionWithLiterals);
    }

    async funVoidReturn(text: string): Promise<void> {
        return await this.barAgent.funVoidReturn(text);
    }


    async funNullReturn(text: string): Promise<null> {
        return await this.barAgent.funNullReturn(text);
    }


    async funUndefinedReturn(text: string): Promise<undefined> {
        return await this.barAgent.funUndefinedReturn(text);
    }

    async funUnstructuredText(unstructuredText: UnstructuredText): Promise<string> {
        return await this.barAgent.funUnstructuredText(unstructuredText);
    }

    async funUnstructuredBinary(unstructuredText: UnstructuredBinary<['application/json']>): Promise<string> {
        return await this.barAgent.funUnstructuredBinary(unstructuredText);
    }

    async funMultimodal(multimodal: MultimodalAdvanced<InputText | InputImage>): Promise<string> {
        return await this.barAgent.funMultimodal(multimodal);
    }

    async funEitherOptional(eitherBothOptional: ResultLikeWithNoTag): Promise<ResultLikeWithNoTag> {
        return await this.barAgent.funResultNoTag(eitherBothOptional);
    }

    async funResultExact(either: ResultExact): Promise<ResultExact> {
        return this.barAgent.funResultExact(either);
    }

    async funResultLike(eitherOneOptional: ResultLike): Promise<ResultLike> {
        return await this.barAgent.funResultLike(eitherOneOptional);
    }

    // TODO: accept result type
    async funBuiltinResultVS(result: string | undefined): Promise<Result<void, string>> {
        return await this.barAgent.funBuiltinResultVS(result);
    }

    // TODO: accept result type
    async funBuiltinResultSV(result: string | undefined): Promise<Result<string, void>> {
        return await this.barAgent.funBuiltinResultSV(result);
    }

    // TODO: accept result type
    async funBuiltinResultSN(result: string | number): Promise<Result<string, number>> {
        return await this.barAgent.funBuiltinResultSN(result);
    }

    async funNoReturn(text: string) {
        return await this.barAgent.funNoReturn(text);
    }

    funArrowSync = (text: string) => {
        return this.barAgent.funArrowSync(text);
    };

}


@agent()
class BarAgent extends BaseAgent {
    constructor(
        readonly optionalStringType: string | null,
        readonly optionalUnionType: UnionType | null,
    ) {
        super();
        this.optionalStringType = optionalStringType;
        this.optionalUnionType = optionalUnionType;
    }

    // A function that takes all complex types
    //  cannot determine the type
    async funAll(
        complexType: Types.ObjectComplexType,
        unionType: Types.UnionType,
        unionComplexType: Types.UnionComplexType,
        numberType: Types.NumberType,
        stringType: Types.StringType,
        booleanType: Types.BooleanType,
        mapType: Types.MapType,
        tupleComplexType: Types.TupleComplexType,
        tupleType: Types.TupleType,
        listComplexType: Types.ListComplexType,
        objectType: Types.ObjectType,
        resultLike: ResultLike,
        resultLikeWithNoTag: ResultLikeWithNoTag,
        unionWithNull: string | number | null,
        objectWithUnionWithUndefined1: ObjectWithUnionWithUndefined1,
        objectWithUnionWithUndefined2: ObjectWithUnionWithUndefined2,
        objectWithUnionWithUndefined3: ObjectWithUnionWithUndefined3,
        objectWithUnionWithUndefined4: ObjectWithUnionWithUndefined4,
        optionalStringType: string | undefined,
        optionalUnionType: UnionType | undefined,
        taggedUnionType: TaggedUnion,
    ): Types.PromiseType {
        return Promise.resolve(`Weather for is sunny!`);
    }


    async funOptional(param1: string | number | null,
                      param2: ObjectWithUnionWithUndefined1,
                      param3: ObjectWithUnionWithUndefined2,
                      param4: ObjectWithUnionWithUndefined3,
                      param5: ObjectWithUnionWithUndefined4,
                      param6: string | undefined,
                      param7: UnionType | undefined,) {
        const concatenatedResult = {
            param1: param1,
            param2: param2.a,
            param3: param3.a,
            param4: param4.a,
            param5: param5.a,
            param6: param6,
            param7: param7,
        };

        return Promise.resolve(concatenatedResult);
    }

    async funOptionalQMark(param1: string, param2?: number, param3?: string) {
        return Promise.resolve({param1, param2, param3})
    }

    async funObjectComplexType(text: ObjectComplexType): Promise<ObjectComplexType> {
        return text
    }


    async funUnionType(unionType: UnionType): Promise<UnionType> {
        return unionType
    }

    async funUnionComplexType(unionComplexType: UnionComplexType): Promise<Types.UnionComplexType> {
        return unionComplexType
    }

    async funNumber(numberType: NumberType): Promise<NumberType> {
        return numberType
    }

    async funString(stringType: StringType): Promise<Types.StringType> {
        return stringType
    }

    async funBoolean(booleanType: BooleanType): Promise<Types.BooleanType> {
        return booleanType
    }

    async funText(mapType: MapType): Promise<Types.MapType> {
        return mapType
    }

    async funTupleComplexType(complexType: TupleComplexType): Promise<Types.TupleComplexType> {
        return complexType
    }

    async funTupleType(tupleType: TupleType): Promise<Types.TupleType> {
        return tupleType
    }

    async funListComplexType(listComplexType: ListComplexType): Promise<Types.ListComplexType> {
        return listComplexType
    }

    async funObjectType(objectType: ObjectType): Promise<ObjectType> {
        return objectType
    }

    async funUnionWithLiterals(unionWithLiterals: UnionWithLiterals): Promise<Types.UnionWithLiterals> {
        return unionWithLiterals;
    }

    async funVoidReturn(text: string): Promise<void> {
        return undefined;
    }

    async funNullReturn(text: string): Promise<null> {
        return null
    }

    async funUndefinedReturn(text: string): Promise<undefined> {
        return
    }

    async funUnstructuredText(unstructuredText: UnstructuredText): Promise<string> {
        return "foo"
    }

    async funUnstructuredBinary(unstructuredText: UnstructuredBinary<['application/json']>): Promise<string> {
        return "foo"
    }

    async funMultimodal(multimodal: MultimodalAdvanced<InputText | InputImage>): Promise<string> {
        return "foo"
    }

    async funUnionWithOnlyLiterals(unionWithLiterals: UnionWithOnlyLiterals): Promise<Types.UnionWithOnlyLiterals> {
        return unionWithLiterals;
    }

    async funTaggedUnion(taggedUnionType: TaggedUnion): Promise<TaggedUnion> {
        return taggedUnionType
    }

    async funResultNoTag(eitherBothOptional: ResultLikeWithNoTag): Promise<ResultLikeWithNoTag> {
        return eitherBothOptional
    }

    async funResultExact(either: ResultExact): Promise<ResultExact> {
        return either
    }

    async funResultLike(eitherOneOptional: ResultLike): Promise<ResultLike> {
        return eitherOneOptional
    }

    // TODO: accept result type
    funBuiltinResultVS(result: string | undefined): Result<void, string> {
        if (result) {
            return Result.err(result);
        } else {
            return Result.ok(undefined);
        }
    }

    // TODO: accept result type
    funBuiltinResultSV(result: string | undefined): Result<string, void> {
        if (result) {
            return Result.ok(result);
        } else {
            return Result.err(undefined);
        }
    }

    // TODO: accept result type
    funBuiltinResultSN(result: string | number): Result<string, number> {
        if (typeof result == "string") {
            return Result.ok(result);
        } else {
            return Result.err(result);
        }
    }

    async funNoReturn(text: string) {
        console.log('Hello World');
    }

    funArrowSync = (text: string) => {
        console.log('Hello World');
    };
}

// If this class is decorated with agent, it will fail
// This is kept here to ensure that any internal user class is not part of metadata generation.
// See package.json for metadata generation command.
class InternalClass {
    async fun1(input: string): Promise<Iterator<string>> {
        const array = ['a', 'b', 'c'];
        return array[Symbol.iterator]();
    }
}
