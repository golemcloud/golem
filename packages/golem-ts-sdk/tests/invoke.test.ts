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

import { ClassMetadata, TypeMetadata } from '@golemcloud/golem-ts-types-core';
import * as Either from '../src/newTypes/either';
import { deserializeDataValue } from '../src/decorators';
import * as Option from '../src/newTypes/option';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { expect } from 'vitest';
import * as GolemApiHostModule from 'golem:api/host@1.1.7';
import {
  BarAgentClassName,
  BarAgentName,
  FooAgentClassName,
  FooAgentName,
} from './testUtils';
import * as WitValue from '../src/internal/mapping/values/WitValue';
import * as fc from 'fast-check';
import {
  interfaceArb,
  objectWithUnionWithUndefined1Arb,
  objectWithUnionWithUndefined2Arb,
  objectWithUnionWithUndefined3Arb,
  objectWithUnionWithUndefined4Arb,
  resultTypeExactArb,
  resultTypeNonExact2Arb,
  resultTypeNonExactArb,
  stringOrNumberOrNull,
  stringOrUndefined,
  taggedUnionArb,
  unionArb,
  unionWithLiteralArb,
  unionWithOnlyLiteralsArb,
} from './arbitraries';
import { ResolvedAgent } from '../src/internal/resolvedAgent';
import * as Value from '../src/internal/mapping/values/Value';
import { DataValue } from 'golem:agent/common';
import * as util from 'node:util';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from '../src/internal/registry/agentMethodParamRegistry';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';

test('An agent can be successfully initiated and all of its methods can be invoked', () => {
  fc.assert(
    fc.property(
      fc.string(),
      fc.oneof(fc.oneof(fc.integer(), fc.float())),
      stringOrNumberOrNull,
      objectWithUnionWithUndefined1Arb,
      objectWithUnionWithUndefined2Arb,
      objectWithUnionWithUndefined3Arb,
      objectWithUnionWithUndefined4Arb,
      stringOrUndefined,
      fc.oneof(unionArb, fc.constant(undefined)),
      unionWithLiteralArb,
      taggedUnionArb,
      unionWithOnlyLiteralsArb,
      resultTypeExactArb,
      resultTypeNonExactArb,
      resultTypeNonExact2Arb,
      (
        arbString,
        number,
        stringOrNumberOrNull,
        objectWithUnionWithUndefined1,
        objectWithUnionWithUndefined2,
        objectWithUnionWithUndefined3,
        objectWithUnionWithUndefined4,
        stringOrUndefined,
        unionOrUndefined,
        unionWithLiterals,
        taggedUnion,
        unionWithOnlyLiterals,
        resultTypeExactBoth,
        resultTypeNonExact,
        resultTypeNonExact2,
      ) => {
        overrideSelfMetadataImpl(FooAgentName.value);

        const typeRegistry = TypeMetadata.get(FooAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('FooAgent type metadata not found');
        }

        const resolvedAgent = initiateFooAgent(arbString, typeRegistry);

        testInvoke(
          'fun1',
          [['param', arbString]],
          resolvedAgent,
          'Weather in ' + arbString + ' is sunny!',
        );

        testInvoke(
          'fun2',
          [
            [
              'param',
              {
                value: number,
                data: arbString,
              },
            ],
          ],
          resolvedAgent,
          `Weather in ${arbString} is sunny!`,
        );

        testInvoke(
          'fun3',
          [
            [
              'param',
              {
                data: arbString,
                value: number,
              },
            ],
          ],
          resolvedAgent,
          `Weather in ${arbString} is sunny!`,
        );

        testInvoke(
          'fun4',
          [
            [
              'param',
              {
                data: arbString,
                value: number,
              },
            ],
          ],
          resolvedAgent,
          undefined,
        );

        testInvoke(
          'fun5',
          [['param', arbString]],
          resolvedAgent,
          `Weather in ${arbString} is sunny!`,
        );

        testInvoke('fun6', [['param', arbString]], resolvedAgent, undefined);

        testInvoke(
          'fun7',
          [
            ['param1', stringOrNumberOrNull],
            ['param2', objectWithUnionWithUndefined1],
            ['param3', objectWithUnionWithUndefined2],
            ['param4', objectWithUnionWithUndefined3],
            ['param5', objectWithUnionWithUndefined4],
            ['param6', stringOrUndefined],
            ['param7', unionOrUndefined],
          ],
          resolvedAgent,
          {
            param1: stringOrNumberOrNull,
            param2: objectWithUnionWithUndefined1.a,
            param3: objectWithUnionWithUndefined2.a,
            param4: objectWithUnionWithUndefined3.a,
            param5: objectWithUnionWithUndefined4.a,
            param6: stringOrUndefined,
            param7: unionOrUndefined,
          },
        );

        testInvoke(
          'fun8',
          [['a', unionWithLiterals]],
          resolvedAgent,
          unionWithLiterals,
        );

        testInvoke(
          'fun9',
          [['param', taggedUnion]],
          resolvedAgent,
          taggedUnion,
        );

        testInvoke(
          'fun10',
          [['param', unionWithOnlyLiterals]],
          resolvedAgent,
          unionWithOnlyLiterals,
        );

        testInvoke(
          'fun11',
          [['param', resultTypeExactBoth]],
          resolvedAgent,
          resultTypeExactBoth,
        );

        testInvoke(
          'fun12',
          [['param', resultTypeNonExact]],
          resolvedAgent,
          resultTypeNonExact,
        );

        testInvoke(
          'fun13',
          [['param', resultTypeNonExact2]],
          resolvedAgent,
          resultTypeNonExact2,
        );
      },
    ),
  );
});

