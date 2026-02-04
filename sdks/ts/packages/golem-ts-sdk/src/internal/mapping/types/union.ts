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
import * as Either from '../../../newTypes/either';
import { isKebabCase, isNumberString, trimQuotes } from './stringFormat';
import {
  tryTaggedUnion,
  tryUnionOfOnlyLiteral,
  UserDefinedResultType,
  UnionOfLiteral,
  TaggedTypeMetadata,
} from './taggedUnion';
import {
  AnalysedType,
  EmptyType,
  enum_,
  NameOptionTypePair,
  option,
  result,
  variant,
} from './analysedType';
import { Ctx } from './ctx';
import { TypeMapper } from './typeMapper';
import { Try } from '../../try';
import { generateVariantTermName } from './name';

type TsType = CoreType.Type;

const AnonymousUnionTypeRegistry = new Map<string, AnalysedType>();

type UnionCtx = Ctx & { type: Extract<TsType, { kind: 'union' }> };

export function handleUnion(
  ctx: UnionCtx,
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  const unionType = ctx.type;

  const isAnonymous = !unionType.name;

  const cacheKey = isAnonymous ? hashUnion(unionType) : undefined;

  const analysedType = handleUnionInternal(ctx, mapper, cacheKey);

  cacheAnonymousUnionType(cacheKey, analysedType);

  return analysedType;
}

export function handleUnionInternal(
  ctx: UnionCtx,
  mapper: TypeMapper,
  cacheKey: string | undefined,
): Either.Either<AnalysedType, string> {
  const { type } = ctx;

  // Reuse cached anonymous unions
  const cached = reuseAnonymousUnionCache(cacheKey);
  if (cached) return cached;

  // Try if it's inbuilt Result<T, E>
  const inbuiltResult = tryInbuiltResultType(ctx, mapper);
  if (inbuiltResult) return inbuiltResult;

  // Try if its union of only literals, and if so convert to enum
  const enumResult = tryEnumUnion(ctx);
  if (enumResult) return enumResult;

  // Try if all variants in union are tagged. Example: `{tag : 'a", val: 1} | {tag: 'b', val: 2}`.
  // If they are tagged, it further `process` it such as verifying if it corresponds to `result` type
  const taggedResult = tryTaggedUnionAndProcess(ctx, mapper);
  if (taggedResult) return taggedResult;

  // Try Optional union (null | undefined | void) and convert to `option` WIT
  const optionalResult = tryOptionalUnion(ctx, mapper);
  if (optionalResult) return optionalResult;

  // Normalize union types if it consist of `true`, `false` literals
  const normalizedUnionTypes = normalizeBooleanUnion(type.unionTypes);

  // Otherwise plain variant
  return buildPlainVariant(type, normalizedUnionTypes, mapper);
}

export function hashUnion(type: TsType): string {
  return JSON.stringify(buildJSONFromType(type));
}

function cacheAnonymousUnionType(
  cacheKey: string | undefined,
  result: Either.Either<AnalysedType, string>,
) {
  if (cacheKey && Either.isRight(result)) {
    AnonymousUnionTypeRegistry.set(cacheKey, result.val);
  }
}

// It keeps track of the "generated" `AnalysedType` mainly for anonymous union types.
// The key `string` in Map simply can be string representation of the type.
// The reason can be explained with an example.
//
// In the below function, the argument to the function is an anonymous union
//
// ```ts
//   function foo(x: string | number);
// ```
// Union handler ensures to convert `string | number` is converted to `variant { case0(string), case1(number) }` in WIT.
// But `union` handler also updates a cache of this `AnalysedType`
// which ensures with reusing the same WIT variant whenever `string | number` appears in the code
// instead of unlimited generation of case indices
function reuseAnonymousUnionCache(
  cacheKey: string | undefined,
): Either.Either<AnalysedType, string> | undefined {
  if (!cacheKey) return;

  const cachedAnalysedType: AnalysedType | undefined = AnonymousUnionTypeRegistry.get(cacheKey);

  if (!cachedAnalysedType) return;

  return Either.right(cachedAnalysedType);
}

function tryEnumUnion(ctx: UnionCtx): Either.Either<AnalysedType, string> | undefined {
  const type = ctx.type;

  const literals: Try<UnionOfLiteral> = tryUnionOfOnlyLiteral(type.unionTypes);

  // This happens because it is found that every term is a literal,
  // but there is some error specific to any of the literals,
  // and keep the error
  if (Either.isLeft(literals)) return literals;

  // Not a literal
  if (!literals.val) return;

  // Union of only literals are converted to WIT enum
  const result = enum_(type.name, literals.val.literals);

  return Either.right(result);
}

