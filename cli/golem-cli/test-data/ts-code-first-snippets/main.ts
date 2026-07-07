import { z } from 'zod';
import { defineAgent, method, s, clientFor } from '@golemcloud/golem-ts-sdk';

import {
  ObjectType,
  ObjectComplexType,
  UnionType,
  UnionComplexType,
  NumberType,
  StringType,
  BooleanType,
  MapType,
  TupleType,
  TupleComplexType,
  ListComplexType,
  Tree,
  TaggedUnion,
  UnionWithLiterals,
  UnionWithOnlyLiterals,
  ObjectWithUnionWithUndefined1,
  ObjectWithUnionWithUndefined2,
  ObjectWithUnionWithUndefined3,
  ObjectWithUnionWithUndefined4,
  ResultLike,
  ResultLikeWithNoTag,
  ResultLikeWithVoid,
  ResultExact,
} from './model';

// ---------------------------------------------------------------------------
// Shared schema fragments
// ---------------------------------------------------------------------------

// `string | number` and its nullable form (`string | number | null`).
const StringOrNumber = z.union([z.string(), z.number()]);
const StringOrNumberNullable = StringOrNumber.nullable();

// The built-in `Multimodal` payload: a list whose elements are either an
// unstructured-text or unstructured-binary reference.
const Multimodal = s.multimodal([
  { name: 'text', schema: s.unstructuredText() },
  { name: 'binary', schema: s.unstructuredBinary() },
]);

// `MultimodalAdvanced<InputText | InputImage>`, where InputImage carries a
// `Uint8Array` and InputText carries a `string` (case name = the original tag).
const MultimodalAdvanced = s.multimodal([
  { name: 'image', schema: s.uint8Array() },
  { name: 'text', schema: z.string() },
]);

// Return shape of `funOptional` (mirrors the object the handler assembles).
const FunOptionalReturn = z.object({
  param1: StringOrNumberNullable,
  param2: z.string().optional(),
  param3: StringOrNumber.optional(),
  param4: StringOrNumber.optional(),
  param5: z.string().optional(),
  param6: z.string().optional(),
  param7: UnionType.optional(),
});

// Return shape of `funOptionalQMark`.
const FunOptionalQMarkReturn = z.object({
  param1: z.string(),
  param2: z.number().optional(),
  param3: z.string().optional(),
});

// ---------------------------------------------------------------------------
// Method specs shared by both agents (identical name + schema on each side).
// The two renamed pairs (funText/funMap, funResultNoTag/funEitherOptional)
// reuse the same underlying spec below.
// ---------------------------------------------------------------------------

const mapMethod = method({ input: { mapType: MapType }, returns: MapType });
const resultNoTagMethod = method({
  input: { eitherBothOptional: ResultLikeWithNoTag },
  returns: ResultLikeWithNoTag,
});

