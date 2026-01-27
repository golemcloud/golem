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

import { buildJSONFromType, Node, Type as CoreType } from '@golemcloud/golem-ts-types-core';
import * as Either from "../../../newTypes/either";
import * as Option from "../../../newTypes/option";
import { TypeMappingScope } from './scope';
import { generateVariantCaseName } from './name';
import { convertOptionalTypeNameToKebab, isKebabCase, isNumberString, trimQuotes } from './stringFormat';
import {
  tryTaggedUnion,
  tryUnionOfOnlyLiterals,
  TaggedTypeMetadata,
  UserDefinedResultType, LiteralUnions, TaggedUnion,
} from './taggedUnion';

type TsType = CoreType.Type;

export interface NameTypePair {
  name: string;
  typ: AnalysedType;
}

export interface NameOptionTypePair {
  name: string;
  typ?: AnalysedType;
}

export type TypedArray = 'u8' | 'u16' | 'u32' | 'big-u64' | 'i8' | 'i16' | 'i32' | 'big-i64' | 'f32' | 'f64';

export type EmptyType = 'null' | 'void' | 'undefined' | 'question-mark';

export type CustomOrInbuilt = {tag: 'custom', okValueName: string | undefined, errValueName: string | undefined} | {tag: 'inbuilt', okEmptyType: EmptyType | undefined, errEmptyType: EmptyType | undefined};

// This is similar to internal analyzed-type in wasm-rpc (golem)
// while having extra information useful for WIT -> WIT type and value mapping
export type AnalysedType =
    | { kind: 'variant'; value: TypeVariant, taggedTypes: TaggedTypeMetadata[] }
    | { kind: 'result'; value: TypeResult, resultType: CustomOrInbuilt }
    | { kind: 'option'; value: TypeOption, emptyType: EmptyType }
    | { kind: 'enum'; value: TypeEnum }
    | { kind: 'flags'; value: TypeFlags }
    | { kind: 'record'; value: TypeRecord }
    | { kind: 'tuple'; value: TypeTuple, emptyType: EmptyType | undefined }
    | { kind: 'list'; value: TypeList, typedArray: TypedArray | undefined, mapType: { keyType: AnalysedType, valueType: AnalysedType } | undefined }
    | { kind: 'string' }
    | { kind: 'chr' }
    | { kind: 'f64'  }
    | { kind: 'f32'}
    | { kind: 'u64', isBigInt: boolean  }
    | { kind: 's64', isBigInt: boolean }
    | { kind: 'u32' }
    | { kind: 's32' }
    | { kind: 'u16' }
    | { kind: 's16' }
    | { kind: 'u8' }
    | { kind: 's8' }
    | { kind: 'bool' }
    | { kind: 'handle'; value: TypeHandle };

export function getNameFromAnalysedType(typ: AnalysedType): string | undefined {
  switch (typ.kind) {
    case "string":
      return undefined
    case "chr":
      return undefined
    case "f64":
      return undefined
    case "f32":
      return undefined
    case "u64":
      return undefined
    case "s64":
      return undefined
    case "u32":
      return undefined
    case "s32":
      return undefined
    case "u16":
      return undefined
    case "s16":
      return undefined
    case "u8":
      return undefined
    case "s8":
      return undefined
    case "bool":
      return undefined
    case "handle":
      return typ.value.name;
    case 'variant':
      return typ.value.name;
    case 'result':
      return typ.value.name;
    case 'option':
      return typ.value.name;
    case 'enum':
      return typ.value.name;
    case 'flags':
      return typ.value.name;
    case 'record':
      return typ.value.name;
    case 'tuple':
      return typ.value.name;
    case 'list':
      return typ.value.name;

  }
}

export function getOwnerFromAnalysedType(typ: AnalysedType): string | undefined {
  switch (typ.kind) {
    case 'variant':
      return typ.value.owner;
    case 'result':
      return typ.value.owner;
    case 'option':
      return typ.value.owner;
    case 'enum':
      return typ.value.owner;
    case 'flags':
      return typ.value.owner;
    case 'record':
      return typ.value.owner;
    case 'tuple':
      return typ.value.owner;
    case 'list':
      return typ.value.owner;
    case 'handle':
      return typ.value.owner;
    default:
      return undefined;
  }
}