test('BarAgent can be successfully initiated', () => {
  fc.assert(
    fc.property(
      interfaceArb,
      fc.oneof(fc.string(), fc.constant(null)),
      fc.oneof(unionArb, fc.constant(null)),
      (interfaceValue, stringValue, unionValue) => {
        overrideSelfMetadataImpl(BarAgentName.value);

        const typeRegistry = TypeMetadata.get(BarAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('BarAgent type metadata not found');
        }

        // TestInterfaceType
        const arg0 = AgentConstructorParamRegistry.lookupParamType(
          BarAgentClassName,
          typeRegistry.constructorArgs[0].name,
        );

        // string | null
        const arg1 = AgentConstructorParamRegistry.lookupParamType(
          BarAgentClassName,
          typeRegistry.constructorArgs[1].name,
        );

        // UnionType | null
        const arg2 = AgentConstructorParamRegistry.lookupParamType(
          BarAgentClassName,
          typeRegistry.constructorArgs[2].name,
        );

        if (!arg0 || !arg1 || !arg2) {
          throw new Error('Test error: constructor params not found');
        }

        const interfaceWit = Either.getOrThrowWith(
          WitValue.fromTsValue(interfaceValue, arg0),
          (error) => new Error(error),
        );

        const optionalStringWit = Either.getOrThrowWith(
          WitValue.fromTsValue(stringValue, arg1),
          (error) => new Error(error),
        );

        expect(Value.fromWitValue(optionalStringWit).kind).toEqual('option');

        const optionalUnionWit = Either.getOrThrowWith(
          WitValue.fromTsValue(unionValue, arg2),
          (error) => new Error(error),
        );

        expect(Value.fromWitValue(optionalUnionWit).kind).toEqual('option');

        const dataValue: DataValue = {
          tag: 'tuple',
          val: [
            {
              tag: 'component-model',
              val: interfaceWit,
            },
            {
              tag: 'component-model',
              val: optionalStringWit,
            },
            {
              tag: 'component-model',
              val: optionalUnionWit,
            },
          ],
        };

        const agentInitiator = Option.getOrThrowWith(
          AgentInitiatorRegistry.lookup(BarAgentName),
          () => new Error('BarAgent not found in AgentInitiatorRegistry'),
        );

        const result = agentInitiator.initiate(BarAgentName.value, dataValue);

        expect(result.tag).toEqual('ok');
      },
    ),
  );
});

