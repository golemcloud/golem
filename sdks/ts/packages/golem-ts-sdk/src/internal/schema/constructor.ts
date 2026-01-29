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
// limitations under the License

import { Type } from '@golemcloud/golem-ts-types-core';
import { ClassMetadata, ConstructorArg } from '@golemcloud/golem-ts-types-core';
import * as Either from '../../newTypes/either';
import * as WitType from '../mapping/types/WitType';

import { DataSchema } from 'golem:agent/common';
import {
  getBinaryDescriptor,
  getMultimodalParamDetails,
  getTextDescriptor,
  isMultimodalType,
} from './helpers';
import { getMultimodalDataSchemaFromTypeInternal, TypeInfoInternal } from '../typeInfoInternal';
import { AgentConstructorParamRegistry } from '../registry/agentConstructorParamRegistry';
import { TypeScope } from '../mapping/types/scope';
import { ParameterSchemaCollection } from './paramSchema';

export function getAgentConstructorSchema(
  agentClassName: string,
  classType: ClassMetadata,
): DataSchema {
  const constructorParams = classType.constructorArgs;
  const baseError = buildBaseError(agentClassName);

  const multimodal: Type.Type | undefined = getMultimodalTypeInfo(constructorParams);

  if (multimodal) {
    return resolveMultimodalConstructorSchema(
      multimodal,
      constructorParams[0].name,
      agentClassName,
      baseError,
    );
  }

  return resolveStandardConstructorSchema(agentClassName, constructorParams, baseError);
}

interface ConstructorParamHandler {
  canHandle(param: ConstructorArg): boolean;

  handle(
    agentClassName: string,
    param: ConstructorArg,
    baseError: string,
    collection: ParameterSchemaCollection,
  ): void;
}

const principalHandler: ConstructorParamHandler = {
  canHandle: (param) => param.type.name === 'Principal',

  handle: (agentClassName, param, _, collection) => {
    AgentConstructorParamRegistry.setType(agentClassName, param.name, {
      tag: 'principal',
      tsType: param.type,
    });

    collection.addPrincipalParameter(param.name);
  },
};

const unstructuredTextHandler: ConstructorParamHandler = {
  canHandle: (param) => param.type.name === 'UnstructuredText',

  handle: (agentClassName, param, baseError, collection) => {
    const descriptor = getTextDescriptor(param.type);

    if (Either.isLeft(descriptor)) {
      throw new Error(
        `${baseError}. Failed to get text descriptor for unstructured-text parameter ${param.name}: ${descriptor.val}`,
      );
    }

    AgentConstructorParamRegistry.setType(agentClassName, param.name, {
      tag: 'unstructured-text',
      val: descriptor.val,
      tsType: param.type,
    });

    collection.addComponentModelParameter(param.name, {
      tag: 'unstructured-text',
      val: descriptor.val,
    });
  },
};

const unstructuredBinaryHandler: ConstructorParamHandler = {
  canHandle: (param) => param.type.name === 'UnstructuredBinary',

  handle: (agentClassName, param, baseError, collection) => {
    const descriptor = getBinaryDescriptor(param.type);

    if (Either.isLeft(descriptor)) {
      throw new Error(
        `${baseError}. Failed to get binary descriptor for unstructured-binary parameter ${param.name}: ${descriptor.val}`,
      );
    }

    AgentConstructorParamRegistry.setType(agentClassName, param.name, {
      tag: 'unstructured-binary',
      val: descriptor.val,
      tsType: param.type,
    });

    collection.addComponentModelParameter(param.name, {
      tag: 'unstructured-binary',
      val: descriptor.val,
    });
  },
};

const analysedHandler: ConstructorParamHandler = {
  canHandle: () => true,

  handle: (agentClassName, param, baseError, collection) => {
    const typeInfoEither = WitType.fromTsType(
      param.type,
      TypeScope.constructor(agentClassName, param.name, param.type.optional),
    );

    if (Either.isLeft(typeInfoEither)) {
      throw new Error(`${baseError}. ${typeInfoEither.val}`);
    }

    const [witType, analysedType] = typeInfoEither.val;

    AgentConstructorParamRegistry.setType(agentClassName, param.name, {
      tag: 'analysed',
      val: analysedType,
      witType,
      tsType: param.type,
    });

    collection.addComponentModelParameter(param.name, {
      tag: 'component-model',
      val: witType,
    });
  },
};

const HANDLERS: readonly ConstructorParamHandler[] = [
  principalHandler,
  unstructuredTextHandler,
  unstructuredBinaryHandler,
  analysedHandler,
];

function handleConstructorParam(
  agentClassName: string,
  param: ConstructorArg,
  baseError: string,
  collection: ParameterSchemaCollection,
): void {
  const handler = HANDLERS.find((h) => h.canHandle(param));

  if (!handler) {
    throw new Error(baseError);
  }

  handler.handle(agentClassName, param, baseError, collection);
}

function buildBaseError(agentClassName: string): string {
  return `Schema generation failed for agent class ${agentClassName} due to unsupported types in constructor.`;
}

function getMultimodalTypeInfo(params: readonly ConstructorArg[]): Type.Type | undefined {
  if (params.length === 1 && isMultimodalType(params[0].type)) {
    return params[0].type;
  }
}

function resolveMultimodalConstructorSchema(
  multimodalType: Type.Type,
  paramName: string,
  agentClassName: string,
  baseError: string,
): DataSchema {
  if (multimodalType.kind !== 'array') {
    throw new Error(baseError);
  }

  const typeInfo = resolveMultimodalTypeInfo(
    multimodalType,
    multimodalType.element,
    paramName,
    baseError,
  );

  const schema = resolveMultimodalSchema(typeInfo, paramName, baseError);

  AgentConstructorParamRegistry.setType(agentClassName, paramName, typeInfo);

  return schema;
}

function resolveMultimodalTypeInfo(
  arrayType: Type.Type,
  elementType: Type.Type,
  paramName: string,
  baseError: string,
): TypeInfoInternal {
  const paramDetails = getMultimodalParamDetails(elementType);

  if (Either.isLeft(paramDetails)) {
    throw new Error(
      `${baseError}. Failed to get multimodal details for constructor parameter ${paramName}: ${paramDetails.val}`,
    );
  }

  return {
    tag: 'multimodal',
    tsType: arrayType,
    types: paramDetails.val,
  };
}

function resolveMultimodalSchema(
  typeInfo: TypeInfoInternal,
  paramName: string,
  baseError: string,
): DataSchema {
  const schemaEither = getMultimodalDataSchemaFromTypeInternal(typeInfo);

  if (Either.isLeft(schemaEither)) {
    throw new Error(
      `${baseError}. Failed to get multimodal data schema for constructor parameter ${paramName}: ${schemaEither.val}`,
    );
  }

  return schemaEither.val;
}

function resolveStandardConstructorSchema(
  agentClassName: string,
  params: readonly ConstructorArg[],
  baseError: string,
): DataSchema {
  const parameterSchemaCollection = new ParameterSchemaCollection();

  params.forEach((param) => {
    handleConstructorParam(agentClassName, param, baseError, parameterSchemaCollection);
  });

  return parameterSchemaCollection.getDataSchema();
}
