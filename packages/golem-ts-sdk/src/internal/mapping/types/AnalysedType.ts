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
  getTaggedUnion,
  getUnionOfLiterals,
  TaggedTypeMetadata,
  UserDefinedResultType,
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


export const  field = (name: string, typ: AnalysedType): NameTypePair => ({ name, typ });

export const case_ = (name: string, typ: AnalysedType): NameOptionTypePair => ({ name, typ });
export const optCase = (name: string, typ?: AnalysedType): NameOptionTypePair => ({ name, typ });
export const unitCase=  (name: string): NameOptionTypePair => ({ name });

 export const bool =  (): AnalysedType => ({ kind: 'bool' });
 export const str =  (): AnalysedType => ({ kind: 'string' });
 export const chr = (): AnalysedType => ({ kind: 'chr' });
 export const f64 = (): AnalysedType => ({ kind: 'f64' });
 export const f32 = (): AnalysedType => ({ kind: 'f32' });
 export const u64 = (isBigInt: boolean): AnalysedType => ({ kind: 'u64', isBigInt });
 export const s64 = (isBigInt: boolean): AnalysedType => ({ kind: 's64' , isBigInt});
 export const u32 = (): AnalysedType => ({ kind: 'u32' });
 export const s32 = (): AnalysedType => ({ kind: 's32' });
 export const u16 = (): AnalysedType => ({ kind: 'u16' });
 export const s16 =  (): AnalysedType => ({ kind: 's16' });
 export const u8 =  (): AnalysedType => ({ kind: 'u8' });
 export const s8 =  (): AnalysedType => ({ kind: 's8' });

 export const list = (name: string | undefined, typedArrayKind: TypedArray | undefined, mapType: {keyType: AnalysedType, valueType: AnalysedType} | undefined, inner: AnalysedType): AnalysedType => ({ kind: 'list', typedArray: typedArrayKind, mapType: mapType,  value: { name: convertOptionalTypeNameToKebab(name), owner: undefined, inner } });
 export const option = (name: string| undefined, emptyType: EmptyType, inner: AnalysedType): AnalysedType => ({ kind: 'option',  emptyType: emptyType, value: { name: convertOptionalTypeNameToKebab(name), owner: undefined, inner } });
 export const tuple =  (name: string | undefined, emptyType: EmptyType | undefined, items: AnalysedType[]): AnalysedType => ({ kind: 'tuple',   emptyType: emptyType, value: { name: convertOptionalTypeNameToKebab(name), owner: undefined, items } });
 export const record = ( name: string | undefined, fields: NameTypePair[]): AnalysedType => ({ kind: 'record',  value: { name: convertOptionalTypeNameToKebab(name), owner: undefined, fields } });
 export const flags =  (name: string | undefined, names: string[]): AnalysedType => ({ kind: 'flags', value: { name: convertOptionalTypeNameToKebab(name), owner: undefined, names } });
 export const enum_ = (name: string | undefined, cases: string[]): AnalysedType => ({ kind: 'enum', value: { name: convertOptionalTypeNameToKebab(name), owner: undefined, cases } });
 export const variant = (name: string | undefined, taggedTypes: TaggedTypeMetadata[],  cases: NameOptionTypePair[]): AnalysedType => ({ kind: 'variant', taggedTypes: taggedTypes,  value: { name: convertOptionalTypeNameToKebab(name), owner: undefined, cases } });

 export const result = (name: string | undefined, resultType: CustomOrInbuilt, ok: AnalysedType | undefined, err: AnalysedType | undefined): AnalysedType =>
      ({ kind: 'result', resultType, value: { name: convertOptionalTypeNameToKebab(name), owner: undefined, ok, err } });


 export const handle =  (name: string | undefined, resourceId: AnalysedResourceId, mode: AnalysedResourceMode): AnalysedType =>
      ({ kind: 'handle', value: { name: convertOptionalTypeNameToKebab(name), owner: undefined, resourceId, mode } });



const unionTypeMapRegistry = new Map<string, AnalysedType>();