const sharedMethods = {
  funAll: method({
    input: {
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
      resultLike: ResultLike,
      resultLikeWithNoTag: ResultLikeWithNoTag,
      unionWithNull: StringOrNumberNullable,
      objectWithUnionWithUndefined1: ObjectWithUnionWithUndefined1,
      objectWithUnionWithUndefined2: ObjectWithUnionWithUndefined2,
      objectWithUnionWithUndefined3: ObjectWithUnionWithUndefined3,
      objectWithUnionWithUndefined4: ObjectWithUnionWithUndefined4,
      optionalStringType: z.string().optional(),
      optionalUnionType: UnionType.optional(),
      taggedUnionType: TaggedUnion,
    },
    returns: z.string(),
  }),
  funOptional: method({
    input: {
      param1: StringOrNumberNullable,
      param2: ObjectWithUnionWithUndefined1,
      param3: ObjectWithUnionWithUndefined2,
      param4: ObjectWithUnionWithUndefined3,
      param5: ObjectWithUnionWithUndefined4,
      param6: z.string().optional(),
      param7: UnionType.optional(),
    },
    returns: FunOptionalReturn,
  }),
  funOptionalQMark: method({
    input: {
      param1: z.string(),
      param2: z.number().optional(),
      param3: z.string().optional(),
    },
    returns: FunOptionalQMarkReturn,
  }),
  funObjectComplexType: method({ input: { text: ObjectComplexType }, returns: ObjectComplexType }),
  funUnionType: method({ input: { unionType: UnionType }, returns: UnionType }),
  funUnionComplexType: method({
    input: { unionComplexType: UnionComplexType },
    returns: UnionComplexType,
  }),
  funNumber: method({ input: { numberType: NumberType }, returns: NumberType }),
  funString: method({ input: { stringType: StringType }, returns: StringType }),
  funBoolean: method({ input: { booleanType: BooleanType }, returns: BooleanType }),
  funTupleComplexType: method({
    input: { complexType: TupleComplexType },
    returns: TupleComplexType,
  }),
  funTupleType: method({ input: { tupleType: TupleType }, returns: TupleType }),
  funListComplexType: method({
    input: { listComplexType: ListComplexType },
    returns: ListComplexType,
  }),
  funObjectType: method({ input: { objectType: ObjectType }, returns: ObjectType }),
  funRecursive: method({ input: { tree: Tree }, returns: Tree }),
  funUnionWithLiterals: method({
    input: { unionWithLiterals: UnionWithLiterals },
    returns: UnionWithLiterals,
  }),
  funUnionWithOnlyLiterals: method({
    input: { unionWithLiterals: UnionWithOnlyLiterals },
    returns: UnionWithOnlyLiterals,
  }),
  funTaggedUnion: method({ input: { taggedUnionType: TaggedUnion }, returns: TaggedUnion }),
  funVoidReturn: method({ input: { text: z.string() }, returns: z.void() }),
  funNullReturn: method({ input: { text: z.string() }, returns: z.void() }),
  funUndefinedReturn: method({ input: { text: z.string() }, returns: z.void() }),
  funUnstructuredText: method({
    input: { unstructuredText: s.unstructuredText() },
    returns: z.string(),
  }),
  funUnstructuredBinary: method({
    input: { unstructuredText: s.unstructuredBinary({ mimeTypes: ['application/json'] }) },
    returns: z.string(),
  }),
  funMultimodal: method({ input: { multimodal: Multimodal }, returns: Multimodal }),
  funMultimodalAdvanced: method({
    input: { multimodal: MultimodalAdvanced },
    returns: MultimodalAdvanced,
  }),
  funResultExact: method({ input: { either: ResultExact }, returns: ResultExact }),
  funResultLike: method({ input: { eitherOneOptional: ResultLike }, returns: ResultLike }),
  funResultLikeWithVoid: method({
    input: { resultLikeWithVoid: ResultLikeWithVoid },
    returns: ResultLikeWithVoid,
  }),
  funBuiltinResultVS: method({
    input: { result: s.result(z.void(), z.string()) },
    returns: s.result(z.void(), z.string()),
  }),
  funBuiltinResultSV: method({
    input: { result: s.result(z.string(), z.void()) },
    returns: s.result(z.string(), z.void()),
  }),
  funBuiltinResultSN: method({
    input: { result: s.result(z.string(), z.number()) },
    returns: s.result(z.string(), z.number()),
  }),
  funNoReturn: method({ input: { text: z.string() }, returns: z.void() }),
  funArrowSync: method({ input: { text: z.string() }, returns: z.void() }),
};

// ---------------------------------------------------------------------------
// BarAgent — echoes each input back. `funText` and `funResultNoTag` are the
// names FooAgent forwards `funMap` / `funEitherOptional` to.
// ---------------------------------------------------------------------------

export const BarAgent = defineAgent({
  name: 'BarAgent',
  description: 'TS Code First BarAgent',
  id: {
    optionalStringType: z.string().nullable(),
    optionalUnionType: UnionType.nullable(),
  },
  methods: {
    ...sharedMethods,
    funText: mapMethod,
    funResultNoTag: resultNoTagMethod,
  },
});

export const BarAgentImpl = BarAgent.implement({
  init: ({ id }) => ({
    optionalStringType: id.optionalStringType,
    optionalUnionType: id.optionalUnionType,
  }),
  methods: {
    funAll() {
      return 'Weather for is sunny!';
    },
    funOptional({ param1, param2, param3, param4, param5, param6, param7 }) {
      return {
        param1,
        param2: param2.a,
        param3: param3.a,
        param4: param4.a,
        param5: param5.a,
        param6,
        param7,
      };
    },
    funOptionalQMark({ param1, param2, param3 }) {
      return { param1, param2, param3 };
    },
    funObjectComplexType({ text }) {
      return text;
    },
    funUnionType({ unionType }) {
      return unionType;
    },
    funUnionComplexType({ unionComplexType }) {
      return unionComplexType;
    },
    funNumber({ numberType }) {
      return numberType;
    },
    funString({ stringType }) {
      return stringType;
    },
    funBoolean({ booleanType }) {
      return booleanType;
    },
    funText({ mapType }) {
      return mapType;
    },
    funTupleComplexType({ complexType }) {
      return complexType;
    },
    funTupleType({ tupleType }) {
      return tupleType;
    },
    funListComplexType({ listComplexType }) {
      return listComplexType;
    },
    funObjectType({ objectType }) {
      return objectType;
    },
    funRecursive({ tree }) {
      return tree;
    },
    funUnionWithLiterals({ unionWithLiterals }) {
      return unionWithLiterals;
    },
    funUnionWithOnlyLiterals({ unionWithLiterals }) {
      return unionWithLiterals;
    },
    funTaggedUnion({ taggedUnionType }) {
      return taggedUnionType;
    },
    funVoidReturn() {},
    funNullReturn() {
      return undefined;
    },
    funUndefinedReturn() {},
    funUnstructuredText() {
      return 'foo';
    },
    funUnstructuredBinary() {
      return 'foo';
    },
    funMultimodal({ multimodal }) {
      return multimodal;
    },
    funMultimodalAdvanced({ multimodal }) {
      return multimodal;
    },
    funResultNoTag({ eitherBothOptional }) {
      return eitherBothOptional;
    },
    funResultExact({ either }) {
      return either;
    },
    funResultLike({ eitherOneOptional }) {
      return eitherOneOptional;
    },
    funResultLikeWithVoid({ resultLikeWithVoid }) {
      return resultLikeWithVoid;
    },
    funBuiltinResultVS({ result }) {
      return result;
    },
    funBuiltinResultSV({ result }) {
      return result;
    },
    funBuiltinResultSN({ result }) {
      return result;
    },
    funNoReturn() {
      console.log('Hello World');
    },
    funArrowSync() {
      console.log('Hello World');
    },
  },
});

