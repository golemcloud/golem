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

import { Type } from '@golemcloud/golem-ts-types-core';
import * as Either from "../../../newTypes/either";
import * as Option from "../../../newTypes/option";
import { isNumberString, trimQuotes } from './string-format';
import { TagKeyWords } from './keywords';

export type TsType = Type.Type;

//  { tag: 'a', val: string }
//  is { tagLiteral: 'a', valueType: Option.some(['val', string]) }
export type TaggedTypeMetadata = {
  tagLiteralName: string
  valueType: Option.Option<[string, Type.Type]>,
}

export type UserDefinedResultType = {okType?: [string, TsType], errType?: [string, TsType]};

export type TaggedUnion = {tag: 'custom', val: TaggedTypeMetadata[] } | {tag: 'result', val: UserDefinedResultType }

export const TaggedUnion = {
  getTagNames(tu: TaggedUnion): string[] {
    if (tu.tag === 'custom') {
      return tu.val.map(t => t.tagLiteralName);
    } else {
      return ['ok', 'err'];
    }
  },

  getTaggedTypes(tu: TaggedUnion): TaggedTypeMetadata[] {
    if (tu.tag === 'custom') {
      return tu.val;
    } else {
      const taggedTypes: TaggedTypeMetadata[] = [];
      if (tu.val.okType) {
        taggedTypes.push({
          tagLiteralName: 'ok',
          valueType: tu.val.okType ? Option.some(tu.val.okType) : Option.none()
        });
      } else {
        taggedTypes.push({
          tagLiteralName: 'ok',
          valueType: Option.none()
        });
      }
      if (tu.val.errType) {
        taggedTypes.push({
          tagLiteralName: 'err',
          valueType: tu.val.errType ? Option.some(tu.val.errType) : Option.none()
        });
      } else {
        taggedTypes.push({
          tagLiteralName: 'err',
          valueType: Option.none()
        });
      }
      return taggedTypes;
    }
  },

  isResult(tu: TaggedUnion): tu is {tag: 'result', val: UserDefinedResultType} {
    return tu.tag === 'result';
  }
}

export function getTaggedUnion(
  unionTypes: TsType[]
): Either.Either<Option.Option<TaggedUnion>, string> {

  const taggedTypeMetadata: TaggedTypeMetadata[] = [];

  for (const ut of unionTypes) {
    if (ut.kind === "object") {

      if (ut.properties.length > 2) {
        return Either.right(Option.none());
      }

      const tag =
        ut.properties.find((type) => type.getName() === "tag");

      if (!tag) {
        return Either.right(Option.none());
      }

      const tagType =
        tag.getTypeAtLocation(tag.getValueDeclarationOrThrow());

      if (tagType.kind !== "literal" || !tagType.literalValue) {
        return Either.right(Option.none());
      }

      const tagValue = tagType.literalValue;

      const tagValueTrimmed = trimQuotes(tagValue);

      const nextSymbol =
        ut.properties.find((type) => type.getName() !== "tag");

      if (!nextSymbol){
        taggedTypeMetadata.push({
          tagLiteralName: tagValueTrimmed,
          valueType: Option.none()
        });
      } else {
        const propType = nextSymbol.getTypeAtLocation(nextSymbol.getValueDeclarationOrThrow());
        taggedTypeMetadata.push({
          tagLiteralName: tagValueTrimmed,
          valueType: Option.some([nextSymbol.getName(), propType])
        });
      }
    } else {
      return Either.right(Option.none())
    }
  }

  const eitherType = checkEitherType(taggedTypeMetadata);

  if (Option.isSome(eitherType)) {
    return Either.right(Option.some({tag: 'result', val: eitherType.val}));
  }

  const keys = taggedTypeMetadata
    .map((t) => t.tagLiteralName)
    .filter((t) => TagKeyWords.includes(t));

  if (keys.length > 0) {
    return Either.left(
      `Invalid tag value(s): \`${keys.join(", ")}\`. ` +
      `These are reserved keywords and cannot be used. ` +
      `Reserved keywords: ${TagKeyWords.join(", ")}.`
    );
  }

  return Either.right(Option.some({tag: 'custom', val: taggedTypeMetadata}));
}

function checkEitherType(taggedTypes: TaggedTypeMetadata[]): Option.Option<UserDefinedResultType> {
  if (taggedTypes.length !== 2) {
    return Option.none();
  }

  const okTypeMetadata = taggedTypes.find(t => t.tagLiteralName === 'ok');
  const errTypeMetadata = taggedTypes.find(t => t.tagLiteralName === 'err');

  if (!okTypeMetadata || !errTypeMetadata) {
    return Option.none();
  }

  const okType = Option.isSome(okTypeMetadata.valueType) ? okTypeMetadata.valueType.val : undefined;
  const errType = Option.isSome(errTypeMetadata.valueType) ? errTypeMetadata.valueType.val : undefined;

  if (!okType || !errType) {
    return Option.none();
  }

  return Option.some({ okType, errType });
}


export type LiteralUnions = {
  literals: string[]
}

export function getUnionOfLiterals(
  unionTypes: TsType[]
): Either.Either<Option.Option<LiteralUnions>, string> {

  const literals: string[] = [];

  for (const ut of unionTypes) {
    if (ut.kind === "literal" && ut.literalValue) {
      const literalValue = ut.literalValue;
      if (isNumberString(literalValue)) {
        return Either.right(Option.none());
      }

      if (literalValue === 'true' || literalValue === 'false') {
        return Either.right(Option.none());
      }

      const literalValueTrimmed = trimQuotes(literalValue);

      if (TagKeyWords.includes(literalValueTrimmed)) {
        return Either.left(`\`${literalValueTrimmed}\` is a reserved keyword. The following keywords cannot be used as literals: ` + TagKeyWords.join(', '));
      }

      literals.push(literalValueTrimmed);
    } else {
      return Either.right(Option.none());
    }
  }

  return Either.right(Option.some({ literals }));
}

