// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
import { AgentClassName } from '../src';
import { AgentMethodParamRegistry } from '../src/internal/registry/agentMethodParamRegistry';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';
import {
  ResolvedField,
  ResolvedGraph,
  ResolvedType,
} from '../src/internal/mapping/types/resolvedType';

export const FooAgentClassName = new AgentClassName('FooAgent');
export const BarAgentClassName = new AgentClassName('BarAgent');
export const BarAgentCustomClassName = new AgentClassName('my-complex-agent');
export const EphemeralAgentClassName = new AgentClassName('EphemeralAgent');
export const SimpleHttpAgentClassName = new AgentClassName('SimpleHttpAgent');
export const ComplexHttpAgentClassName = new AgentClassName('ComplexHttpAgent');
export const AllHttpMethodsAgentClassName = new AgentClassName('AllHttpMethodsAgent');
export const SnapshottingDisabledAgentClassName = new AgentClassName('SnapshottingDisabledAgent');
export const SnapshottingEnabledAgentClassName = new AgentClassName('SnapshottingEnabledAgent');
export const SnapshottingPeriodicAgentClassName = new AgentClassName('SnapshottingPeriodicAgent');
export const SnapshottingEveryNAgentClassName = new AgentClassName('SnapshottingEveryNAgent');
export const ConstructorUnionOrderAgentClassName = new AgentClassName('ConstructorUnionOrderAgent');
export const ReadOnlyAgentClassName = new AgentClassName('ReadOnlyAgent');

/**
 * The pair returned by the `getXxxType()` helpers: the schema-native
 * {@link ResolvedGraph} produced by the production mapper (as stored in the
 * registry by the `@agent` decorator) plus the original reflected `Type.Type`.
 */
export type TypePair = [ResolvedGraph, Type.Type];

export function getTestInterfaceType(): TypePair {
  return fetchTypeFromBarAgent('TestInterfaceType');
}

export function getTestMapType(): TypePair {
  return fetchTypeFromBarAgent('MapType');
}

export function getTestObjectType(): TypePair {
  return fetchTypeFromBarAgent('ObjectType');
}

export function getTestListOfObjectType(): TypePair {
  return fetchTypeFromBarAgent('ListComplexType');
}

export function getUnionType(): TypePair {
  return fetchTypeFromBarAgent('UnionType');
}

export function getResultTypeExact(): TypePair {
  return fetchTypeFromBarAgent('ResultTypeExactBoth');
}

export function getUnionComplexType(): TypePair {
  return fetchTypeFromBarAgent('UnionComplexType');
}

export function getTupleType(): TypePair {
  return fetchTypeFromBarAgent('TupleType');
}

export function getTupleComplexType(): TypePair {
  return fetchTypeFromBarAgent('TupleComplexType');
}

export function getBooleanType(): TypePair {
  return fetchTypeFromBarAgent('boolean');
}

export function getStringType(): TypePair {
  return fetchTypeFromBarAgent('string');
}

export function getNumberType(): TypePair {
  return fetchTypeFromBarAgent('number');
}

export function getPromiseType(): TypePair {
  return fetchTypeFromBarAgent('PromiseType');
}

export function getUnionWithLiterals(): TypePair {
  return fetchTypeFromBarAgent('UnionWithLiterals');
}

export function getUnionWithBooleanInMiddle(): TypePair {
  return fetchTypeFromBarAgent('UnionWithBooleanInMiddle');
}

export function getImportedSourceOrderedUnion(): TypePair {
  return fetchTypeFromBarAgent('ImportedSourceOrderedUnion');
}

export function getObjectOrBooleanOrUndefined(): TypePair {
  return fetchTypeFromBarAgent('ObjectOrBooleanOrUndefined');
}

export function getUnionWithOnlyLiterals(): TypePair {
  return fetchTypeFromBarAgent('UnionWithOnlyLiterals');
}

/** Resolve a graph's root to its underlying composite, following a top-level `ref`. */
export function resolveRoot(graph: ResolvedGraph): ResolvedType {
  const root = graph.root;
  if (root.body.tag === 'ref') {
    const def = graph.defs.get(root.body.id);
    if (!def) throw new Error(`Test failure: unresolved ref ${root.body.id}`);
    return def;
  }
  return root;
}

/** The record fields of a graph whose (possibly ref'd) root is a record. */
export function getRecordFields(graph: ResolvedGraph): ResolvedField[] | undefined {
  const root = resolveRoot(graph);
  return root.body.tag === 'record' ? root.body.fields : undefined;
}

/**
 * Build a sub-graph rooted at a named field of a record graph, preserving the
 * original `defs` so any `ref`s in the field type still resolve.
 */
export function pickField(graph: ResolvedGraph, fieldName: string): ResolvedGraph {
  const fields = getRecordFields(graph);
  if (!fields) throw new Error('Test failure: expected a record graph');
  const field = fields.find((f) => f.name === fieldName);
  if (!field) throw new Error(`Test failure: missing field ${fieldName}`);
  return { defs: graph.defs, root: field.type };
}

export function fetchTypeFromBarAgent(typeNameInTestData: string): TypePair {
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

    if (!typeInfo || typeInfo.tag !== 'schema') {
      throw new Error(
        `Test failure: Unsupported type for constructor parameter ${constructorArg.name}`,
      );
    }

    return [typeInfo.graph, constructorArg.type];
  }

  const methods = Array.from(complexAgentMetadata.methods);

  for (const [name, method] of methods) {
    if (method.returnType && Type.getTypeName(method.returnType) === typeNameInTestData) {
      const returnType = AgentMethodRegistry.getReturnType('BarAgent', name);

      if (!returnType || returnType.tag !== 'single' || returnType.type.tag !== 'schema') {
        throw new Error(`Return type ${returnType?.tag} not supported in test data`);
      }

      return [returnType.type.graph, method.returnType];
    }

    const param = Array.from(method.methodParams.entries()).find(([_, t]) => {
      const typeName = Type.getTypeName(t);
      return typeName === typeNameInTestData;
    });

    if (param) {
      const typeInfo = AgentMethodParamRegistry.getParamType('BarAgent', name, param[0]);

      if (!typeInfo || typeInfo.tag !== 'schema') {
        throw new Error(
          `Test failure: Unsupported type for parameter ${param[0]} in method ${name}`,
        );
      }

      return [typeInfo.graph, param[1]];
    }
  }
  throw new Error(
    `Test failure: Unresolved type ${typeNameInTestData}. Make sure \`${BarAgentClassName.value}\` use ${typeNameInTestData}`,
  );
}