// A typed RPC client factory for the remote BarAgent (mirrors `Client<BarAgent>`).
const barAgentClient = clientFor(BarAgent);

// ---------------------------------------------------------------------------
// FooAgent — forwards every call to its BarAgent client and returns the result.
// ---------------------------------------------------------------------------

export const FooAgent = defineAgent({
  name: 'FooAgent',
  description: 'TS Code First FooAgent',
  id: { input: z.string() },
  methods: {
    ...sharedMethods,
    funMap: mapMethod,
    funEitherOptional: resultNoTagMethod,
  },
});

export const FooAgentImpl = FooAgent.implement({
  // Build the phantom BarAgent client mirroring the old `BarAgent.get("foooo", 1)`
  // (constructor params optionalStringType = "foooo", optionalUnionType = 1).
  init: ({ id }) => ({
    input: id.input,
    barAgent: barAgentClient({ optionalStringType: 'foooo', optionalUnionType: 1 }),
  }),
  methods: {
    funAll(input) {
      return this.barAgent.funAll(input);
    },
    funOptional(input) {
      return this.barAgent.funOptional(input);
    },
    funOptionalQMark(input) {
      return this.barAgent.funOptionalQMark(input);
    },
    funObjectComplexType(input) {
      return this.barAgent.funObjectComplexType(input);
    },
    funUnionType(input) {
      return this.barAgent.funUnionType(input);
    },
    funUnionComplexType(input) {
      return this.barAgent.funUnionComplexType(input);
    },
    funNumber(input) {
      return this.barAgent.funNumber(input);
    },
    funString(input) {
      return this.barAgent.funString(input);
    },
    funBoolean(input) {
      return this.barAgent.funBoolean(input);
    },
    funMap(input) {
      return this.barAgent.funText(input);
    },
    funTupleComplexType(input) {
      return this.barAgent.funTupleComplexType(input);
    },
    funTupleType(input) {
      return this.barAgent.funTupleType(input);
    },
    funListComplexType(input) {
      return this.barAgent.funListComplexType(input);
    },
    funObjectType(input) {
      return this.barAgent.funObjectType(input);
    },
    funRecursive(input) {
      return this.barAgent.funRecursive(input);
    },
    funUnionWithLiterals(input) {
      return this.barAgent.funUnionWithLiterals(input);
    },
    funUnionWithOnlyLiterals(input) {
      return this.barAgent.funUnionWithOnlyLiterals(input);
    },
    funTaggedUnion(input) {
      return this.barAgent.funTaggedUnion(input);
    },
    funVoidReturn(input) {
      return this.barAgent.funVoidReturn(input);
    },
    funNullReturn(input) {
      return this.barAgent.funNullReturn(input);
    },
    funUndefinedReturn(input) {
      return this.barAgent.funUndefinedReturn(input);
    },
    funUnstructuredText(input) {
      return this.barAgent.funUnstructuredText(input);
    },
    funUnstructuredBinary(input) {
      return this.barAgent.funUnstructuredBinary(input);
    },
    funMultimodal(input) {
      return this.barAgent.funMultimodal(input);
    },
    funMultimodalAdvanced(input) {
      return this.barAgent.funMultimodalAdvanced(input);
    },
    funEitherOptional(input) {
      return this.barAgent.funResultNoTag(input);
    },
    funResultExact(input) {
      return this.barAgent.funResultExact(input);
    },
    funResultLike(input) {
      return this.barAgent.funResultLike(input);
    },
    funResultLikeWithVoid(input) {
      return this.barAgent.funResultLikeWithVoid(input);
    },
    funBuiltinResultVS(input) {
      return this.barAgent.funBuiltinResultVS(input);
    },
    funBuiltinResultSV(input) {
      return this.barAgent.funBuiltinResultSV(input);
    },
    funBuiltinResultSN(input) {
      return this.barAgent.funBuiltinResultSN(input);
    },
    funNoReturn(input) {
      return this.barAgent.funNoReturn(input);
    },
    funArrowSync(input) {
      return this.barAgent.funArrowSync(input);
    },
  },
});
