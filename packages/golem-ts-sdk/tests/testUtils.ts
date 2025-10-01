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

import { Type, TypeMetadata } from '@golemcloud/golem-ts-types-core';
import {
  AnalysedType,
  NameTypePair,
} from '../src/internal/mapping/types/AnalysedType';
import { AgentClassName } from '../src';
import { AgentMethodParamRegistry } from '../src/internal/registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';

export const ComplexAgentClassName = new AgentClassName('ComplexAgent');

export const CustomComplexAgentTypeName = new AgentClassName(
  'my-complex-agent',
);

export const SimpleAgentClassName = new AgentClassName('SimpleAgent');

export function getAll() {
  return TypeMetadata.getAll();
}

export function getTestInterfaceType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('TestInterfaceType');
}

export function getTestMapType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('MapType');
}

export function getTestObjectType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('ObjectType');
}

export function getTestListOfObjectType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('ListComplexType');
}

export function getUnionType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('UnionType');
}

export function getResultTypeExact(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('ResultTypeExactBoth');
}

export function getUnionComplexType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('UnionComplexType');
}

export function getTupleType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('TupleType');
}

export function getTupleComplexType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('TupleComplexType');
}

export function getBooleanType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('boolean');
}

export function getStringType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('string');
}

export function getNumberType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('number');
}

export function getPromiseType(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('PromiseType');
}

export function getUnionWithLiterals(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('UnionWithLiterals');
}

export function getUnionWithOnlyLiterals(): [AnalysedType, Type.Type] {
  return fetchTypeFromComplexAgent('UnionWithOnlyLiterals');
}

export function getRecordFieldsFromAnalysedType(
  analysedType: AnalysedType,
): NameTypePair[] | undefined {
  return analysedType.kind === 'record' ? analysedType.value.fields : undefined;
}

function fetchTypeFromComplexAgent(
  typeNameInTestData: string,
): [AnalysedType, Type.Type] {
  const complexAgentMetadata = TypeMetadata.get('ComplexAgent');

  if (!complexAgentMetadata) {
    throw new Error('Class metadata for ComplexAgent not found');
  }

  const constructorArg = complexAgentMetadata.constructorArgs.find((arg) => {
    const typeName = Type.getTypeName(arg.type);
    return typeName === typeNameInTestData;
  });

  if (constructorArg) {
    const analysedType = AgentConstructorParamRegistry.lookupParamType(
      ComplexAgentClassName,
      constructorArg.name,
    );

    return [analysedType!, constructorArg.type];
  }

  const methods = Array.from(complexAgentMetadata.methods);

  for (const [name, method] of methods) {
    if (
      method.returnType &&
      Type.getTypeName(method.returnType) === typeNameInTestData
    ) {
      const analysedType = AgentMethodRegistry.lookupReturnType(
        ComplexAgentClassName,
        name,
      );

      return [analysedType!, method.returnType];
    }

    const param = Array.from(method.methodParams.entries()).find(([_, t]) => {
      const typeName = Type.getTypeName(t);
      return typeName === typeNameInTestData;
    });

    if (param) {
      const analysedType = AgentMethodParamRegistry.lookupParamType(
        ComplexAgentClassName,
        name,
        param[0],
      );

      return [analysedType!, param[1]];
    }
  }
  throw new Error(`Type ${typeNameInTestData} not found in metadata`);
}
