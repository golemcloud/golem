import { z } from 'zod';
import { s } from '@golemcloud/golem-ts-sdk';

// Every shared TS type from the decorator fixture, re-expressed as an exported
// zod (Standard Schema) schema. Where the surrounding code also needs the TS
// type, an `export type X = z.infer<typeof X>` alias accompanies the schema.

export const SimpleInterfaceType = z.object({
  n: z.number(),
});
export type SimpleInterfaceType = z.infer<typeof SimpleInterfaceType>;

export const ObjectType = z.object({
  a: z.string(),
  b: z.number(),
  c: z.boolean(),
});
export type ObjectType = z.infer<typeof ObjectType>;

// number | string | boolean | ObjectType
export const UnionType = z.union([z.number(), z.string(), z.boolean(), ObjectType]);
export type UnionType = z.infer<typeof UnionType>;

export const ListType = z.array(z.string());
export type ListType = z.infer<typeof ListType>;

export const ListComplexType = z.array(ObjectType);
export type ListComplexType = z.infer<typeof ListComplexType>;

export const TupleType = z.tuple([z.string(), z.number(), z.boolean()]);
export type TupleType = z.infer<typeof TupleType>;

export const TupleComplexType = z.tuple([z.string(), z.number(), ObjectType]);
export type TupleComplexType = z.infer<typeof TupleComplexType>;

export const MapType = z.map(z.string(), z.number());
export type MapType = z.infer<typeof MapType>;

export const BooleanType = z.boolean();
export type BooleanType = z.infer<typeof BooleanType>;

export const StringType = z.string();
export type StringType = z.infer<typeof StringType>;

export const NumberType = z.number();
export type NumberType = z.infer<typeof NumberType>;

// The resolved value of the original `Promise<string>` return type.
export const PromiseType = z.string();
export type PromiseType = z.infer<typeof PromiseType>;

export const ObjectComplexType = z.object({
  a: z.string(),
  b: z.number(),
  c: z.boolean(),
  d: ObjectType,
  e: UnionType,
  f: ListType,
  g: ListComplexType,
  h: TupleType,
  i: TupleComplexType,
  j: MapType,
  k: SimpleInterfaceType,
});
export type ObjectComplexType = z.infer<typeof ObjectComplexType>;

// number | string | boolean | ObjectComplexType | UnionType | TupleType
//   | TupleComplexType | SimpleInterfaceType | MapType | ListType | ListComplexType
export const UnionComplexType = z.union([
  z.number(),
  z.string(),
  z.boolean(),
  ObjectComplexType,
  UnionType,
  TupleType,
  TupleComplexType,
  SimpleInterfaceType,
  MapType,
  ListType,
  ListComplexType,
]);
export type UnionComplexType = z.infer<typeof UnionComplexType>;

// A tagged (discriminated) union keyed on `tag`. Cases `i`/`j` carry no payload.
export const TaggedUnion = z.discriminatedUnion('tag', [
  z.object({ tag: z.literal('a'), val: z.string() }),
  z.object({ tag: z.literal('b'), val: z.number() }),
  z.object({ tag: z.literal('c'), val: z.boolean() }),
  z.object({ tag: z.literal('d'), val: UnionType }),
  z.object({ tag: z.literal('e'), val: ObjectType }),
  z.object({ tag: z.literal('f'), val: ListType }),
  z.object({ tag: z.literal('g'), val: TupleType }),
  z.object({ tag: z.literal('h'), val: SimpleInterfaceType }),
  z.object({ tag: z.literal('i') }),
  z.object({ tag: z.literal('j') }),
]);
export type TaggedUnion = z.infer<typeof TaggedUnion>;

// 'lit1' | 'lit2' | 'lit3' | boolean
export const UnionWithLiterals = z.union([
  z.literal('lit1'),
  z.literal('lit2'),
  z.literal('lit3'),
  z.boolean(),
]);
export type UnionWithLiterals = z.infer<typeof UnionWithLiterals>;

// "foo" | "bar" | "baz" — a pure string-literal union → zod enum.
export const UnionWithOnlyLiterals = z.enum(['foo', 'bar', 'baz']);
export type UnionWithOnlyLiterals = z.infer<typeof UnionWithOnlyLiterals>;

