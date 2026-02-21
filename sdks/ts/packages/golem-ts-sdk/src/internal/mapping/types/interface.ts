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

import { Node, Type as CoreType } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../../newTypes/either';
import { AnalysedType, field, record } from './analysedType';
import { Ctx } from './ctx';
import { TypeScope } from './scope';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;

type InterfaceCtx = Ctx & { type: Extract<TsType, { kind: 'interface' }> };

export function handleInterface(
  { type }: InterfaceCtx,
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  const interfaceResult = Either.all(
    type.properties.map((prop) => {
      const internalType = prop.getTypeAtLocation(prop.getValueDeclarationOrThrow());

      const nodes: Node[] = prop.getDeclarations();
      const node = nodes[0];

      const entityName = type.name ?? type.kind;

      if (
        (Node.isPropertySignature(node) || Node.isPropertyDeclaration(node)) &&
        node.hasQuestionToken()
      ) {
        const tsType = mapper(internalType, TypeScope.interface(entityName, prop.getName(), true));

        return Either.map(tsType, (analysedType) => {
          return field(prop.getName(), analysedType);
        });
      }

      const tsType = mapper(internalType, TypeScope.interface(entityName, prop.getName(), false));

      return Either.map(tsType, (analysedType) => {
        return field(prop.getName(), analysedType);
      });
    }),
  );

  if (Either.isLeft(interfaceResult)) {
    return Either.left(interfaceResult.val);
  }

  const interfaceFields = interfaceResult.val;

  if (interfaceFields.length === 0) {
    return Either.left(
      `Type ${type.name} is an object but has no properties. Object types must define at least one property.`,
    );
  }

  return Either.right(record(type.name, interfaceFields));
}