export interface TypeResult {
  name: string | undefined;
  owner: string | undefined;
  ok?: AnalysedType;
  err?: AnalysedType;
}

export interface TypeVariant {
  name: string | undefined;
  owner: string | undefined;
  cases: NameOptionTypePair[];
}

export interface TypeOption {
  name: string | undefined;
  owner: string | undefined;
  inner: AnalysedType;
}

export interface TypeEnum {
  name: string | undefined;
  owner: string | undefined;
  cases: string[];
}

export interface TypeFlags {
  name: string | undefined;
  owner: string | undefined;
  names: string[];
}

export interface TypeRecord {
  name: string | undefined;
  owner: string | undefined;
  fields: NameTypePair[];
}

export interface TypeTuple {
  name: string | undefined;
  owner: string | undefined;
  items: AnalysedType[];
}

export interface TypeList {
  name: string | undefined;
  owner: string | undefined;
  inner: AnalysedType;
}

export interface TypeHandle {
  name: string | undefined;
  owner: string | undefined;
  resourceId: AnalysedResourceId;
  mode: AnalysedResourceMode;
}

export type AnalysedResourceMode = 'owned' | 'borrowed';

export type AnalysedResourceId = number;

export function field(name: string, typ: AnalysedType): NameTypePair {
  return { name, typ };
}

export function case_(name: string, typ: AnalysedType): NameOptionTypePair {
  return { name, typ };
}

export function optCase(name: string, typ?: AnalysedType): NameOptionTypePair {
  return { name, typ };
}

export function unitCase(name: string): NameOptionTypePair {
  return { name };
}

export function bool(): AnalysedType {
  return { kind: 'bool' };
}

export function str(): AnalysedType {
  return { kind: 'string' };
}

export function chr(): AnalysedType {
  return { kind: 'chr' };
}

export function f64(): AnalysedType {
  return { kind: 'f64' };
}

export function f32(): AnalysedType {
  return { kind: 'f32' };
}

export function u64(isBigInt: boolean): AnalysedType {
  return { kind: 'u64', isBigInt };
}

export function s64(isBigInt: boolean): AnalysedType {
  return { kind: 's64', isBigInt };
}

export function u32(): AnalysedType {
  return { kind: 'u32' };
}

export function s32(): AnalysedType {
  return { kind: 's32' };
}

export function u16(): AnalysedType {
  return { kind: 'u16' };
}

export function s16(): AnalysedType {
  return { kind: 's16' };
}

export function u8(): AnalysedType {
  return { kind: 'u8' };
}

export function s8(): AnalysedType {
  return { kind: 's8' };
}

export function list(
  name: string | undefined,
  typedArrayKind: TypedArray | undefined,
  mapType: { keyType: AnalysedType; valueType: AnalysedType } | undefined,
  inner: AnalysedType,
): AnalysedType {
  return {
    kind: 'list',
    typedArray: typedArrayKind,
    mapType,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      inner,
    },
  };
}

export function option(
  name: string | undefined,
  emptyType: EmptyType,
  inner: AnalysedType,
): AnalysedType {
  return {
    kind: 'option',
    emptyType,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      inner,
    },
  };
}

export function tuple(
  name: string | undefined,
  emptyType: EmptyType | undefined,
  items: AnalysedType[],
): AnalysedType {
  return {
    kind: 'tuple',
    emptyType,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      items,
    },
  };
}

export function record(
  name: string | undefined,
  fields: NameTypePair[],
): AnalysedType {
  return {
    kind: 'record',
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      fields,
    },
  };
}

export function flags(
  name: string | undefined,
  names: string[],
): AnalysedType {
  return {
    kind: 'flags',
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      names,
    },
  };
}

export function enum_(
  name: string | undefined,
  cases: string[],
): AnalysedType {
  return {
    kind: 'enum',
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      cases,
    },
  };
}

export function variant(
  name: string | undefined,
  taggedTypes: TaggedTypeMetadata[],
  cases: NameOptionTypePair[],
): AnalysedType {
  return {
    kind: 'variant',
    taggedTypes,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      cases,
    },
  };
}

export function result(
  name: string | undefined,
  resultType: CustomOrInbuilt,
  ok: AnalysedType | undefined,
  err: AnalysedType | undefined,
): AnalysedType {
  return {
    kind: 'result',
    resultType,
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      ok,
      err,
    },
  };
}