// { a: string | undefined }
export const ObjectWithUnionWithUndefined1 = z.object({
  a: z.string().optional(),
});
export type ObjectWithUnionWithUndefined1 = z.infer<typeof ObjectWithUnionWithUndefined1>;

// { a: string | number | undefined }
export const ObjectWithUnionWithUndefined2 = z.object({
  a: z.union([z.string(), z.number()]).optional(),
});
export type ObjectWithUnionWithUndefined2 = z.infer<typeof ObjectWithUnionWithUndefined2>;

// { a?: string | number | undefined }
export const ObjectWithUnionWithUndefined3 = z.object({
  a: z.union([z.string(), z.number()]).optional(),
});
export type ObjectWithUnionWithUndefined3 = z.infer<typeof ObjectWithUnionWithUndefined3>;

// { a?: string | undefined }
export const ObjectWithUnionWithUndefined4 = z.object({
  a: z.string().optional(),
});
export type ObjectWithUnionWithUndefined4 = z.infer<typeof ObjectWithUnionWithUndefined4>;

// { ok?: string; err?: string }
export const ResultLikeWithNoTag = z.object({
  ok: z.string().optional(),
  err: z.string().optional(),
});
export type ResultLikeWithNoTag = z.infer<typeof ResultLikeWithNoTag>;

// { tag: 'okay'; value: string } | { tag: 'error'; value?: string }
export const ResultLike = z.discriminatedUnion('tag', [
  z.object({ tag: z.literal('okay'), value: z.string() }),
  z.object({ tag: z.literal('error'), value: z.string().optional() }),
]);
export type ResultLike = z.infer<typeof ResultLike>;

// { tag: 'ok', okVal: void } | { tag: 'err', errVal: void }
export const ResultLikeWithVoid = z.discriminatedUnion('tag', [
  z.object({ tag: z.literal('ok'), okVal: z.void() }),
  z.object({ tag: z.literal('err'), errVal: z.void() }),
]);
export type ResultLikeWithVoid = z.infer<typeof ResultLikeWithVoid>;

// { tag: 'ok'; value: string } | { tag: 'err'; value: string }
export const ResultExact = z.discriminatedUnion('tag', [
  z.object({ tag: z.literal('ok'), value: z.string() }),
  z.object({ tag: z.literal('err'), value: z.string() }),
]);
export type ResultExact = z.infer<typeof ResultExact>;

// Recursive tree: `Tree` references itself through `children`.
export type Tree = { label: string; children: Tree[] };
export const Tree: z.ZodType<Tree> = z.lazy(() =>
  z.object({ label: z.string(), children: z.array(Tree) }),
);

export const InterfaceType = z.object({
  numberProp: z.number(),
  stringProp: z.string(),
  booleanProp: z.boolean(),
  bigintProp: s.s64(),
  trueProp: z.literal(true),
  falseProp: z.literal(false),
  optionalProp: z.number().optional(),
  nestedProp: SimpleInterfaceType,
  unionProp: UnionType,
  unionComplexProp: UnionComplexType,
  objectProp: ObjectType,
  objectComplexProp: ObjectComplexType,
  listProp: ListType,
  listObjectProp: ListComplexType,
  tupleProp: TupleType,
  tupleObjectProp: TupleComplexType,
  mapProp: MapType,
  uint8ArrayProp: s.uint8Array(),
  uint16ArrayProp: s.uint16Array(),
  uint32ArrayProp: s.uint32Array(),
  uint64ArrayProp: s.bigUint64Array(),
  int8ArrayProp: s.int8Array(),
  int16ArrayProp: s.int16Array(),
  int32ArrayProp: s.int32Array(),
  int64ArrayProp: s.bigInt64Array(),
  float32ArrayProp: s.float32Array(),
  float64ArrayProp: s.float64Array(),
  objectPropInlined: z.object({
    a: z.string(),
    b: z.number(),
    c: z.boolean(),
  }),
  unionPropInlined: z.union([z.string(), z.number()]),
});
export type InterfaceType = z.infer<typeof InterfaceType>;
