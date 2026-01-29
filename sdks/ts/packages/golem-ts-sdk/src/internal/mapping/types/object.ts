import { Node, Type as CoreType } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../../newTypes/either';
import { TypeScope } from './scope';
import { Ctx } from './ctx';
import { AnalysedType, field, record } from './analysedType';
import { TypeMapper } from './typeMapper';

type TsType = CoreType.Type;

type ObjectCtx = Ctx & { type: Extract<TsType, { kind: 'object' }> };

export function handleObject(
  { type }: ObjectCtx,
  mapper: TypeMapper,
): Either.Either<AnalysedType, string> {
  const result = Either.all(
    type.properties.map((prop) => {
      const internalType = prop.getTypeAtLocation(prop.getValueDeclarationOrThrow());

      const nodes: Node[] = prop.getDeclarations();
      const node = nodes[0];

      const entityName = type.name ?? type.kind;

      if (
        (Node.isPropertySignature(node) || Node.isPropertyDeclaration(node)) &&
        node.hasQuestionToken()
      ) {
        const tsType = mapper(internalType, TypeScope.object(entityName, prop.getName(), true));

        return Either.map(tsType, (analysedType) => {
          return field(prop.getName(), analysedType);
        });
      }

      const tsType = mapper(internalType, TypeScope.object(entityName, prop.getName(), false));

      return Either.map(tsType, (analysedType) => {
        return field(prop.getName(), analysedType);
      });
    }),
  );

  if (Either.isLeft(result)) {
    return Either.left(result.val);
  }

  const fields = result.val;

  if (fields.length === 0) {
    return Either.left(
      `Type ${type.name} is an object but has no properties. Object types must define at least one property.`,
    );
  }

  return Either.right(record(type.name, fields));
}