export function handle(
  name: string | undefined,
  resourceId: AnalysedResourceId,
  mode: AnalysedResourceMode,
): AnalysedType {
  return {
    kind: 'handle',
    value: {
      name: convertOptionalTypeNameToKebab(name),
      owner: undefined,
      resourceId,
      mode,
    },
  };
}

const AnonymousUnionTypeRegistry = new Map<string, AnalysedType>();

export function fromTsType(tsType: TsType, scope: Option.Option<TypeMappingScope>): Either.Either<AnalysedType, string> {
  const result =
    fromTsTypeInternal(tsType, scope);

  if (Option.isSome(scope) && TypeMappingScope.isOptional(scope.val)) {
    return Either.map(result, (analysedType) => {

      if (analysedType.kind === 'option' && analysedType.emptyType !== 'question-mark') {
        return analysedType;
      }

      return option(undefined, "question-mark", analysedType)
    })
  }

  return result
}

export function fromTsTypeInternal(type: TsType, scope: Option.Option<TypeMappingScope>): Either.Either<AnalysedType, string> {
  const rejected = rejectBoxedTypes(type);
  if (Either.isLeft(rejected)) return rejected;

  return callHandler(type.kind, ctx(type, scope));
}

function callHandler<K extends TsType["kind"]>(
  kind: K,
  ctx: Ctx
): Either.Either<AnalysedType, string> {
  const handler = handlers[kind] as Handler<K>;
  return handler(ctx as Ctx & { type: Extract<TsType, { kind: K }> });
}

type Ctx = {
  type: TsType;
  scope: Option.Option<TypeMappingScope>;
  scopeName?: string;
  parameterInScope: Option.Option<string>;
};

function ctx(type: TsType, scope: Option.Option<TypeMappingScope>): Ctx {
  return {
    type,
    scope,
    scopeName: Option.isSome(scope) ? scope.val.name : undefined,
    parameterInScope: Option.isSome(scope)
      ? TypeMappingScope.paramName(scope.val)
      : Option.none(),
  };
}

type Handler<K extends TsType["kind"]> =
  (ctx: Ctx & { type: Extract<TsType, { kind: K }> }) => Either.Either<AnalysedType, string>;

const handlers: { [K in TsType["kind"]]: Handler<K> } = {
  "boolean": () => Either.right(bool()),
  "number":  () => Either.right(f64()),
  "string":  () => Either.right(str()),
  "bigint":  () => Either.right(u64(true)),

  "null": unsupported("null"),
  "undefined": unsupported("undefined"),
  "void": unsupported("void"),

  "tuple": handleTuple,
  "union": handleUnion,
  "object": handleObject,
  "interface": handleInterface,
  "class": unsupportedWithHint("class", "Use object instead."),
  "promise": handlePromise,
  "map": handleMap,
  "literal": handleLiteral,
  "alias": handleAlias,
  "others": handleOthers,
  "unresolved-type": handleUnresolved,
  "array": handleArray,
}

function unsupported(kind: string): Handler<any> {
  return ({ scopeName, parameterInScope }) =>
    Either.left(
      `Unsupported type \`${kind}\``
      + (scopeName ? ` in ${scopeName}` : "")
      + (Option.isSome(parameterInScope) ? ` for parameter \`${parameterInScope.val}\`` : "")
    );
}

function unsupportedWithHint(kind: string, hint: string): Handler<any> {
  return ({ scopeName, parameterInScope }) =>
    Either.left(
      `Unsupported type \`${kind}\`${scopeName ? ` in ${scopeName}` : ""}` +
      (Option.isSome(parameterInScope) ? ` for parameter \`${parameterInScope.val}\`` : "") +
      `. Hint: ${hint}`
    );
}

function rejectBoxedTypes(type: TsType): Either.Either<never, string> {
  switch (type.name) {
    case "String":  return Either.left("Unsupported type `String`, use `string` instead");
    case "Boolean": return Either.left("Unsupported type `Boolean`, use `boolean` instead");
    case "BigInt":  return Either.left("Unsupported type `BigInt`, use `bigint` instead");
    case "Number":  return Either.left("Unsupported type `Number`, use `number` instead");
    case "Symbol":  return Either.left("Unsupported type `Symbol`, use `string` if possible");
    case "Date":    return Either.left("Unsupported type `Date`. Use a `string` if possible");
    case "RegExp":  return Either.left("Unsupported type `RegExp`. Use a `string` if possible");
  }
  return Either.right(undefined as never);
}