export function tryInbuiltResultType(
  ctx: UnionCtx,
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> | undefined {
  const type = ctx.type;
  const typeName = type.name;
  const originalTypeName = type.originalTypeName;
  const unionTypes = type.unionTypes;
  const typeParams = type.typeParams;

  const isInbuiltResult = typeName === 'Result' || originalTypeName === 'Result';

  if (
    isInbuiltResult &&
    unionTypes.length === 2 &&
    unionTypes[0].name === 'Ok' &&
    unionTypes[1].name === 'Err'
  ) {
    const okType = typeParams[0];
    const errType = typeParams[1];

    const okIsVoid = okType.kind === 'void';
    const errIsVoid = errType.kind === 'void';

    if (okIsVoid && errIsVoid) {
      return Either.right(
        result(
          undefined,
          { tag: 'inbuilt', okEmptyType: 'void', errEmptyType: 'void' },
          undefined,
          undefined,
        ),
      );
    }

    if (okIsVoid) {
      return Either.map(mapper(errType, undefined), (err) =>
        result(
          undefined,
          { tag: 'inbuilt', okEmptyType: 'void', errEmptyType: undefined },
          undefined,
          err,
        ),
      );
    }

    if (errIsVoid) {
      return Either.map(mapper(okType, undefined), (ok) =>
        result(
          undefined,
          { tag: 'inbuilt', okEmptyType: undefined, errEmptyType: 'void' },
          ok,
          undefined,
        ),
      );
    }

    const okAnalysed = mapper(okType, undefined);
    const errAnalysed = mapper(errType, undefined);

    return Either.map(Either.zipBoth(okAnalysed, errAnalysed), ([ok, err]) => {
      return result(
        undefined,
        { tag: 'inbuilt', okEmptyType: undefined, errEmptyType: undefined },
        ok,
        err,
      );
    });
  }
}

function tryTaggedUnionAndProcess(
  ctx: UnionCtx,
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> | undefined {
  const type = ctx.type;

  const tagged = tryTaggedUnion(type.unionTypes);

  if (Either.isLeft(tagged)) return tagged;

  if (!tagged.val) return;

  // If the tagged union resembles `Result` type, convert to `result` WIT,
  // else convert to simple `variant` WIT with each variant name corresponds to tag name.
  const result =
    tagged.val.tag === 'result'
      ? convertUserDefinedResultToWitResult(type.name, tagged.val.val, mapper)
      : convertToVariantAnalysedType(type.name, tagged.val.val, mapper);

  return result;
}

function tryOptionalUnion(
  ctx: UnionCtx,
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> | undefined {
  const emptyType: EmptyType | undefined = getFirstEmptyType(ctx.type.unionTypes);

  if (!emptyType) return;

  const stripped = filterEmptyTypesFromUnion(ctx);

  if (Either.isLeft(stripped)) return stripped;

  // Get the `AnalysedType` without taking `EmptyType` into account,
  // which is also mostly a variant/result type, unless
  // the number of types in union other than empty type is equal to 1.
  const innerAnalysedType: Either.Either<AnalysedType, string> = mapper(stripped.val, undefined);

  if (Either.isLeft(innerAnalysedType)) return innerAnalysedType;

  return Either.right(option(undefined, emptyType, innerAnalysedType.val));
}

// In typescript type-system, and hence in ts-morph
// a boolean is already `true | false`. So if we see this, then it's not a user defined
// union type but actual boolean. This is handled below, along with the possibility
// of existence of these literal `true` or `false` alongside other terms.
// Example: `true | "x"`, `false` etc is left untouched. If both of them exist,
// remove these true and false from the union, and add the actual type `boolean` into the list.
// But when doing this we ensure to keep the optionality aspect which is very subtle
// In TypeScript, these are equivalent:
// ```ts
//  x?: boolean
//  x: boolean | undefined
//
// ```
// But ts-morph does not always attach optional to undefined
// Instead it often encodes optionality like this:
// `true | false | undefined`
// At this point the `optional` flag may end up attached to one of the union members - often a literal
// and we re-attach this to the new type `boolean`.
function normalizeBooleanUnion(types: TsType[]): TsType[] {
  const hasTrue = types.some((t) => t.kind === 'literal' && t.literalValue === 'true');
  const hasFalse = types.some((t) => t.kind === 'literal' && t.literalValue === 'false');

  if (!hasTrue || !hasFalse) return types;

  const withoutBoolLiterals = types.filter(
    (t) => !(t.kind === 'literal' && (t.literalValue === 'true' || t.literalValue === 'false')),
  );

  const optional = withoutBoolLiterals.find((t) => t.kind === 'literal')?.optional ?? false;

  return [...withoutBoolLiterals, { kind: 'boolean', optional }];
}

function getFirstEmptyType(unionTypes: TsType[]): EmptyType | undefined {
  for (const t of unionTypes) {
    switch (t.kind) {
      case 'null':
        return 'null';
      case 'undefined':
        return 'undefined';
      case 'void':
        return 'void';
    }
  }
  return undefined;
}

// Filter out any empty type, but fails if there is nothing remaining.
function filterEmptyTypesFromUnion(ctx: UnionCtx): Either.Either<TsType, string> {
  const type = ctx.type;
  const alternateTypes = type.unionTypes.filter(
    (ut) => ut.kind !== 'undefined' && ut.kind !== 'null' && ut.kind !== 'void',
  );

  if (alternateTypes.length === 0) {
    if (ctx.parameterInScope) {
      const paramName = ctx.parameterInScope;

      return Either.left(
        `Parameter \`${paramName}\` in \`${ctx.scopeName}\` has a union type that cannot be resolved to a valid type`,
      );
    }

    return Either.left(`Union type cannot be resolved`);
  }

  if (alternateTypes.length === 1) {
    return Either.right(alternateTypes[0]);
  }

  return Either.right({
    kind: 'union',
    name: type.name,
    unionTypes: alternateTypes,
    optional: type.optional,
    typeParams: type.typeParams,
    originalTypeName: type.originalTypeName,
  });
}

function buildPlainVariant(
  type: TsType,
  unionTypes: TsType[],
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  let fieldIdx = 1;
  const cases: NameOptionTypePair[] = [];

  for (const t of unionTypes) {
    if (t.kind === 'literal') {
      if (!t.literalValue) {
        return Either.left('Unable to determine the literal value');
      }
      if (isNumberString(t.literalValue)) {
        return Either.left('Literals of number type are not supported');
      }

      // If they are literal types, the type info keeps the quotes as it is apparently.
      // We just remove it.
      cases.push({ name: trimQuotes(t.literalValue) });
      continue;
    }

    const analysed = mapper(t, undefined);
    if (Either.isLeft(analysed)) return analysed;

    cases.push({
      // Refer documentation of `generateVariantTermName`
      name: generateVariantTermName(type.name, fieldIdx++),
      typ: analysed.val,
    });
  }

  const result = variant(type.name, [], cases);

  return Either.right(result);
}

function convertUserDefinedResultToWitResult(
  typeName: string | undefined,
  resultType: UserDefinedResultType,
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
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
      ? undefined
      : mapper(resultType.errType[1], undefined)
    : undefined;

  if (errTypeResult && Either.isLeft(errTypeResult)) {
    return Either.left(errTypeResult.val);
  }

  const okValueName = resultType.okType ? resultType.okType[0] : undefined;
  const errValueName = resultType.errType ? resultType.errType[0] : undefined;

  return Either.right(
    result(
      typeName,
      { tag: 'custom', okValueName, errValueName },
      okTypeResult ? okTypeResult.val : undefined,
      errTypeResult ? errTypeResult.val : undefined,
    ),
  );
}

function convertToVariantAnalysedType(
  typeName: string | undefined,
  taggedTypes: TaggedTypeMetadata[],
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  const possibleTypes: NameOptionTypePair[] = [];

  for (const taggedTypeMetadata of taggedTypes) {
    if (!isKebabCase(taggedTypeMetadata.tagLiteralName)) {
      return Either.left(
        `Tagged union case names must be in kebab-case. Found: ${taggedTypeMetadata.tagLiteralName}`,
      );
    }

    if (taggedTypeMetadata.valueType && taggedTypeMetadata.valueType[1].kind === 'literal') {
      return Either.left('Tagged unions cannot have literal types in the value section');
    }

    if (!taggedTypeMetadata.valueType) {
      possibleTypes.push({
        name: taggedTypeMetadata.tagLiteralName,
      });
    } else {
      const result = mapper(taggedTypeMetadata.valueType[1], undefined);

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
