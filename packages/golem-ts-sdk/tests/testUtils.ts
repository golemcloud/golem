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

export const AssistantAgentClassName = new AgentClassName('AssistantAgent');

export const WeatherAgentClassName = new AgentClassName('WeatherAgent');

export const WeatherAgentName = AgentTypeName.fromAgentClassName(
  WeatherAgentClassName,
);

export const AssistantAgentName = AgentTypeName.fromAgentClassName(
  AssistantAgentClassName,
);

export function getAll() {
  return TypeMetadata.getAll();
}

export function getTestInterfaceType(): Type.Type {
  return fetchType('TestInterfaceType');
}

export function getTestMapType(): Type.Type {
  return fetchType('MapType');
}

export function getTestObjectType(): Type.Type {
  return fetchType('ObjectType');
}

export function getTestListOfObjectType(): Type.Type {
  return fetchType('ListComplexType');
}

export function getUnionType(): Type.Type {
  return fetchType('UnionType');
}

export function getUnionComplexType(): Type.Type {
  return fetchType('UnionComplexType');
}

export function getTupleType(): Type.Type {
  return fetchType('TupleType');
}

export function getTupleComplexType(): Type.Type {
  return fetchType('TupleComplexType');
}

export function getBooleanType(): Type.Type {
  return fetchType('boolean');
}

export function getStringType(): Type.Type {
  return fetchType('string');
}

export function getNumberType(): Type.Type {
  return fetchType('number');
}

export function getPromiseType(): Type.Type {
  return fetchType('PromiseType');
}

export function getUnionOfLiterals(): Type.Type {
  return fetchType('UnionOfLiterals');
}

export function getRecordFieldsFromAnalysedType(
  analysedType: AnalysedType,
): NameTypePair[] | undefined {
  return analysedType.kind === 'record' ? analysedType.value.fields : undefined;
}

function fetchType(typeNameInTestData: string): Type.Type {
  const classMetadata = Array.from(getAll()).map(([_, v]) => v);

  for (const type of classMetadata) {
    const constructorArg = type.constructorArgs.find((arg) => {
      const typeName = Type.getTypeName(arg.type);
      return typeName === typeNameInTestData;
    });

    if (constructorArg) {
      return constructorArg.type;
    }

    const methods = Array.from(type.methods.values());

    for (const method of methods) {
      if (
        method.returnType &&
        Type.getTypeName(method.returnType) === typeNameInTestData
      ) {
        return method.returnType;
      }

      const param = Array.from(method.methodParams.entries()).find(([_, t]) => {
        const typeName = Type.getTypeName(t);
        return typeName === typeNameInTestData;
      });

      if (param) {
        return param[1];
      }
    }
  }

  throw new Error(`Type ${typeNameInTestData} not found in metadata`);
}