type TupleCtx = Ctx & { type: Extract<TsType, { kind: "tuple" }> };

function handleTuple({ type }: TupleCtx): Either.Either<AnalysedType, string> {
  if (!type.elements.length) {
    return Either.left("Empty tuple types are not supported");
  }

  return Either.map(
    Either.all(type.elements.map(el => fromTsTypeInternal(el, Option.none()))),
    items => tuple(type.name, undefined, items)
  );
}

type UnionCtx = Ctx & { type: Extract<TsType, { kind: "union" }> };

function handleUnion({type, scope} : UnionCtx): Either.Either<AnalysedType, string> {
  const hash = JSON.stringify(buildJSONFromType(type));

  const analysedType = AnonymousUnionTypeRegistry.get(hash);
  const isAnonymous = !type.name;

  // We reuse the previously computed analysed-type for anonymous types with the same shape
  // This reduces the size of the generated WIT significantly
  if (analysedType && isAnonymous) {

    if (type.unionTypes.some((ut) => ut.kind === "null")) {
      return Either.right(option(undefined, "null", analysedType));
    }

    if (type.unionTypes.some((ut) => ut.kind === "undefined")) {
      return Either.right(option(undefined, "undefined", analysedType));
    }

    if (type.unionTypes.some((ut) => ut.kind === "void")) {
      return Either.right(option(undefined, "void", analysedType));
    }


    return Either.right(analysedType);
  }

  // Check for inbuilt result type first
  const inbuiltResultType: Either.Either<AnalysedType, string> | undefined  =
    tryInbuiltResultType(type.name, type.originalTypeName, type.unionTypes, type.typeParams);

  if (inbuiltResultType) {
    if (isAnonymous && Either.isRight(inbuiltResultType) ) {
      AnonymousUnionTypeRegistry.set(hash, inbuiltResultType.val);
    }

    return inbuiltResultType;
  }

  // Union field Index
  let fieldIdx = 1;
  const possibleTypes: NameOptionTypePair[] = [];

  const unionOfOnlyLiterals: Either.Either<Option.Option<LiteralUnions>, string> =
    tryUnionOfOnlyLiterals(type.unionTypes);

  if (Either.isLeft(unionOfOnlyLiterals)) {
    return unionOfOnlyLiterals;
  }

  // If the union is made up of only literals, we can convert it to enum type
  if (Option.isSome(unionOfOnlyLiterals.val)) {
    const analysedType = enum_(type.name, unionOfOnlyLiterals.val.val.literals);

    // If it's an anonymous union, we cache it to avoid any new indices being generated for the same shape
    if (isAnonymous) {
      AnonymousUnionTypeRegistry.set(hash, analysedType);
    }

    return Either.right(analysedType);
  }

  // If all elements of the union are tagged types, we can convert it to variant or result
  const taggedUnion: Either.Either<Option.Option<TaggedUnion>, string> =
    tryTaggedUnion(type.unionTypes);

  if (Either.isLeft(taggedUnion)) {
    return taggedUnion;
  }

  // If it's a tagged union, convert to variant or result
  if (Option.isSome(taggedUnion.val)) {
    const taggedUnionSome = taggedUnion.val;
    const unionType = taggedUnionSome.val;

    switch (unionType.tag) {
      case "custom":
        const analysedTypeEither: Either.Either<AnalysedType, string> =
          convertToVariantAnalysedType(type.name, unionType.val);

        return Either.map(analysedTypeEither, (result) => {

          if (isAnonymous) {
            AnonymousUnionTypeRegistry.set(hash, result);
          }
          return result;
        })

      // Checking if the tagged union resembles a result type
      case "result":
        const userDefinedResultType = unionType.val;
        const analysedTypeForCustomResult: Either.Either<AnalysedType, string> =
          convertUserDefinedResultToWitResult(type.name, userDefinedResultType);

        return Either.map(analysedTypeForCustomResult, (result) => {
          if (isAnonymous) {
            AnonymousUnionTypeRegistry.set(hash, result);
          }
          return result;
        })
    }
  }

  // If the union is neither a tagged union nor a union of only literals, we proceed with normal union handling
  // First, we check if the union includes undefined or null types in it.
  if (includesEmptyType(type.unionTypes)) {
    const unionTypeWithoutEmptyTypes = filterEmptyTypesFromUnion(
      scope,
      type.name,
      type,
      type.unionTypes,
      type.typeParams,
      type.originalTypeName
    );

    if (Either.isLeft(unionTypeWithoutEmptyTypes)) {
      return Either.left(unionTypeWithoutEmptyTypes.val);
    }

    // We keep the rest of the type and retry with rest of the union types
    const innerTypeEither: Either.Either<AnalysedType, string> =
      fromTsTypeInternal(unionTypeWithoutEmptyTypes.val, Option.none());

    if (Either.isLeft(innerTypeEither)) {
      return Either.left(innerTypeEither.val);
    }

    // Type is already optional and further loop will solve it
    if ((Option.isSome(scope) && TypeMappingScope.isOptional(scope.val))) {
      const innerType = innerTypeEither.val;

      if (isAnonymous) {
        AnonymousUnionTypeRegistry.set(hash, innerType);
      }
      return Either.right(innerType);
    }

    if (!type.name) {
      AnonymousUnionTypeRegistry.set(hash, innerTypeEither.val);
    }

    const emptyType = type.unionTypes.some((ut) => ut.kind === "null") ?  "null" :
      (type.unionTypes.some((ut) => ut.kind === "undefined") ? "undefined" : "void");

    const result = option(undefined, emptyType, innerTypeEither.val);

    return Either.right(result)
  }

  // If union has both true and false (because ts-morph consider boolean to be a union of literal true and literal false)

  const hasFalseLiteral = type.unionTypes.some(t => t.kind === 'literal' && t.literalValue === 'false');

  const hasTrueLiteral = type.unionTypes.some(type => type.kind === 'literal' && type.literalValue === 'true');

  let hasBoolean = hasFalseLiteral && hasTrueLiteral;

  let unionTypesLiteralBoolFiltered =
    type.unionTypes.filter(field => !(field.kind === 'literal' && (field.literalValue === 'false' || field.literalValue === 'true')));

  const optional =
    unionTypesLiteralBoolFiltered.find((field) => field.kind  === 'literal')?.optional;

  unionTypesLiteralBoolFiltered.push({kind: "boolean", optional: optional ?? false})

  const newUnionTypes = hasBoolean ? unionTypesLiteralBoolFiltered : type.unionTypes;

  for (const t of newUnionTypes) {
    // Special handling of literal types
    if (t.kind === "literal") {
      const name = t.literalValue;
      if (!name) {
        return Either.left(`Unable to determine the literal value`);
      }
      if (isNumberString(name)) {
        return Either.left("Literals of number type are not supported");
      }

      // If literals, ts-morph holds on to `\"` for string literals
      // and hence should be trimmed off.
      possibleTypes.push({
        name: trimQuotes(name),
      });

      continue;
    }

    // Since we are in union handling, we don't pass down any scope
    const result = fromTsTypeInternal(t, Option.none());


    if (Either.isLeft(result)) {
      return result;
    }

    possibleTypes.push({
      // Note that for untagged-unions, all elements are anonymus
      // and we generate the name using the original union type name and field index
      name: generateVariantCaseName(type.name, fieldIdx++),
      typ: result.val,
    });
  }

  const result = variant(type.name, [], possibleTypes);

  if (!type.name) {
    AnonymousUnionTypeRegistry.set(hash, result);
  }

  return Either.right(result);
}

