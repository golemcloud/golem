import {BaseAgent, agent, UnstructuredText,} from '@golemcloud/golem-ts-sdk';

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
    MapType, TupleComplexType, TupleType, ListComplexType, EitherBothOptional, Either, EitherOneOptional,
} from './model';


@agent()
class FooAgent extends BaseAgent {
    readonly barAgent: BarAgent;

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
            objectComplexProp: {a: "foo", b: 1, c: true, d: {a: "foo", b: 1, c: true}, e: 1, f: ["foo"], g: [{a: "foo", b: 1, c: true}], h: ["foo", 1, false], i: ["foo", 1, {a: "foo", b: 1, c: true}], j: new Map(), k: { n: 1}},
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

        this.barAgent = BarAgent.get(interfaceType, "foo", {a: "foo", b: 1, c: true} );

        this.input = input;
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
        unionWithLiterals: UnionWithLiterals,
        textType: UnstructuredText,
        eitherType: Either,
        eitherBothOptional: EitherBothOptional,
        eitherOneOptional: EitherOneOptional,
        unionWithNull: string | number | null,
        objectWithUnionWithUndefined1: ObjectWithUnionWithUndefined1,
        objectWithUnionWithUndefined2: ObjectWithUnionWithUndefined2,
        objectWithUnionWithUndefined3: ObjectWithUnionWithUndefined3,
        objectWithUnionWithUndefined4: ObjectWithUnionWithUndefined4,
        optionalStringType: string | undefined,
        optionalUnionType: UnionType | undefined,
        taggedUnionType: TaggedUnion,
        //        unionWithOnlyLiterals: UnionWithOnlyLiterals,
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
            unionWithLiterals,
            textType,
            eitherType,
            eitherBothOptional,
            eitherOneOptional,
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

    async funOptional(  param1: string | number | null,
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

    async funObjectComplexType(text: ObjectComplexType): Promise<ObjectComplexType> {
        return  await this.barAgent.funObjectComplexType(text);
    }

    async funUnionType(unionType: UnionType): Promise<UnionType> {
        return await this.barAgent.funUnionType(unionType);
    }

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

    async funText(mapType: MapType): Promise<Types.MapType> {
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

    async funUnionWithLiterals(unionWithLiterals: UnionWithLiterals): Promise<Types.UnionWithLiterals> {
        return await this.barAgent.funUnionWithLiterals(unionWithLiterals);
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

    async funUnstructuredText(unstructuredText: UnstructuredText): Promise<UnstructuredText> {
        return await this.barAgent.funUnstructuredText(unstructuredText);
    }

    async funEitherOptional(eitherBothOptional: EitherBothOptional): Promise<EitherBothOptional> {
        return await this.barAgent.funEitherOptional(eitherBothOptional);
    }

    async funEither(either: Either): Promise<Either> {
        return await this.barAgent.funEither(either);
    }

    async funEitherOneOptional(eitherOneOptional: EitherOneOptional): Promise<EitherOneOptional> {
        return await this.barAgent.funEitherOneOptional(eitherOneOptional);
    }

    async funNoReturn(text: string) {
        return await this.barAgent.funNoReturn(text);
    }

    funArrowSync = (text: string) => {
        return this.barAgent.funArrowSync(text);
    };

    // async fun10(param: "foo" | "bar" | "baz"): Promise<UnionWithOnlyLiterals> {
    //     return param;
    // }
}


@agent()
class BarAgent extends BaseAgent {
    constructor(
        readonly interfaceType: Types.InterfaceType,
        readonly optionalStringType: string | null,
        readonly optionalUnionType: UnionType | null,
    ) {
        super();
        this.interfaceType = interfaceType;
        this.optionalStringType = optionalStringType;
        this.optionalUnionType = optionalUnionType;
    }

    // A function that takes all complex types
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
        unionWithLiterals: UnionWithLiterals,
        textType: UnstructuredText,
        eitherType: Either,
        eitherBothOptional: EitherBothOptional,
        eitherOneOptional: EitherOneOptional,
        unionWithNull: string | number | null,
        objectWithUnionWithUndefined1: ObjectWithUnionWithUndefined1,
        objectWithUnionWithUndefined2: ObjectWithUnionWithUndefined2,
        objectWithUnionWithUndefined3: ObjectWithUnionWithUndefined3,
        objectWithUnionWithUndefined4: ObjectWithUnionWithUndefined4,
        optionalStringType: string | undefined,
        optionalUnionType: UnionType | undefined,
        taggedUnionType: TaggedUnion,
//        unionWithOnlyLiterals: UnionWithOnlyLiterals,
    ): Types.PromiseType {
        return Promise.resolve(`Weather for is sunny!`);
    }


    async funOptional(  param1: string | number | null,
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

    // A set
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

    async funUnstructuredText(unstructuredText: UnstructuredText): Promise<UnstructuredText> {
        return unstructuredText
    }

    async funEitherOptional(eitherBothOptional: EitherBothOptional): Promise<EitherBothOptional> {
        return { ok: 'hello' };
    }

    async funEither(either: Either): Promise<Either> {
       return either
    }

    async funEitherOneOptional(eitherOneOptional: EitherOneOptional): Promise<EitherOneOptional> {
        return eitherOneOptional
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
