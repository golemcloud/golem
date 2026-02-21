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
import { AnalysedType, NameTypePair } from '../src/internal/mapping/types/analysedType';
import { AgentClassName } from '../src';
import { AgentMethodParamRegistry } from '../src/internal/registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';

export const FooAgentClassName = new AgentClassName('FooAgent');
export const BarAgentClassName = new AgentClassName('BarAgent');
export const BarAgentCustomClassName = new AgentClassName('my-complex-agent');
export const EphemeralAgentClassName = new AgentClassName('EphemeralAgent');
export const SimpleHttpAgentClassName = new AgentClassName('SimpleHttpAgent');
export const ComplexHttpAgentClassName = new AgentClassName('ComplexHttpAgent');

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getTestInterfaceType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('TestInterfaceType');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getTestMapType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('MapType');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getTestObjectType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('ObjectType');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getTestListOfObjectType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('ListComplexType');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getUnionType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('UnionType');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getResultTypeExact(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('ResultTypeExactBoth');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getUnionComplexType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('UnionComplexType');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getTupleType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('TupleType');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getTupleComplexType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('TupleComplexType');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getBooleanType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('boolean');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getStringType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('string');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getNumberType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('number');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getPromiseType(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('PromiseType');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getUnionWithLiterals(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('UnionWithLiterals');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getUnionWithOnlyLiterals(): [AnalysedType, Type.Type] {
  return fetchTypeFromBarAgent('UnionWithOnlyLiterals');
}

// Fetch the analysed type set in the global registry, and the original (ts-morph-lite) `Type` from BarAgent metadata
export function getRecordFieldsFromAnalysedType(
  analysedType: AnalysedType,
): NameTypePair[] | undefined {
  return analysedType.kind === 'record' ? analysedType.value.fields : undefined;
}

export function fetchTypeFromBarAgent(typeNameInTestData: string): [AnalysedType, Type.Type] {
  const complexAgentMetadata = TypeMetadata.get(BarAgentClassName.value);

  if (!complexAgentMetadata) {
    throw new Error('Class metadata for BarAgent not found');
  }

  const constructorArg = complexAgentMetadata.constructorArgs.find((arg) => {
    const typeName = Type.getTypeName(arg.type);
    return typeName === typeNameInTestData;
  });

  if (constructorArg) {
    const typeInfo = AgentConstructorParamRegistry.getParamType('BarAgent', constructorArg.name);

    if (!typeInfo || typeInfo.tag !== 'analysed') {
      throw new Error(
        `Test failure: Unsupported type for constructor parameter ${constructorArg.name}`,
      );
    }

    return [typeInfo.val, constructorArg.type];
  }

  const methods = Array.from(complexAgentMetadata.methods);

  for (const [name, method] of methods) {
    if (method.returnType && Type.getTypeName(method.returnType) === typeNameInTestData) {
      const returnType = AgentMethodRegistry.getReturnType('BarAgent', name);

      if (!returnType || returnType.tag !== 'analysed') {
        throw new Error(`Return type ${returnType?.tag} not supported in test data`);
      }

      return [returnType.val, method.returnType];
    }

    const param = Array.from(method.methodParams.entries()).find(([_, t]) => {
      const typeName = Type.getTypeName(t);
      return typeName === typeNameInTestData;
    });

    if (param) {
      const typeInfo = AgentMethodParamRegistry.getParamType('BarAgent', name, param[0]);

      if (!typeInfo || typeInfo.tag !== 'analysed') {
        throw new Error(
          `Test failure: Unsupported type for parameter ${param[0]} in method ${name}`,
        );
      }

      return [typeInfo.val, param[1]];
    }
  }
  throw new Error(
    `Test failure: Unresolved type ${typeNameInTestData}. Make sure \`${BarAgentClassName.value}\` use ${typeNameInTestData}`,
  );
}