type ObjectCtx = Ctx & { type: Extract<TsType, { kind: "object" }> };

function handleObject({ type }: ObjectCtx): Either.Either<AnalysedType, string> {
  const result = Either.all(type.properties.map((prop) => {
    const internalType = prop.getTypeAtLocation(prop.getValueDeclarationOrThrow());

    const nodes: Node[] = prop.getDeclarations();
    const node = nodes[0];

    const entityName = type.name ?? type.kind;

    if ((Node.isPropertySignature(node) || Node.isPropertyDeclaration(node)) && node.hasQuestionToken()) {
      const tsType = fromTsType(internalType, Option.some(TypeMappingScope.object(
        entityName,
        prop.getName(),
        true
      )));

      return Either.map(tsType, (analysedType) => {
        return field(prop.getName(), analysedType)
      });
    }

    const tsType = fromTsTypeInternal(internalType, Option.some(TypeMappingScope.object(
      entityName,
      prop.getName(),
      false
    )));

    return Either.map(tsType, (analysedType) => {
      return field(prop.getName(), analysedType)
    })
  }));

  if (Either.isLeft(result)) {
    return Either.left(result.val);
  }

  const fields = result.val;

  if (fields.length === 0) {
    return Either.left(`Type ${type.name} is an object but has no properties. Object types must define at least one property.`);

  }

  return Either.right(record(type.name, fields))
}

