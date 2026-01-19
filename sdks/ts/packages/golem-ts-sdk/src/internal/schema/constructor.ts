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
import { ClassMetadata, ConstructorArg } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../newTypes/either';
import * as Option from '../../newTypes/option';
import * as WitType from '../mapping/types/WitType';

import { DataSchema, ElementSchema } from 'golem:agent/common';
import {
  getBinaryDescriptor,
  getMultimodalParamDetails,
  getTextDescriptor,
  isNamedMultimodal,
} from './helpers';
import {
  getMultimodalDataSchemaFromTypeInternal,
  TypeInfoInternal,
} from '../typeInfoInternal';
import { AgentConstructorParamRegistry } from '../registry/agentConstructorParamRegistry';
import { TypeMappingScope } from '../mapping/types/scope';

export function getConstructorDataSchema(
  agentClassName: string,
  classType: ClassMetadata,
): DataSchema {
  const constructorParamInfos: readonly ConstructorArg[] =
    classType.constructorArgs;

  const baseError = `Schema generation failed for agent class ${agentClassName} due to unsupported types in constructor. `;

  const multimodalInputInConstructor: Type.Type | undefined =
    getMultimodalTypeInConstructor(constructorParamInfos);

  if (
    multimodalInputInConstructor &&
    multimodalInputInConstructor.kind === 'array'
  ) {
    return getMultimodalDataSchema(
      multimodalInputInConstructor,
      multimodalInputInConstructor.element,
      baseError,
      constructorParamInfos[0].name,
      agentClassName,
    );
  }

  const paramAndSchemaCollection: [string, ElementSchema][] =
    getParamAndSchemaCollection(
      agentClassName,
      constructorParamInfos,
      baseError,
    );

  return {
    tag: 'tuple',
    val: paramAndSchemaCollection,
  };
}

function getMultimodalDataSchema(
  multimodalInputInConstructor: Type.Type,
  elementType: Type.Type,
  baseError: string,
  paramName: string,
  agentClassName: string,
): DataSchema {
  const multiModalParameters = getMultimodalParamDetails(elementType);

  if (Either.isLeft(multiModalParameters)) {
    throw new Error(
      `${baseError}. Failed to get multimodal details for constructor parameter ${paramName}: ${multiModalParameters.val}`,
    );
  }

  const typeInfoInternal: TypeInfoInternal = {
    tag: 'multimodal',
    tsType: multimodalInputInConstructor,
    types: multiModalParameters.val,
  };

  const schemaDetailsEither =
    getMultimodalDataSchemaFromTypeInternal(typeInfoInternal);

  if (Either.isLeft(schemaDetailsEither)) {
    throw new Error(
      `${baseError}. Failed to get multimodal data schema for constructor parameter ${paramName}: ${schemaDetailsEither.val}`,
    );
  }

  AgentConstructorParamRegistry.setType(
    agentClassName,
    paramName,
    typeInfoInternal,
  );

  return schemaDetailsEither.val;
}

function getMultimodalTypeInConstructor(
  constructorParamInfos: readonly ConstructorArg[],
): Type.Type | undefined {
  if (
    constructorParamInfos.length === 1 &&
    isNamedMultimodal(constructorParamInfos[0].type)
  ) {
    return constructorParamInfos[0].type;
  }
}

// For a principal, we don't track any ElementSchema
type ConstructorArgElementSchema =
  | { tag: 'principal' }
  | { tag: 'component-model'; name: string; schema: ElementSchema };

function getParamAndSchemaCollection(
  agentClassName: string,
  constructorParamInfos: readonly ConstructorArg[],
  baseError: string,
): ConstructorArgElementSchema[] {
  return constructorParamInfos.map((paramInfo) => {
    const paramType = paramInfo.type;

    const paramTypeName = paramType.name;

    if (paramTypeName && paramTypeName === 'Principal') {
      AgentConstructorParamRegistry.setType(agentClassName, paramInfo.name, {
        tag: 'principal',
        tsType: paramType,
      });

      return { tag: 'principal' };
    }

    if (paramTypeName && paramTypeName === 'UnstructuredText') {
      const textDescriptor = getTextDescriptor(paramType);

      if (Either.isLeft(textDescriptor)) {
        throw new Error(
          `${baseError}. Failed to get text descriptor for unstructured-text parameter ${paramInfo.name}: ${textDescriptor.val}`,
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

      return {
        tag: 'component-model',
        name: paramInfo.name,
        schema: elementSchema,
      };
    }

    if (paramTypeName && paramTypeName === 'UnstructuredBinary') {
      const binaryDescriptor = getBinaryDescriptor(paramType);

      if (Either.isLeft(binaryDescriptor)) {
        throw new Error(
          `${baseError}. Failed to get binary descriptor for unstructured-binary parameter ${paramInfo.name}: ${binaryDescriptor.val}`,
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

      return {
        tag: 'component-model',
        name: paramInfo.name,
        schema: elementSchema,
      };
    }

    const typeInfoEither = WitType.fromTsType(
      paramInfo.type,
      Option.some(
        TypeMappingScope.constructor(
          agentClassName,
          paramInfo.name,
          paramInfo.type.optional,
        ),
      ),
    );

    if (Either.isLeft(typeInfoEither)) {
      throw new Error(`${baseError}. ${typeInfoEither.val}`);
    }

    const typeInfo = typeInfoEither.val;

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
    return {
      tag: 'component-model',
      name: paramInfo.name,
      schema: elementSchema,
    };
  });
}