export function fromTsType(tsType: TsType, scope: Option.Option<TypeMappingScope>): Either.Either<AnalysedType, string> {
  if (Option.isSome(scope) && (scope.val.scope === "constructor" || scope.val.scope === "method")) {
    // A question mark optional is not allowed if the scope is just method or constructor
    // They are only allowed in objects and interface scopes.
    if (tsType.optional) {
      return Either.left(`Optional parameters are not supported in ${scope.val.scope}. Parameter \`${scope.val.parameterName}\` is optional. Remove \`?\` and change the type to a union with \`undefined\``);
    }
  }

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

  if (type.name === 'String') {
    return Either.left(
      "Unsupported type `String`, use `string` instead"
    )
  }

  if (type.name === 'Boolean') {
    return Either.left(
      "Unsupported type `Boolean`, use `boolean` instead"
    )
  }

  if (type.name === 'BigInt') {
    return Either.left(
      "Unsupported type `BigInt`, use `bigint` instead"
    )
  }

  if (type.name === 'Number') {
    return Either.left(
      "Unsupported type `Number`, use `number` instead"
    )
  }

  if (type.name === 'Symbol') {
    return Either.left(
      "Unsupported type `Symbol`, use `string` if possible"
    )
  }

  if (type.name === 'Date') {
    return Either.left("Unsupported type `Date`. Use a `string` if possible");
  }

  if (type.name === 'RegExp') {
    return Either.left("Unsupported type `RegExp`. Use a `string` if possible");
  }


  const scopeName = Option.isSome(scope) ? scope.val.name : undefined;

  const parameterInScope: Option.Option<string> =
    Option.isSome(scope) ? TypeMappingScope.paramName(scope.val) : Option.none();

  switch (type.kind) {
    case "boolean":
      return Either.right(bool())

    // https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Number?utm_source=chatgpt.com#number_encoding
    case "number":
      return Either.right(f64())

    case "string":
      return Either.right(str())

    case "bigint":
      return Either.right(u64(true))

    case "null":
      return Either.left("Unsupported type `null` in " + (scopeName ? `${scopeName}` : "") + " " + (Option.isSome(parameterInScope) ? `for parameter \`${parameterInScope.val}\`` : ""));

    case "undefined":
      return Either.left("Unsupported type `undefined` in " + (scopeName ? `${scopeName}` : "") + " " + (Option.isSome(parameterInScope) ? `for parameter \`${parameterInScope.val}\`` : ""));

    case "void":
      return Either.left("Unsupported type `void` in " + (scopeName ? `${scopeName}` : "") + " " + (Option.isSome(parameterInScope) ? `for parameter \`${parameterInScope.val}\`` : ""));

    case "tuple":
      const tupleElems = Either.all(type.elements.map(el => fromTsTypeInternal(el, Option.none())));
      return Either.map(tupleElems, (items) => tuple(type.name, undefined, items));

    case "union": {
      const hash = JSON.stringify(buildJSONFromType(type));

      const analysedType = unionTypeMapRegistry.get(hash);

      // We reuse the analysed only for anoymous types
      if (analysedType && !type.name) {

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


      const inbuiltResultType =
        getInbuiltResultType(type.name, type.originalTypeName, type.unionTypes, type.typeParams);

      if (inbuiltResultType) {
        if (!type.name && Either.isRight(inbuiltResultType) ) {
          unionTypeMapRegistry.set(hash, inbuiltResultType.val);
        }
        return inbuiltResultType;
      }

      let fieldIdx = 1;
      const possibleTypes: NameOptionTypePair[] = [];

      const unionOfOnlyLiterals =
        getUnionOfLiterals(type.unionTypes);

      if (Either.isLeft(unionOfOnlyLiterals)) {
        return unionOfOnlyLiterals;
      }

      if (Option.isSome(unionOfOnlyLiterals.val)) {
        const analysedType = enum_(type.name, unionOfOnlyLiterals.val.val.literals);

        if (!type.name) {
          unionTypeMapRegistry.set(hash, analysedType);
        }

        return Either.right(analysedType);
      }

      const taggedUnion =
        getTaggedUnion(type.unionTypes);

      if (Either.isLeft(taggedUnion)) {
        return taggedUnion;
      }

      if (Option.isSome(taggedUnion.val)) {

        const unionType = taggedUnion.val.val;

        switch (unionType.tag) {
          case "custom":
            const analysedTypeEither = convertTaggedTypesToVariant(type.name, unionType.val);
            return Either.map(analysedTypeEither, (result) => {

              if (!type.name) {
                unionTypeMapRegistry.set(hash, result);
              }
              return result;
            })

          case "result":
            const userDefinedEither = convertUserDefinedResultToWitResult(type.name, unionType.val);

            return Either.map(userDefinedEither, (result) => {
              if (!type.name) {
                unionTypeMapRegistry.set(hash, result);
              }
              return result;
            })
        }
      }

      // If the union includes undefined, we need to treat it as option
      if (includesUndefined(type.unionTypes)) {
        const filteredTypes = filterUndefinedTypes(
          scope,
          type.name,
          type,
          type.unionTypes,
          type.typeParams,
          type.originalTypeName
        );

        if (Either.isLeft(filteredTypes)) {
          return Either.left(filteredTypes.val);
        }

        const innerTypeEither =
          fromTsTypeInternal(filteredTypes.val, Option.none());

        if (Either.isLeft(innerTypeEither)) {
          return Either.left(innerTypeEither.val);
        }

        // Type is already optional and further loop will solve it
        if ((Option.isSome(scope) && TypeMappingScope.isOptional(scope.val))) {
          const innerType = innerTypeEither.val;

          if (!type.name){
            unionTypeMapRegistry.set(hash, innerType);
          }
          return Either.right(innerType);
        }

        if (!type.name) {
          unionTypeMapRegistry.set(hash, innerTypeEither.val);
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
        if (t.kind === "literal") {
          const name = t.literalValue;
          if (!name) {
            return Either.left(`Unable to determine the literal value`);
          }
          if (isNumberString(name)) {
            return Either.left("Literals of number type are not supported");
          }

          possibleTypes.push({
            name: trimQuotes(name),
          });
          continue;
        }

        const result = fromTsTypeInternal(t, Option.none());

        if (Either.isLeft(result)) {
          return result;
        }

        const name = type.name;

        possibleTypes.push({
          name: generateVariantCaseName(name, fieldIdx++),
          typ: result.val,
        });
      }

      const result = variant(type.name, [], possibleTypes);

      if (!type.name) {
        unionTypeMapRegistry.set(hash, result);
      }

      return Either.right(result);
    }


    case "object":
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

    case "class":
      const message =
        type.name ? `${type.name} is a class, which is not supported` : "class is not supported";

      return Either.left(`${message}. Use object instead.`)

    case "interface":
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

    case "promise":
      const inner = type.element;
      return fromTsTypeInternal(inner, Option.none());

    case "map":
      const keyT = type.key;
      const valT = type.value;

      const key = fromTsTypeInternal(keyT, Option.none());
      const value = fromTsTypeInternal(valT, Option.none());


      return Either.zipWith(key, value, (k, v) =>
        list(type.name, undefined, {keyType: k, valueType: v}, tuple(undefined, undefined, [k, v])));

    case "literal":
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

    case "alias":
      return Either.left(`Type aliases are not supported. Found alias: ${type.name ?? "<anonymous>"}`);

    case "others":
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
        return Either.left(`Unsupported recursive type \`${customTypeName}\``);
      } else {
        return Either.left(`Unsupported type \`${customTypeName}\``);
      }

    case "unresolved-type":
      return Either.left(`Failed to resolve type for \`${type.text}\`: ${type.error}`);

    case 'array':
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
}


function getScopeName(optScope: Option.Option<TypeMappingScope>): string | undefined {
  if (Option.isSome(optScope)) {
    const scope = optScope.val;

    return scope.name
  }
  return undefined;
}

function convertTaggedTypesToVariant(typeName: string | undefined, taggedTypes: TaggedTypeMetadata[]): Either.Either<AnalysedType, string> {
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
    ? fromTsTypeInternal(resultType.okType[1], Option.none())
    : undefined;

  if (okTypeResult && Either.isLeft(okTypeResult)) {
    return Either.left(okTypeResult.val);
  }

  const errTypeResult = resultType.errType
    ? fromTsTypeInternal(resultType.errType[1], Option.none())
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

function includesUndefined(
  unionTypes: TsType[]
): boolean {
  return unionTypes.some((ut) => ut.kind === "undefined" || ut.kind === "null" || ut.kind === "void");
}

function filterUndefinedTypes(
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

export function getInbuiltResultType(
  typeName: string | undefined,
  originalTypeName: string | undefined, // if aliased
  unionTypes: TsType[],
  typeParams: TsType[],
): Either.Either<AnalysedType, string> | undefined {
    const isResult = typeName === 'Result' || originalTypeName === 'Result';

    if (isResult && unionTypes.length === 2 && unionTypes[0].name === 'Ok' && unionTypes[1].name === 'Err') {
      const okType = typeParams[0];
      const errType = typeParams[1];

      const okAnalysed = fromTsTypeInternal(okType, Option.none());
      const errAnalysed = fromTsTypeInternal(errType, Option.none());

      return Either.map(Either.zipBoth(okAnalysed, errAnalysed), ([ok, err]) => {
        return result(undefined, { tag: 'inbuilt' , okEmptyType: undefined, errEmptyType: undefined}, ok, err);
      });
    }
}