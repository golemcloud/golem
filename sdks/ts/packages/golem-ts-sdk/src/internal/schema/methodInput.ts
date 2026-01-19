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
import * as Either from '../../newTypes/either';
import * as Option from '../../newTypes/option';
import { DataSchema, ElementSchema } from 'golem:agent/common';
import * as WitType from '../mapping/types/WitType';
import { MethodParams } from '@golemcloud/golem-ts-types-core';
import { TypeMappingScope } from '../mapping/types/scope';
import { AgentMethodParamRegistry } from '../registry/agentMethodParamRegistry';
import {
  getMultimodalDataSchema,
  TypeInfoInternal,
} from '../registry/typeInfoInternal';
import {
  getBinaryDescriptor,
  getMultimodalDetails,
  getTextDescriptor,
  isNamedMultimodal,
} from './helpers';

export function buildMethodInputSchema(
  agentClassName: string,
  methodName: string,
  paramTypes: MethodParams,
): Either.Either<DataSchema, string> {
  const paramTypesArray = Array.from(paramTypes);

  if (
    paramTypesArray.length === 1 &&
    isNamedMultimodal(paramTypesArray[0][1])
  ) {
    const paramType = paramTypesArray[0][1];

    if (isNamedMultimodal(paramType) && paramType.kind === 'array') {
      const multiModalDetails = getMultimodalDetails(paramType.element);

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

      const multimodalDataSchema = getMultimodalDataSchema(typeInfoInternal);

      if (Either.isLeft(multimodalDataSchema)) {
        return Either.left(multimodalDataSchema.val);
      }

      AgentMethodParamRegistry.setType(
        agentClassName,
        methodName,
        paramTypesArray[0][0],
        {
          tag: 'multimodal',
          tsType: paramType,
          types: multiModalDetails.val,
        },
      );

      return Either.right(multimodalDataSchema.val);
    } else {
      return Either.left('Multimodal type is not an array');
    }
  } else {
    const result = Either.all(
      paramTypesArray.map((parameterInfo) =>
        Either.mapBoth(
          convertMethodParameterToElementSchema(
            agentClassName,
            methodName,
            parameterInfo[0],
            parameterInfo[1],
            Option.some(
              TypeMappingScope.method(
                methodName,
                parameterInfo[0],
                parameterInfo[1].optional,
              ),
            ),
          ),
          (result) => {
            return [parameterInfo[0], result] as [string, ElementSchema];
          },
          (err) =>
            `Method: \`${methodName}\`, Parameter: \`${parameterInfo[0]}\`. Error: ${err}`,
        ),
      ),
    );

    return Either.map(result, (res) => {
      return {
        tag: 'tuple',
        val: res,
      };
    });
  }
}

function convertMethodParameterToElementSchema(
  agentClassName: string,
  methodName: string,
  parameterName: string,
  parameterType: Type.Type,
  scope: Option.Option<TypeMappingScope>,
): Either.Either<ElementSchema, string> {
  const paramTypeName = parameterType.name;

  if (paramTypeName && paramTypeName === 'UnstructuredText') {
    const textDescriptor = getTextDescriptor(parameterType);

    if (Either.isLeft(textDescriptor)) {
      return Either.left(
        `Failed to get text descriptor for unstructured-text parameter ${parameterName}: ${textDescriptor.val}`,
      );
    }

    AgentMethodParamRegistry.setType(
      agentClassName,
      methodName,
      parameterName,
      {
        tag: 'unstructured-text',
        val: textDescriptor.val,
        tsType: parameterType,
      },
    );

    const elementSchema: ElementSchema = {
      tag: 'unstructured-text',
      val: textDescriptor.val,
    };

    return Either.right(elementSchema);
  }

  if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
    const binaryDescriptor = getBinaryDescriptor(parameterType);

    if (Either.isLeft(binaryDescriptor)) {
      return Either.left(
        `Failed to get binary descriptor for unstructured-binary parameter ${parameterName}: ${binaryDescriptor.val}`,
      );
    }

    AgentMethodParamRegistry.setType(
      agentClassName,
      methodName,
      parameterName,
      {
        tag: 'unstructured-binary',
        val: binaryDescriptor.val,
        tsType: parameterType,
      },
    );

    const elementSchema: ElementSchema = {
      tag: 'unstructured-binary',
      val: binaryDescriptor.val,
    };

    return Either.right(elementSchema);
  }

  return Either.map(WitType.fromTsType(parameterType, scope), (typeInfo) => {
    const witType = typeInfo[0];
    const analysedType = typeInfo[1];

    AgentMethodParamRegistry.setType(
      agentClassName,
      methodName,
      parameterName,
      {
        tag: 'analysed',
        val: analysedType,
        tsType: parameterType,
        witType: witType,
      },
    );

    return {
      tag: 'component-model',
      val: witType,
    };
  });
}
