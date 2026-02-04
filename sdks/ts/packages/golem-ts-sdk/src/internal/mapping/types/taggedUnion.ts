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
import * as Either from '../../../newTypes/either';
import { isNumberString, trimQuotes } from './stringFormat';
import { TagKeyWords } from './keywords';
import { Try } from '../../try';

export type TsType = Type.Type;

export type TaggedTypeMetadata = {
  tagLiteralName: string;
  valueType: [string, Type.Type] | undefined;
};

export type UserDefinedResultType = {
  okType?: [string, TsType];
  errType?: [string, TsType];
};

export type TaggedUnion =
  | { tag: 'custom'; val: TaggedTypeMetadata[] }
  | { tag: 'result'; val: UserDefinedResultType };

export const TaggedUnion = {
  getTagNames(tu: TaggedUnion): string[] {
    return tu.tag === 'custom' ? tu.val.map((t) => t.tagLiteralName) : ['ok', 'err'];
  },

  getTaggedTypes(tu: TaggedUnion): TaggedTypeMetadata[] {
    if (tu.tag === 'custom') return tu.val;

    return [
      { tagLiteralName: 'ok', valueType: tu.val.okType ?? undefined },
      { tagLiteralName: 'err', valueType: tu.val.errType ?? undefined },
    ];
  },

  isResult(tu: TaggedUnion): tu is { tag: 'result'; val: UserDefinedResultType } {
    return tu.tag === 'result';
  },
};

export function tryTaggedUnion(unionTypes: TsType[]): Try<TaggedUnion> {
  const taggedTypeMetadata: TaggedTypeMetadata[] = [];

  for (const ut of unionTypes) {
    if (ut.kind !== 'object' || ut.properties.length > 2) {
      return Either.right(undefined);
    }

    const tag = ut.properties.find((p) => p.getName() === 'tag');
    if (!tag) return Either.right(undefined);

    const tagType = tag.getTypeAtLocation(tag.getValueDeclarationOrThrow());
    if (tagType.kind !== 'literal' || !tagType.literalValue) {
      return Either.right(undefined);
    }

    const tagValueTrimmed = trimQuotes(tagType.literalValue);

    const nextSymbol = ut.properties.find((p) => p.getName() !== 'tag');
    if (!nextSymbol) {
      taggedTypeMetadata.push({
        tagLiteralName: tagValueTrimmed,
        valueType: undefined,
      });
    } else {
      const node = nextSymbol.getDeclarations()[0];
      const propType = nextSymbol.getTypeAtLocation(nextSymbol.getValueDeclarationOrThrow());
      propType.optional = node.hasQuestionToken();

      taggedTypeMetadata.push({
        tagLiteralName: tagValueTrimmed,
        valueType: [nextSymbol.getName(), propType],
      });
    }
  }

  const eitherResultType = tryResultType(taggedTypeMetadata);
  if (Either.isLeft(eitherResultType)) return eitherResultType;
  if (eitherResultType.val) return Either.right({ tag: 'result', val: eitherResultType.val });

  const reservedKeys = taggedTypeMetadata
    .map((t) => t.tagLiteralName)
    .filter((t) => TagKeyWords.includes(t));

  if (reservedKeys.length > 0) {
    return Either.left(
      `Invalid tag value(s): \`${reservedKeys.join(', ')}\`. ` +
        `These are reserved keywords and cannot be used. ` +
        `Reserved keywords: ${TagKeyWords.join(', ')}.`,
    );
  }

  return Either.right({ tag: 'custom', val: taggedTypeMetadata });
}

function tryResultType(taggedTypes: TaggedTypeMetadata[]): Try<UserDefinedResultType> {
  if (taggedTypes.length !== 2) return Either.right(undefined);

  const okTypeMetadata = taggedTypes.find((t) => t.tagLiteralName === 'ok');
  const errTypeMetadata = taggedTypes.find((t) => t.tagLiteralName === 'err');
  if (!okTypeMetadata || !errTypeMetadata) return Either.right(undefined);

  const okType = okTypeMetadata.valueType;
  const errType = errTypeMetadata.valueType;
  if (!okType || !errType) return Either.right(undefined);

  if (okType[1].optional) {
    return Either.left(
      "The value corresponding to the tag 'ok' cannot be optional. " +
        'Avoid using the tag names `ok`, `err`. Alternatively, make the value type non optional',
    );
  }

  if (errType[1].optional) {
    return Either.left(
      "The value corresponding to the tag 'err' cannot be optional. " +
        'Avoid using the tag names `ok`, `err`. Alternatively, make the value type non optional',
    );
  }

  return Either.right({ okType, errType });
}

export type UnionOfLiteral = { literals: string[] };

export function tryUnionOfOnlyLiteral(unionTypes: TsType[]): Try<UnionOfLiteral> {
  const literals: string[] = [];

  for (const ut of unionTypes) {
    if (ut.kind !== 'literal' || !ut.literalValue) return Either.right(undefined);

    const valueTrimmed = trimQuotes(ut.literalValue);
    if (isNumberString(valueTrimmed) || valueTrimmed === 'true' || valueTrimmed === 'false') {
      return Either.right(undefined);
    }

    if (TagKeyWords.includes(valueTrimmed)) {
      return Either.left(
        `\`${valueTrimmed}\` is a reserved keyword. The following keywords cannot be used as literals: ${TagKeyWords.join(', ')}`,
      );
    }

    literals.push(valueTrimmed);
  }

  return Either.right({ literals });
}