type InterfaceCtx = Ctx & { type: Extract<TsType, { kind: "interface" }> };

function handleInterface({ type }: InterfaceCtx): Either.Either<AnalysedType, string> {
  const interfaceResult = Either.all(type.properties.map((prop) => {
    const internalType = prop.getTypeAtLocation(prop.getValueDeclarationOrThrow());

    const nodes: Node[] = prop.getDeclarations();
    const node = nodes[0];

    const entityName = type.name ?? type.kind;

    if ((Node.isPropertySignature(node) || Node.isPropertyDeclaration(node)) && node.hasQuestionToken()) {
      const tsType = fromTsType(internalType, Option.some(TypeMappingScope.interface(
        entityName,
        prop.getName(),
        true
      )));

      return Either.map(tsType, (analysedType) => {
        return field(prop.getName(), analysedType)
      });
    }

    const tsType = fromTsTypeInternal(internalType, Option.some(TypeMappingScope.interface(
      entityName,
      prop.getName(),
      false
    )));

    return Either.map(tsType, (analysedType) => {
      return field(prop.getName(), analysedType)
    })
  }));

  if (Either.isLeft(interfaceResult)) {
    return Either.left(interfaceResult.val);
  }

  const interfaceFields = interfaceResult.val;

  if (interfaceFields.length === 0) {
    return Either.left(`Type ${type.name} is an object but has no properties. Object types must define at least one property.`);

  }

  return Either.right(record(type.name, interfaceFields))
}

type PromiseCtx = Ctx & { type: Extract<TsType, { kind: "promise" }> };

function handlePromise({ type }: PromiseCtx): Either.Either<AnalysedType, string> {
  const inner = type.element;
  return fromTsTypeInternal(inner, Option.none());
}

type MapCtx = Ctx & { type: Extract<TsType, { kind: "map" }> };

function handleMap({ type }: MapCtx): Either.Either<AnalysedType, string> {
  const keyT = type.key;
  const valT = type.value;

  const key = fromTsTypeInternal(keyT, Option.none());
  const value = fromTsTypeInternal(valT, Option.none());


  return Either.zipWith(key, value, (k, v) =>
    list(type.name, undefined, {keyType: k, valueType: v}, tuple(undefined, undefined, [k, v])));
}

type LiteralCtx = Ctx & { type: Extract<TsType, { kind: "literal" }> };

function handleLiteral({ type }: LiteralCtx): Either.Either<AnalysedType, string> {
  const literalName = type.literalValue;

  if (!literalName) {
    return Either.left(`internal error: failed to retrieve the literal value from type of kind ${type.kind}`);
  }

  if (literalName === 'true' || literalName === 'false') {
    return Either.right(bool());
  }

  if (isNumberString(literalName)) {
    return Either.left("Literals of number type are not supported");
  }

  return Either.right(enum_(type.name, [trimQuotes(literalName)]))
}

type AliasCtx = Ctx & { type: Extract<TsType, { kind: "alias" }> };

function handleAlias({ type }: AliasCtx): Either.Either<AnalysedType, string> {
  return Either.left(`Type aliases are not supported. Found alias: ${type.name ?? "<anonymous>"}`);
}

// Types that are known but tagged as "others"
type OthersCtx = Ctx & { type: Extract<TsType, { kind: "others" }> };

