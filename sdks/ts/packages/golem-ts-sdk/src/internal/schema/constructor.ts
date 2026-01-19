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

import { ClassMetadata, ConstructorArg } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../newTypes/either';
import * as Option from '../../newTypes/option';
import * as WitType from '../mapping/types/WitType';

import { DataSchema, ElementSchema } from 'golem:agent/common';
import {
  getBinaryDescriptor,
  getMultimodalDetails,
  getTextDescriptor,
  isNamedMultimodal,
} from './helpers';
import {
  convertTypeInfoToElementSchema,
  TypeInfoInternal,
} from '../registry/typeInfoInternal';
import { AgentConstructorParamRegistry } from '../registry/agentConstructorParamRegistry';
import { TypeMappingScope } from '../mapping/types/scope';

export function getConstructorDataSchema(
  agentClassName: string,
  classType: ClassMetadata,
): Either.Either<DataSchema, string> {
  const constructorParamInfos: readonly ConstructorArg[] =
    classType.constructorArgs;

  if (
    constructorParamInfos.length === 1 &&
    isNamedMultimodal(constructorParamInfos[0].type)
  ) {
    const paramType = constructorParamInfos[0].type;

    if (isNamedMultimodal(paramType) && paramType.kind === 'array') {
      const elementType = paramType.element;

      const multiModalDetails = getMultimodalDetails(elementType);

      if (Either.isLeft(multiModalDetails)) {
        return Either.left(
          `Failed to get multimodal details: ${multiModalDetails.val}`,
        );
      }

      const typeInfoInternal: TypeInfoInternal = {
        tag: 'multimodal',
        tsType: paramType,
        types: multiModalDetails.val,
      };

      const schemaDetails: Either.Either<[string, ElementSchema][], string> =
        Either.all(
          multiModalDetails.val.map((parameterDetail) => {
            const elementSchema = convertTypeInfoToElementSchema(
              parameterDetail.type,
            );

            if (Either.isLeft(elementSchema)) {
              return Either.left(
                `Nested multimodal types are not supported. ${parameterDetail.name}: ${elementSchema.val}`,
              );
            }

            return Either.right([parameterDetail.name, elementSchema.val] as [
              string,
              ElementSchema,
            ]);
          }),
        );

      if (Either.isLeft(schemaDetails)) {
        return Either.left(schemaDetails.val);
      }

      AgentConstructorParamRegistry.setType(
        agentClassName,
        constructorParamInfos[0].name,
        typeInfoInternal,
      );

      return Either.right({
        tag: 'multimodal',
        val: schemaDetails.val,
      });
    }
  }

  // For other type other than multimodal
  const constructDataSchemaResult: Either.Either<
    [string, ElementSchema][],
    string
  > = getConstructorParamsAndElementSchema(
    agentClassName,
    constructorParamInfos,
  );

  return Either.map(constructDataSchemaResult, (nameAndElementSchema) => {
    return {
      tag: 'tuple',
      val: nameAndElementSchema,
    };
  });
}

function getConstructorParamsAndElementSchema(
  agentClassName: string,
  constructorParamInfos: readonly ConstructorArg[],
): Either.Either<[string, ElementSchema][], string> {
  return Either.all(
    constructorParamInfos.map((paramInfo) => {
      const paramType = paramInfo.type;

      const paramTypeName = paramType.name;

      if (paramTypeName && paramTypeName === 'UnstructuredText') {
        const textDescriptor = getTextDescriptor(paramType);

        if (Either.isLeft(textDescriptor)) {
          return Either.left(
            `Failed to get text descriptor for unstructured-text parameter ${paramInfo.name}: ${textDescriptor.val}`,
          );
        }

        AgentConstructorParamRegistry.setType(agentClassName, paramInfo.name, {
          tag: 'unstructured-text',
          val: textDescriptor.val,
          tsType: paramType,
        });

        const elementSchema: ElementSchema = {
          tag: 'unstructured-text',
          val: textDescriptor.val,
        };

        return Either.right([paramInfo.name, elementSchema]);
      }

      if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
        const binaryDescriptor = getBinaryDescriptor(paramType);

        if (Either.isLeft(binaryDescriptor)) {
          return Either.left(
            `Failed to get binary descriptor for unstructured-binary parameter ${paramInfo.name}: ${binaryDescriptor.val}`,
          );
        }

        AgentConstructorParamRegistry.setType(agentClassName, paramInfo.name, {
          tag: 'unstructured-binary',
          val: binaryDescriptor.val,
          tsType: paramType,
        });

        const elementSchema: ElementSchema = {
          tag: 'unstructured-binary',
          val: binaryDescriptor.val,
        };

        return Either.right([paramInfo.name, elementSchema]);
      }

      const witType = WitType.fromTsType(
        paramInfo.type,
        Option.some(
          TypeMappingScope.constructor(
            agentClassName,
            paramInfo.name,
            paramInfo.type.optional,
          ),
        ),
      );

      return Either.map(witType, (typeInfo) => {
        const witType = typeInfo[0];
        const analysedType = typeInfo[1];

        AgentConstructorParamRegistry.setType(agentClassName, paramInfo.name, {
          tag: 'analysed',
          val: analysedType,
          witType: witType,
          tsType: paramType,
        });

        const elementSchema: ElementSchema = {
          tag: 'component-model',
          val: witType,
        };
        return [paramInfo.name, elementSchema];
      });
    }),
  );
}
