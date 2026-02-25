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
import { DataSchema, ElementSchema } from 'golem:agent/common@1.5.0';
import * as WitType from '../mapping/types/WitType';
import { MethodParams } from '@golemcloud/golem-ts-types-core';
import { TypeScope } from '../mapping/types/scope';
import { AgentMethodParamRegistry } from '../registry/agentMethodParamRegistry';
import { getMultimodalDataSchemaFromTypeInternal, TypeInfoInternal } from '../typeInfoInternal';
import {
  getBinaryDescriptor,
  getMultimodalParamDetails,
  getTextDescriptor,
  isMultimodalType,
} from './helpers';
import { ParameterSchemaCollection } from './paramSchema';

export function resolveMethodInputSchema(
  agentClassName: string,
  methodName: string,
  paramTypes: MethodParams,
): Either.Either<DataSchema, string> {
  const params: [string, Type.Type][] = Array.from(paramTypes);

  if (params.length === 1) {
    const [paramName, paramType] = params[0];

    if (isMultimodalType(paramType)) {
      return buildSingleMultimodalInputSchema(agentClassName, methodName, paramName, paramType);
    }
  }

  return Either.map(
    buildMethodParameterSchemas(agentClassName, methodName, paramTypes),
    (schemaCollection) => schemaCollection.getDataSchema(),
  );
}

function buildSingleMultimodalInputSchema(
  agentClassName: string,
  methodName: string,
  parameterName: string,
  parameterType: Type.Type,
): Either.Either<DataSchema, string> {
  if (parameterType.kind !== 'array') {
    return Either.left('Multimodal type is not an array');
  }

  const multimodalDetails = getMultimodalParamDetails(parameterType.element);

  if (Either.isLeft(multimodalDetails)) {
    return Either.left(`Failed to get multimodal details: ${multimodalDetails.val}`);
  }

  const typeInfo: TypeInfoInternal = {
    tag: 'multimodal',
    tsType: parameterType,
    types: multimodalDetails.val,
  };

  const dataSchema = getMultimodalDataSchemaFromTypeInternal(typeInfo);

  if (Either.isLeft(dataSchema)) {
    return Either.left(dataSchema.val);
  }

  AgentMethodParamRegistry.setType(agentClassName, methodName, parameterName, {
    tag: 'multimodal',
    tsType: parameterType,
    types: multimodalDetails.val,
  });

  return Either.right(dataSchema.val);
}

function buildMethodParameterSchemas(
  agentClassName: string,
  methodName: string,
  paramTypes: MethodParams,
): Either.Either<ParameterSchemaCollection, string> {
  const paramTypesArray = Array.from(paramTypes);
  const parameterSchemaCollection = new ParameterSchemaCollection();

  const result = Either.all(
    paramTypesArray.map((parameterInfo) =>
      Either.mapError(
        processMethodParameter(
          agentClassName,
          methodName,
          parameterInfo[0],
          parameterInfo[1],
          parameterSchemaCollection,
        ),
        (err) => `Method: \`${methodName}\`, Parameter: \`${parameterInfo[0]}\`. Error: ${err}`,
      ),
    ),
  );

  return Either.map(result, () => parameterSchemaCollection);
}

function processMethodParameter(
  agentClassName: string,
  methodName: string,
  parameterName: string,
  parameterType: Type.Type,
  accumulator: ParameterSchemaCollection,
): Either.Either<void, string> {
  const paramTypeName = parameterType.name;

  if (paramTypeName && paramTypeName === 'Principal') {
    AgentMethodParamRegistry.setType(agentClassName, methodName, parameterName, {
      tag: 'principal',
      tsType: parameterType,
    });

    accumulator.addPrincipalParameter(parameterName);

    return Either.right(undefined);
  }

  if (paramTypeName && paramTypeName === 'UnstructuredText') {
    const textDescriptor = getTextDescriptor(parameterType);

    if (Either.isLeft(textDescriptor)) {
      return Either.left(
        `Failed to get text descriptor for unstructured-text parameter ${parameterName}: ${textDescriptor.val}`,
      );
    }

    AgentMethodParamRegistry.setType(agentClassName, methodName, parameterName, {
      tag: 'unstructured-text',
      val: textDescriptor.val,
      tsType: parameterType,
    });

    const elementSchema: ElementSchema = {
      tag: 'unstructured-text',
      val: textDescriptor.val,
    };

    accumulator.addComponentModelParameter(parameterName, elementSchema);

    return Either.right(undefined);
  }

  if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
    const binaryDescriptor = getBinaryDescriptor(parameterType);

    if (Either.isLeft(binaryDescriptor)) {
      return Either.left(
        `Failed to get binary descriptor for unstructured-binary parameter ${parameterName}: ${binaryDescriptor.val}`,
      );
    }

    AgentMethodParamRegistry.setType(agentClassName, methodName, parameterName, {
      tag: 'unstructured-binary',
      val: binaryDescriptor.val,
      tsType: parameterType,
    });

    const elementSchema: ElementSchema = {
      tag: 'unstructured-binary',
      val: binaryDescriptor.val,
    };

    accumulator.addComponentModelParameter(parameterName, elementSchema);

    return Either.right(undefined);
  }

  return Either.map(
    WitType.fromTsType(
      parameterType,

      TypeScope.method(methodName, parameterName, parameterType.optional),
    ),
    (typeInfo) => {
      const witType = typeInfo[0];
      const analysedType = typeInfo[1];

      AgentMethodParamRegistry.setType(agentClassName, methodName, parameterName, {
        tag: 'analysed',
        val: analysedType,
        tsType: parameterType,
        witType: witType,
      });

      const elementSchema: ElementSchema = {
        tag: 'component-model',
        val: witType,
      };

      accumulator.addComponentModelParameter(parameterName, elementSchema);

      return undefined;
    },
  );
}
