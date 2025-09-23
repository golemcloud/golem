import {BaseAgent, agent, UnstructuredText, WithRemoteMethods} from '@golemcloud/golem-ts-sdk';

// TODO: Once the golem-ts-sdk is moved to golem, we could reuse the sample agents in the SDK tests
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
    MapType, TupleComplexType, TupleType, ListComplexType, ResultLikeWithNoTag, ResultExact, ResultLike,
} from './model';


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

    // async funAll(
    //     complexType: Types.ObjectComplexType,
    //     unionType: Types.UnionType,
    //     unionComplexType: Types.UnionComplexType,
    //     numberType: Types.NumberType,
    //     stringType: Types.StringType,
    //     booleanType: Types.BooleanType,
    //     mapType: Types.MapType,
    //     tupleComplexType: Types.TupleComplexType,
    //     tupleType: Types.TupleType,
    //     listComplexType: Types.ListComplexType,
    //     objectType: Types.ObjectType,
    //     resultLike: ResultLike,
    //     resultLikeWithNoTag: ResultLikeWithNoTag,
    //     unionWithNull: string | number | null,
    //     objectWithUnionWithUndefined1: ObjectWithUnionWithUndefined1,
    //     objectWithUnionWithUndefined2: ObjectWithUnionWithUndefined2,
    //     objectWithUnionWithUndefined3: ObjectWithUnionWithUndefined3,
    //     objectWithUnionWithUndefined4: ObjectWithUnionWithUndefined4,
    //     optionalStringType: string | undefined,
    //     optionalUnionType: UnionType | undefined,
    //     taggedUnionType: TaggedUnion,
    // ): Types.PromiseType {
    //     return await this.barAgent.funAll(
    //         complexType,
    //         unionType,
    //         unionComplexType,
    //         numberType,
    //         stringType,
    //         booleanType,
    //         mapType,
    //         tupleComplexType,
    //         tupleType,
    //         listComplexType,
    //         objectType,
    //         resultLike,
    //         resultLikeWithNoTag,
    //         unionWithNull,
    //         objectWithUnionWithUndefined1,
    //         objectWithUnionWithUndefined2,
    //         objectWithUnionWithUndefined3,
    //         objectWithUnionWithUndefined4,
    //         optionalStringType,
    //         optionalUnionType,
    //         taggedUnionType,
    //     )
    // }

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

        return this.barAgent.funOptional(
            param1,
            param2,
            param3,
            param4,
            param5,
            param6,
            param7,)
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
    //
    // // FIXME: runtime error:
    // // async funUnionWithLiterals(unionWithLiterals: UnionWithLiterals): Promise<Types.UnionWithLiterals> {
    // //     return await this.barAgent.funUnionWithLiterals(unionWithLiterals);
    // // }
    //
    //
    async funVoidReturn(text: string): Promise<void> {
        return await this.barAgent.funVoidReturn(text);
    }


    async funNullReturn(text: string): Promise<null> {
        return await this.barAgent.funNullReturn(text);
    }


    async funUndefinedReturn(text: string): Promise<undefined> {
        return await this.barAgent.funUndefinedReturn(text);
    }

    async funUnstructuredText(unstructuredText: UnstructuredText): Promise<void> {
        return await this.barAgent.funUnstructuredText(unstructuredText);
    }

    async funEitherOptional(eitherBothOptional: ResultLikeWithNoTag): Promise<ResultLikeWithNoTag> {
        return await this.barAgent.funResultNoTag(eitherBothOptional);
    }
    //
    // // FIXME: moonbit generation fails
    // // async funEither(either: ResultExact): Promise<ResultExact> {
    // //     return await this.barAgent.funEither(either);
    // // }
    //
    //
    async funResultLike(eitherOneOptional: ResultLike): Promise<ResultLike> {
        return await this.barAgent.funResultLike(eitherOneOptional);
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


    // Works
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

    // Works
    async funObjectComplexType(text: ObjectComplexType): Promise<ObjectComplexType> {
        return text
    }


    // Works
    async funUnionType(unionType: UnionType): Promise<UnionType> {
        return unionType
    }

    // works
    async funUnionComplexType(unionComplexType: UnionComplexType): Promise<Types.UnionComplexType> {
        return unionComplexType
    }

    // works
    async funNumber(numberType: NumberType): Promise<NumberType> {
        return numberType
    }

    // works
    async funString(stringType: StringType): Promise<Types.StringType> {
        return stringType
    }

    // works
    async funBoolean(booleanType: BooleanType): Promise<Types.BooleanType> {
        return booleanType
    }

    // works
    async funText(mapType: MapType): Promise<Types.MapType> {
        return mapType
    }

    // works
    async funTupleComplexType(complexType: TupleComplexType): Promise<Types.TupleComplexType> {
        return complexType
    }

    // works
    async funTupleType(tupleType: TupleType): Promise<Types.TupleType> {
        return tupleType
    }

    // works
    async funListComplexType(listComplexType: ListComplexType): Promise<Types.ListComplexType> {
        return listComplexType
    }

    // works
    async funObjectType(objectType: ObjectType): Promise<ObjectType> {
        return objectType
    }

    // no
    async funUnionWithLiterals(unionWithLiterals: UnionWithLiterals): Promise<Types.UnionWithLiterals> {
        return unionWithLiterals;
    }

    // works
    async funVoidReturn(text: string): Promise<void> {
        return undefined;
    }

    // works
    async funNullReturn(text: string): Promise<null> {
        return null
    }

    //
    async funUndefinedReturn(text: string): Promise<undefined> {
        return
    }

    // no
    async funUnstructuredText(unstructuredText: UnstructuredText): Promise<void> {
        return
    }

    //works
    async funResultNoTag(eitherBothOptional: ResultLikeWithNoTag): Promise<ResultLikeWithNoTag> {
        return eitherBothOptional
    }

    // doesn't work - moonbit generation fails
    // async funEither(either: ResultExact): Promise<ResultExact> {
    //     return either
    // }

    // no
    async funResultLike(eitherOneOptional: ResultLike): Promise<ResultLike> {
        return eitherOneOptional
    }

    // works
    async funNoReturn(text: string) {
        console.log('Hello World');
    }

    // works
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
