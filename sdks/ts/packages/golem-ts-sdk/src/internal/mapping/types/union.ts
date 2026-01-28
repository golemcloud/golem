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

import { buildJSONFromType, Type as CoreType } from '@golemcloud/golem-ts-types-core';
import * as Either from "../../../newTypes/either";
import { TypeMappingScope } from './scope';
import { generateVariantCaseName } from './name';
import { isKebabCase, isNumberString, trimQuotes } from './stringFormat';
import {
  tryTaggedUnion,
  tryUnionOfOnlyLiteral,
  UserDefinedResultType, UnionOfLiteral, TaggedUnion, TaggedTypeMetadata,
} from './taggedUnion';
import { AnalysedType, EmptyType, enum_, NameOptionTypePair, option, result, variant } from './analysedType';
import { Ctx } from './ctx';
import {TypeMapper} from "./typeMapper";
import { Try } from '../../try';

type TsType = CoreType.Type;

const AnonymousUnionTypeRegistry = new Map<string, AnalysedType>();

type UnionCtx = Ctx & { type: Extract<TsType, { kind: "union" }> };

// TODO; Refactor more here, (added only comments for now to avoid losing changes)
export function handleUnion(ctx : UnionCtx, mapper: TypeMapper): Either.Either<AnalysedType, string> {
  const { type, scope } = ctx;
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
    tryInbuiltResultType(type.name, type.originalTypeName, type.unionTypes, type.typeParams, mapper);

  if (inbuiltResultType) {
    if (isAnonymous && Either.isRight(inbuiltResultType) ) {
      AnonymousUnionTypeRegistry.set(hash, inbuiltResultType.val);
    }

    return inbuiltResultType;
  }

  // Union field Index
  let fieldIdx = 1;
  const possibleTypes: NameOptionTypePair[] = [];

  const unionOfOnlyLiterals: Try<UnionOfLiteral> =
    tryUnionOfOnlyLiteral(type.unionTypes);

  if (Either.isLeft(unionOfOnlyLiterals)) {
    return unionOfOnlyLiterals;
  }

  // If the union is made up of only literals, we can convert it to enum type
  if (unionOfOnlyLiterals.val) {
    const analysedType = enum_(type.name, unionOfOnlyLiterals.val.literals);

    // If it's an anonymous union, we cache it to avoid any new indices being generated for the same shape
    if (isAnonymous) {
      AnonymousUnionTypeRegistry.set(hash, analysedType);
    }

    return Either.right(analysedType);
  }

  // If all elements of the union are tagged types, we can convert it to variant or result
  const taggedUnion: Try<TaggedUnion> =
    tryTaggedUnion(type.unionTypes);

  if (Either.isLeft(taggedUnion)) {
    return taggedUnion;
  }

  // If it's a tagged union, convert to variant or result
  if (taggedUnion.val) {
    const unionType = taggedUnion.val;

    switch (unionType.tag) {
      case "custom":
        const analysedTypeEither: Either.Either<AnalysedType, string> =
          convertToVariantAnalysedType(type.name, unionType.val, mapper);

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
          convertUserDefinedResultToWitResult(type.name, userDefinedResultType, mapper);

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
    const unionTypeWithoutEmptyTypes = filterEmptyTypesFromUnion(ctx);

    if (Either.isLeft(unionTypeWithoutEmptyTypes)) {
      return Either.left(unionTypeWithoutEmptyTypes.val);
    }

    // We keep the rest of the type and retry with rest of the union types
    const innerTypeEither: Either.Either<AnalysedType, string> =
      mapper(unionTypeWithoutEmptyTypes.val, undefined);

    if (Either.isLeft(innerTypeEither)) {
      return Either.left(innerTypeEither.val);
    }

    // Type is already optional and further loop will solve it
    if (scope && TypeMappingScope.isOptional(scope)) {
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
    const result = mapper(t, undefined);


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

export function tryInbuiltResultType(
  typeName: string | undefined,
  originalTypeName: string | undefined, // if aliased
  unionTypes: TsType[],
  typeParams: TsType[],
  mapper: TypeMapper
): Either.Either<AnalysedType, string> | undefined {
  const isInbuiltResult = typeName === 'Result' || originalTypeName === 'Result';

  if (isInbuiltResult && unionTypes.length === 2 && unionTypes[0].name === 'Ok' && unionTypes[1].name === 'Err') {
    const okType = typeParams[0];
    const errType = typeParams[1];

    const okIsVoid = okType.kind === 'void';
    const errIsVoid = errType.kind === 'void';

    if (okIsVoid && errIsVoid) {
      return Either.right(result(undefined, { tag: 'inbuilt', okEmptyType: 'void', errEmptyType: 'void' }, undefined, undefined));
    }

    if (okIsVoid) {
      return Either.map(mapper(errType, undefined), (err) =>
        result(undefined, { tag: 'inbuilt', okEmptyType: 'void', errEmptyType: undefined }, undefined, err)
      );
    }

    if (errIsVoid) {
      return Either.map(mapper(okType, undefined), (ok) =>
        result(undefined, { tag: 'inbuilt', okEmptyType: undefined, errEmptyType: 'void' }, ok, undefined)
      );
    }

    const okAnalysed = mapper(okType, undefined);
    const errAnalysed = mapper(errType, undefined);

    return Either.map(Either.zipBoth(okAnalysed, errAnalysed), ([ok, err]) => {
      return result(undefined, { tag: 'inbuilt' , okEmptyType: undefined, errEmptyType: undefined}, ok, err);
    });
  }
}


function includesEmptyType(
  unionTypes: TsType[]
): boolean {
  return unionTypes.some((ut) => ut.kind === "undefined" || ut.kind === "null" || ut.kind === "void");
}

function filterEmptyTypesFromUnion(
    ctx: UnionCtx
): Either.Either<TsType, string> {

  const type = ctx.type;
  const alternateTypes = type.unionTypes.filter(
    (ut) => (ut.kind !== "undefined") && (ut.kind !== "null") && (ut.kind !== "void"),
  );

  if (alternateTypes.length === 0) {
    if (ctx.parameterInScope) {
      const paramName = ctx.parameterInScope;

      return Either.left(
        `Parameter \`${paramName}\` in \`${ctx.scopeName}\` has a union type that cannot be resolved to a valid type`,
      );
    }

    return Either.left(
      `Union type cannot be resolved`,
    );
  }


  if (alternateTypes.length === 1) {
    return Either.right(alternateTypes[0]);
  }

  return Either.right({ kind: "union", name: type.name, unionTypes: alternateTypes, optional: type.optional, typeParams: type.typeParams, originalTypeName: type.originalTypeName });
}

function convertUserDefinedResultToWitResult(typeName: string | undefined, resultType: UserDefinedResultType, mapper: TypeMapper): Either.Either<AnalysedType, string> {
  const okTypeResult = resultType.okType
    ? resultType.okType[1].kind === 'void'
      ? undefined
      : mapper(resultType.okType[1], undefined)
    : undefined;

  if (okTypeResult && Either.isLeft(okTypeResult)) {
    return Either.left(okTypeResult.val);
  }

  const errTypeResult = resultType.errType
    ? resultType.errType[1].kind === 'void'
      ? undefined : mapper(resultType.errType[1], undefined)
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

function convertToVariantAnalysedType(typeName: string | undefined, taggedTypes: TaggedTypeMetadata[], mapper: TypeMapper): Either.Either<AnalysedType, string> {
  const possibleTypes: NameOptionTypePair[] = [];

  for (const taggedTypeMetadata of taggedTypes) {

    if (!isKebabCase(taggedTypeMetadata.tagLiteralName)) {
      return Either.left(`Tagged union case names must be in kebab-case. Found: ${taggedTypeMetadata.tagLiteralName}`);
    }

    if (taggedTypeMetadata.valueType && taggedTypeMetadata.valueType[1].kind === "literal") {
      return Either.left("Tagged unions cannot have literal types in the value section")
    }

    if (!taggedTypeMetadata.valueType) {
      possibleTypes.push({
        name: taggedTypeMetadata.tagLiteralName
      })
    } else {
      const result =
        mapper(taggedTypeMetadata.valueType[1], undefined);

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