function handleOthers({ type }: OthersCtx): Either.Either<AnalysedType, string> {
  const customTypeName = type.name

  if (!customTypeName) {
    return Either.left("Unsupported type (anonymous) found.");
  }

  if (customTypeName === 'any') {
    return Either.left("Unsupported type `any`. Use a specific type instead");
  }

  if (customTypeName === 'Date') {
    return Either.left("Unsupported type `Date`. Use a string in ISO 8601 format instead");
  }

  if (customTypeName === 'next') {
    return Either.left("Unsupported type `Iterator`. Use `Array` type instead");
  }

  if (customTypeName.includes('asyncIterator')) {
    return Either.left(`Unsupported type \`AsyncIterator\`. Use \`Array\` type instead`);
  }

  if (customTypeName.includes('iterator')) {
    return Either.left(`Unsupported type \`Iterable\`. Use \`Array\` type instead`);
  }

  if (customTypeName.includes('asyncIterable')) {
    return Either.left(`Unsupported type \`AsyncIterable\`. Use \`Array\` type instead`);
  }

  if (customTypeName === 'Record') {
    return Either.left(`Unsupported type \`${customTypeName}\`. Use a plain object or a \`Map\` type instead`);
  }

  if (type.recursive) {
    return Either.left(`\`${customTypeName}\` is recursive.\nRecursive types are not supported yet. \nHelp: Avoid recursion in this type (e.g. using index-based node lists) and try again.`);
  } else {
    return Either.left(`Unsupported type \`${customTypeName}\``);
  }
}

// Types that are fully unknown
type UnresolvedCtx = Ctx & { type: Extract<TsType, { kind: "unresolved-type" }> };

function handleUnresolved({ type }: UnresolvedCtx): Either.Either<AnalysedType, string> {
  return Either.left(`Failed to resolve type for \`${type.text}\`: ${type.error}`);
}

type ArrayCtx = Ctx & { type: Extract<TsType, { kind: "array" }> };

function handleArray({ type }: ArrayCtx): Either.Either<AnalysedType, string> {
  const name = type.name;

  switch (name) {
    case "Float64Array": return Either.right(list(undefined, 'f64', undefined, f64()));
    case "Float32Array": return Either.right(list(undefined, 'f32', undefined, f32()));
    case "Int8Array":    return Either.right(list(undefined, 'i8', undefined, s8()));
    case "Uint8Array":   return Either.right(list(undefined,  'u8', undefined, u8()));
    case "Int16Array":   return Either.right(list(undefined,  'i16', undefined, s16()));
    case "Uint16Array":  return Either.right(list(undefined,  'u16',  undefined, u16()));
    case "Int32Array":   return Either.right(list(undefined, 'i32', undefined, s32()));
    case "Uint32Array":  return Either.right(list(undefined, 'u32',  undefined, u32()));
    case "BigInt64Array":  return Either.right(list(undefined, 'big-i64', undefined, s64(true)));
    case "BigUint64Array": return Either.right(list(undefined,'big-u64', undefined, u64(true,)));
  }

  const arrayElementType =
    (type.kind === "array") ? type.element : undefined;

  if (!arrayElementType) {
    return Either.left("Unable to determine the array element type");
  }

  const elemType = fromTsTypeInternal(arrayElementType, Option.none());

  return Either.map(elemType, (inner) => list(type.name, undefined, undefined, inner));
}

function getScopeName(optScope: Option.Option<TypeMappingScope>): string | undefined {
  if (Option.isSome(optScope)) {
    const scope = optScope.val;

    return scope.name
  }
  return undefined;
}

function convertToVariantAnalysedType(typeName: string | undefined, taggedTypes: TaggedTypeMetadata[]): Either.Either<AnalysedType, string> {
  const possibleTypes: NameOptionTypePair[] = [];

  for (const taggedTypeMetadata of taggedTypes) {

    if (!isKebabCase(taggedTypeMetadata.tagLiteralName)) {
      return Either.left(`Tagged union case names must be in kebab-case. Found: ${taggedTypeMetadata.tagLiteralName}`);
    }

    if (Option.isSome(taggedTypeMetadata.valueType) && taggedTypeMetadata.valueType.val[1].kind === "literal") {
      return Either.left("Tagged unions cannot have literal types in the value section")
    }

    if (Option.isNone(taggedTypeMetadata.valueType)) {
      possibleTypes.push({
        name: taggedTypeMetadata.tagLiteralName
      })
    } else {
      const result =
        fromTsTypeInternal(taggedTypeMetadata.valueType.val[1], Option.none());

      if (Either.isLeft(result)) {
        return result;
      }

      possibleTypes.push({
        name: taggedTypeMetadata.tagLiteralName,
        typ: result.val,
      });
    }
  }

  return Either.right(variant(typeName, taggedTypes, possibleTypes));
}

