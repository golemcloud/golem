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
  isMultimodalType,
} from './helpers';
import {
  getMultimodalDataSchemaFromTypeInternal,
  TypeInfoInternal,
} from '../typeInfoInternal';
import { AgentConstructorParamRegistry } from '../registry/agentConstructorParamRegistry';
import { TypeMappingScope } from '../mapping/types/scope';
import { ParameterSchemaCollection } from './paramSchema';

export function getAgentConstructorSchema(
  agentClassName: string,
  classType: ClassMetadata,
): DataSchema {
  const constructorParams = classType.constructorArgs;
  const baseError = buildBaseError(agentClassName);

  const multimodal: Type.Type | undefined =
    getSingleMultimodalConstructorParam(constructorParams);

  if (multimodal) {
    return resolveMultimodalConstructorSchema(
      multimodal,
      constructorParams[0].name,
      agentClassName,
      baseError,
    );
  }

  return resolveStandardConstructorSchema(
    agentClassName,
    constructorParams,
    baseError,
  );
}

function buildBaseError(agentClassName: string): string {
  return `Schema generation failed for agent class ${agentClassName} due to unsupported types in constructor.`;
}

function getSingleMultimodalConstructorParam(
  params: readonly ConstructorArg[],
): Type.Type | undefined {
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
  const collection = new ParameterSchemaCollection();

  params.forEach((param) => {
    handleConstructorParam(agentClassName, param, baseError, collection);
  });

  return collection.getDataSchema();
}

function handleConstructorParam(
  agentClassName: string,
  param: ConstructorArg,
  baseError: string,
  collection: ParameterSchemaCollection,
): void {
  if (tryHandlePrincipal(agentClassName, param, collection)) return;
  if (tryHandleUnstructuredText(agentClassName, param, baseError, collection))
    return;
  if (tryHandleUnstructuredBinary(agentClassName, param, baseError, collection))
    return;

  handleAnalysedType(agentClassName, param, baseError, collection);
}

function tryHandlePrincipal(
  agentClassName: string,
  param: ConstructorArg,
  collection: ParameterSchemaCollection,
): boolean {
  if (param.type.name !== 'Principal') return false;

  AgentConstructorParamRegistry.setType(agentClassName, param.name, {
    tag: 'principal',
    tsType: param.type,
  });

  collection.addPrincipalParameter(param.name);
  return true;
}

function tryHandleUnstructuredText(
  agentClassName: string,
  param: ConstructorArg,
  baseError: string,
  collection: ParameterSchemaCollection,
): boolean {
  if (param.type.name !== 'UnstructuredText') return false;

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

  return true;
}

function tryHandleUnstructuredBinary(
  agentClassName: string,
  param: ConstructorArg,
  baseError: string,
  collection: ParameterSchemaCollection,
): boolean {
  if (param.type.name !== 'UnstructuredBinary') return false;

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

  return true;
}

function handleAnalysedType(
  agentClassName: string,
  param: ConstructorArg,
  baseError: string,
  collection: ParameterSchemaCollection,
): void {
  const typeInfoEither = WitType.fromTsType(
    param.type,
    Option.some(
      TypeMappingScope.constructor(
        agentClassName,
        param.name,
        param.type.optional,
      ),
    ),
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
}
