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
import { isNumberString, trimQuotes } from './stringFormat';
import { TagKeyWords } from './keywords';
import { Try } from '../../try';

export type TsType = Type.Type;

//  { tag: 'a', val: string }
//  is { tagLiteralName: 'a', valueType: ['val', string] }
export type TaggedTypeMetadata = {
  tagLiteralName: string
  valueType: [string, Type.Type] | undefined,
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
          valueType: tu.val.okType ? tu.val.okType : undefined
        });
      } else {
        taggedTypes.push({
          tagLiteralName: 'ok',
          valueType: undefined
        });
      }
      if (tu.val.errType) {
        taggedTypes.push({
          tagLiteralName: 'err',
          valueType: tu.val.errType ? tu.val.errType : undefined
        });
      } else {
        taggedTypes.push({
          tagLiteralName: 'err',
          valueType: undefined
        });
      }
      return taggedTypes;
    }
  },

  isResult(tu: TaggedUnion): tu is {tag: 'result', val: UserDefinedResultType} {
    return tu.tag === 'result';
  }
}


export function tryTaggedUnion(
  unionTypes: TsType[]
): Try<TaggedUnion> {

  const taggedTypeMetadata: TaggedTypeMetadata[] = [];

  for (const ut of unionTypes) {
    if (ut.kind === "object") {

      if (ut.properties.length > 2) {
        return Either.right(undefined);
      }

      const tag =
        ut.properties.find((type) => type.getName() === "tag");

      if (!tag) {
        return Either.right(undefined);
      }

      const tagType =
        tag.getTypeAtLocation(tag.getValueDeclarationOrThrow());

      if (tagType.kind !== "literal" || !tagType.literalValue) {
        return Either.right(undefined);
      }

      const tagValue = tagType.literalValue;

      const tagValueTrimmed = trimQuotes(tagValue);

      const nextSymbol =
        ut.properties.find((type) => type.getName() !== "tag");

      if (!nextSymbol){
        taggedTypeMetadata.push({
          tagLiteralName: tagValueTrimmed,
          valueType: undefined
        });
      } else {
        const nodes = nextSymbol.getDeclarations();
        const node = nodes[0];

        const propType = nextSymbol.getTypeAtLocation(nextSymbol.getValueDeclarationOrThrow());

        propType.optional = node.hasQuestionToken();

        taggedTypeMetadata.push({
          tagLiteralName: tagValueTrimmed,
          valueType: [nextSymbol.getName(), propType]
        });
      }
    } else {
      return Either.right(undefined)
    }
  }

  const eitherType = tryResultType(taggedTypeMetadata);

  if (Either.isLeft(eitherType)) {
    return eitherType;
  }

  if (eitherType.val) {
    return Either.right({tag: 'result', val: eitherType.val});
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

  return Either.right({tag: 'custom', val: taggedTypeMetadata});
}

function tryResultType(taggedTypes: TaggedTypeMetadata[]): Try<UserDefinedResultType> {
  if (taggedTypes.length !== 2) {
    return Either.right(undefined);
  }

  const okTypeMetadata = taggedTypes.find(t => t.tagLiteralName === 'ok');
  const errTypeMetadata = taggedTypes.find(t => t.tagLiteralName === 'err');

  if (!okTypeMetadata || !errTypeMetadata) {
    return Either.right(undefined);
  }

  const okType = okTypeMetadata.valueType;
  const errType = errTypeMetadata.valueType;


  if (!okType || !errType) {
    return Either.right(undefined);
  }


  if (okType[1].optional) {
    return Either.left("The value corresponding to the tag 'ok'  cannot be optional. Avoid using the tag names `ok`, `err`. Alternatively, make the value type non optional");
  }

  if(errType[1].optional) {
    return Either.left("The value corresponding to the tag 'err' cannot be optional. Avoid using the tag names `ok , `err`. Alternatively,  make the value type non optional");
  }

  return Either.right({ okType, errType });
}


export type UnionOfLiteral = {
  literals: string[]
}

export function tryUnionOfOnlyLiteral(
  unionTypes: TsType[]
): Try<UnionOfLiteral> {

  const literals: string[] = [];

  for (const ut of unionTypes) {
    if (ut.kind === "literal" && ut.literalValue) {
      const literalValue = ut.literalValue;
      if (isNumberString(literalValue)) {
        return Either.right(undefined);
      }

      if (literalValue === 'true' || literalValue === 'false') {
        return Either.right(undefined);
      }

      const literalValueTrimmed = trimQuotes(literalValue);

      if (TagKeyWords.includes(literalValueTrimmed)) {
        return Either.left(`\`${literalValueTrimmed}\` is a reserved keyword. The following keywords cannot be used as literals: ` + TagKeyWords.join(', '));
      }

      literals.push(literalValueTrimmed);
    } else {
      return Either.right(undefined);
    }
  }

  return Either.right({ literals });
}