function convertUserDefinedResultToWitResult(typeName: string | undefined, resultType: UserDefinedResultType): Either.Either<AnalysedType, string> {
  const okTypeResult = resultType.okType
    ? isVoidType(resultType.okType[1])
      ? undefined
      : fromTsTypeInternal(resultType.okType[1], Option.none())
    : undefined;

  if (okTypeResult && Either.isLeft(okTypeResult)) {
    return Either.left(okTypeResult.val);
  }

  const errTypeResult = resultType.errType
    ? isVoidType(resultType.errType[1])
      ? undefined : fromTsTypeInternal(resultType.errType[1], Option.none())
    : undefined;

  if (errTypeResult && Either.isLeft(errTypeResult)) {
    return Either.left(errTypeResult.val);
  }

  const okValueName = resultType.okType ? resultType.okType[0] : undefined;
  const errValueName = resultType.errType ? resultType.errType[0] : undefined;

  return Either.right(
    result(
      typeName,
      {tag: 'custom', okValueName, errValueName},
      okTypeResult ? okTypeResult.val : undefined,
      errTypeResult ? errTypeResult.val : undefined
    )
  );
}

function isVoidType(t: TsType): boolean {
  return t.kind === 'void'
}

function includesEmptyType(
  unionTypes: TsType[]
): boolean {
  return unionTypes.some((ut) => ut.kind === "undefined" || ut.kind === "null" || ut.kind === "void");
}

function filterEmptyTypesFromUnion(
  scope: Option.Option<TypeMappingScope>,
  unionTypeName: string | undefined,
  type: TsType,
  unionTypes: TsType[],
  typeParams: TsType[],
  originalTypeName: string | undefined
): Either.Either<TsType, string> {

  const scopeName = getScopeName(scope);

  const paramNameOpt: Option.Option<string> = Option.isSome(scope)
    ? TypeMappingScope.paramName(scope.val) : Option.none();


  const alternateTypes = unionTypes.filter(
    (ut) => (ut.kind !== "undefined") && (ut.kind !== "null") && (ut.kind !== "void"),
  );

  if (alternateTypes.length === 0) {
    if (Option.isSome(paramNameOpt)) {
      const paramName = paramNameOpt.val;
      return Either.left(
        `Parameter \`${paramName}\` in \`${scopeName}\` has a union type that cannot be resolved to a valid type`,
      );
    }

    return Either.left(
      `Union type cannot be resolved`,
    );
  }


  if (alternateTypes.length === 1) {
    return Either.right(alternateTypes[0]);
  }

  return Either.right({ kind: "union", name: unionTypeName, unionTypes: alternateTypes, optional: type.optional, typeParams: typeParams, originalTypeName: originalTypeName });
}

export function tryInbuiltResultType(
  typeName: string | undefined,
  originalTypeName: string | undefined, // if aliased
  unionTypes: TsType[],
  typeParams: TsType[],
): Either.Either<AnalysedType, string> | undefined {
    const isInbuiltResult = typeName === 'Result' || originalTypeName === 'Result';

    if (isInbuiltResult && unionTypes.length === 2 && unionTypes[0].name === 'Ok' && unionTypes[1].name === 'Err') {
      const okType = typeParams[0];
      const errType = typeParams[1];

      const okIsVoid = isVoidType(okType);
      const errIsVoid = isVoidType(errType);

      if (okIsVoid && errIsVoid) {
        return Either.right(result(undefined, { tag: 'inbuilt', okEmptyType: 'void', errEmptyType: 'void' }, undefined, undefined));
      }

      if (okIsVoid) {
        return Either.map(fromTsTypeInternal(errType, Option.none()), (err) =>
          result(undefined, { tag: 'inbuilt', okEmptyType: 'void', errEmptyType: undefined }, undefined, err)
        );
      }

      if (errIsVoid) {
        return Either.map(fromTsTypeInternal(okType, Option.none()), (ok) =>
          result(undefined, { tag: 'inbuilt', okEmptyType: undefined, errEmptyType: 'void' }, ok, undefined)
        );
      }

      const okAnalysed = fromTsTypeInternal(okType, Option.none());
      const errAnalysed = fromTsTypeInternal(errType, Option.none());

      return Either.map(Either.zipBoth(okAnalysed, errAnalysed), ([ok, err]) => {
        return result(undefined, { tag: 'inbuilt' , okEmptyType: undefined, errEmptyType: undefined}, ok, err);
      });
    }
}