function initiateFooAgent(
  constructorParamString: string,
  simpleAgentClassMeta: ClassMetadata,
) {
  const constructorInfo = simpleAgentClassMeta.constructorArgs[0];

  const constructorParamAnalysedType =
    AgentConstructorParamRegistry.lookupParamType(
      FooAgentClassName,
      constructorInfo.name,
    );

  if (!constructorParamAnalysedType) {
    throw new Error(
      `Constructor parameter type for FooAgent constructor parameter ${constructorInfo.name} not found in metadata.`,
    );
  }

  const witValue = Either.getOrThrowWith(
    WitValue.fromTsValue(constructorParamString, constructorParamAnalysedType),
    (error) => new Error(error),
  );

  const constructorParams: DataValue = {
    tag: 'tuple',
    val: [
      {
        tag: 'component-model',
        val: witValue,
      },
    ],
  };

  const agentInitiator = Option.getOrThrowWith(
    AgentInitiatorRegistry.lookup(FooAgentName),
    () => new Error('FooAgent not found in AgentInitiatorRegistry'),
  );

  const result = agentInitiator.initiate(FooAgentName.value, constructorParams);

  if (result.tag !== 'ok') {
    throw new Error('Agent initiation failed');
  }

  return result.val;
}

function testInvoke(
  methodName: string,
  parameterAndValue: [string, any][],
  resolvedAgent: ResolvedAgent,
  expectedOutput: any,
) {


  const returnType = TypeMetadata.get(FooAgentClassName.value)?.methods.get(
    methodName,
  )?.returnType;

  if (!returnType) {
    throw new Error(`Method ${methodName} not found in metadata`);
  }

  const returnTypeAnalysedType = AgentMethodRegistry.lookupReturnType(
    FooAgentClassName,
    methodName,
  );

  if (!returnTypeAnalysedType || returnTypeAnalysedType.tag !== 'analysed') {
    throw new Error(`Unsupported return type for method ${methodName}`);
  }

  const witValues = parameterAndValue.map(([paramName, value]) => {
    const paramAnalysedType = AgentMethodParamRegistry.lookupParamType(
      FooAgentClassName,
      methodName,
      paramName,
    );

    if (!paramAnalysedType) {
      throw new Error(
        `Parameter type for parameter ${paramName} of method ${methodName} not found in metadata.`,
      );
    }

    return Either.getOrThrowWith(
      WitValue.fromTsValue(value, paramAnalysedType),
      (error) => new Error(error),
    );
  });

  const dataValues: DataValue = {
    tag: 'tuple',
    val: witValues.map((witValue) => ({
      tag: 'component-model',
      val: witValue,
    })),
  };

  resolvedAgent.invoke(methodName, dataValues).then((invokeResult) => {
    const resultDataValue =
      invokeResult.tag === 'ok'
        ? invokeResult.val
        : (() => {
            throw new Error(util.format(invokeResult.val));
          })();

    const result = deserializeDataValue(resultDataValue, [
      ['return-value', [returnType, Option.some(returnTypeAnalysedType.val)]],
    ])[0];

    expect(result).toEqual(expectedOutput);
  });
}

function overrideSelfMetadataImpl(agentName: string) {
  vi.spyOn(GolemApiHostModule, 'getSelfMetadata').mockImplementation(() => ({
    workerId: {
      componentId: {
        uuid: {
          highBits: 42n,
          lowBits: 99n,
        },
      },
      workerName: agentName,
    },
    args: [],
    env: [],
    wasiConfigVars: [],
    status: 'running',
    componentVersion: 0n,
    retryCount: 0n,
  }));
}
