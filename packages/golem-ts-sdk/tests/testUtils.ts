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
import { AgentTypeName } from '../src/newTypes/agentTypeName';
import { AgentMethodParamRegistry } from '../src/internal/registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';

export const BarAgentClassName = new AgentClassName('BarAgent');

export const FooAgentClassName = new AgentClassName('FooAgent');

export const FooAgentName = AgentTypeName.fromAgentClassName(FooAgentClassName);

export const BarAgentName = AgentTypeName.fromString('my-complex-agent');

export function getAll() {
  return TypeMetadata.getAll();
}

export function getTestInterfaceType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('TestInterfaceType');
}

export function getTestMapType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('MapType');
}

export function getTestObjectType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('ObjectType');
}

export function getTestListOfObjectType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('ListComplexType');
}

export function getUnionType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('UnionType');
}

export function getResultTypeExact(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('ResultTypeExactBoth');
}

export function getUnionComplexType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('UnionComplexType');
}

export function getTupleType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('TupleType');
}

export function getTupleComplexType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('TupleComplexType');
}

export function getBooleanType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('boolean');
}

export function getStringType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('string');
}

export function getNumberType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('number');
}

export function getPromiseType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('PromiseType');
}

export function getUnionWithLiterals(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('UnionWithLiterals');
}

export function getUnionWithOnlyLiterals(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('UnionWithOnlyLiterals');
}

export function getRecordFieldsFromAnalysedType(
  analysedType: AnalysedType,
): NameTypePair[] | undefined {
  return analysedType.kind === 'record' ? analysedType.value.fields : undefined;
}

function fetchTypeFromBarAgent(
  typeNameInTestData: string,
): [AnalysedType, Type.Type] {
  const complexAgentMetadata = TypeMetadata.get('BarAgent');

  if (!complexAgentMetadata) {
    throw new Error('Class metadata for BarAgent not found');
  }

  const constructorArg = complexAgentMetadata.constructorArgs.find((arg) => {
    const typeName = Type.getTypeName(arg.type);
    return typeName === typeNameInTestData;
  });

  if (constructorArg) {
    const analysedType = AgentConstructorParamRegistry.lookupParamType(
      BarAgentClassName,
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
      const returnType = AgentMethodRegistry.lookupReturnType(
        BarAgentClassName,
        name,
      );

      if (!returnType || returnType.tag !== 'analysed') {
        throw new Error(
          `Return type ${returnType?.tag} not supported in test data`,
        );
      }

      return [returnType.val, method.returnType];
    }

    const param = Array.from(method.methodParams.entries()).find(([_, t]) => {
      const typeName = Type.getTypeName(t);
      return typeName === typeNameInTestData;
    });

    if (param) {
      const analysedType = AgentMethodParamRegistry.lookupParamType(
        BarAgentClassName,
        name,
        param[0],
      );

      return [analysedType!, param[1]];
    }
  }
  throw new Error(`Unresolved type ${typeNameInTestData}. Make sure \`${BarAgentClassName.value}\` use ${typeNameInTestData}`);
}
