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
import { expect, it } from 'vitest';
import * as GolemApiHostModule from 'golem:api/host@1.1.7';
import {
  ComplexAgentClassName,
  ComplexAgentName,
  SimpleAgentClassName,
  SimpleAgentName,
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
  resultTypeNonExact3Arb,
  resultTypeNonExact4Arb,
  resultTypeNonExact5Arb,
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
import { TsType } from '../src/internal/mapping/types/taggedUnion';
import * as util from 'node:util';

test('SimpleAgent can be successfully initiated and all of its methods can be invoked', () => {
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
        overrideSelfMetadataImpl(SimpleAgentName.value);

        const typeRegistry = TypeMetadata.get(SimpleAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('SimpleAgent type metadata not found');
        }

        const resolvedAgent = initiateSimpleAgent(arbString, typeRegistry);

        testInvoke(
          typeRegistry,
          'fun1',
          [['param', arbString]],
          resolvedAgent,
          'Weather in ' + arbString + ' is sunny!',
        );

        testInvoke(
          typeRegistry,
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
          typeRegistry,
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
          typeRegistry,
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
          typeRegistry,
          'fun5',
          [['param', arbString]],
          resolvedAgent,
          `Weather in ${arbString} is sunny!`,
        );

        testInvoke(
          typeRegistry,
          'fun6',
          [['param', arbString]],
          resolvedAgent,
          undefined,
        );

        testInvoke(
          typeRegistry,
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
          typeRegistry,
          'fun8',
          [['a', unionWithLiterals]],
          resolvedAgent,
          unionWithLiterals,
        );

        testInvoke(
          typeRegistry,
          'fun9',
          [['param', taggedUnion]],
          resolvedAgent,
          taggedUnion,
        );

        testInvoke(
          typeRegistry,
          'fun10',
          [['param', unionWithOnlyLiterals]],
          resolvedAgent,
          unionWithOnlyLiterals,
        );

        testInvoke(
          typeRegistry,
          'fun11',
          [['param', resultTypeExactBoth]],
          resolvedAgent,
          resultTypeExactBoth,
        );

        testInvoke(
          typeRegistry,
          'fun12',
          [['param', resultTypeNonExact]],
          resolvedAgent,
          resultTypeNonExact,
        );

        testInvoke(
          typeRegistry,
          'fun13',
          [['param', resultTypeNonExact2]],
          resolvedAgent,
          resultTypeNonExact2,
        );
      },
    ),
  );
});

test('testing my thing', () => {
  overrideSelfMetadataImpl(SimpleAgentName.value);

  const typeRegistry = TypeMetadata.get(SimpleAgentClassName.value);

  if (!typeRegistry) {
    throw new Error('SimpleAgent type metadata not found');
  }

  const resolvedAgent = initiateSimpleAgent("foo", typeRegistry);

  console.log("here???");

  testInvoke(
    typeRegistry,
    'fun8',
    [['a', 'a']],
    resolvedAgent,
    'a',
  );
})

test('ComplexAgent can be successfully initiated', () => {
  fc.assert(
    fc.property(
      interfaceArb,
      fc.oneof(fc.string(), fc.constant(null)),
      fc.oneof(unionArb, fc.constant(null)),
      (interfaceValue, stringValue, unionValue) => {
        overrideSelfMetadataImpl(ComplexAgentName.value);

        const typeRegistry = TypeMetadata.get(ComplexAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('ComplexAgent type metadata not found');
        }

        // TestInterfaceType
        const arg0 = typeRegistry.constructorArgs[0].type;

        // string | null
        const arg1 = typeRegistry.constructorArgs[1].type;

        // UnionType | null
        const arg2 = typeRegistry.constructorArgs[2].type;

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
          AgentInitiatorRegistry.lookup(ComplexAgentName),
          () => new Error('ComplexAgent not found in AgentInitiatorRegistry'),
        );

        const result = agentInitiator.initiate(
          ComplexAgentName.value,
          dataValue,
        );

        expect(result.tag).toEqual('ok');
      },
    ),
  );
});

function initiateSimpleAgent(
  constructorParamString: string,
  simpleAgentClassMeta: ClassMetadata,
) {
  const constructorInfo = simpleAgentClassMeta.constructorArgs[0].type;

  const witValue = Either.getOrThrowWith(
    WitValue.fromTsValue(constructorParamString, constructorInfo),
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
    AgentInitiatorRegistry.lookup(SimpleAgentName),
    () => new Error('SimpleAgent not found in AgentInitiatorRegistry'),
  );

  const result = agentInitiator.initiate(
    SimpleAgentName.value,
    constructorParams,
  );

  if (result.tag !== 'ok') {
    throw new Error('Agent initiation failed');
  }

  return result.val;
}

function testInvoke(
  typeRegistry: ClassMetadata,
  methodName: string,
  parameterName: [string, any][],
  resolvedAgent: ResolvedAgent,
  expectedOutput: any,
) {
  const methodSignature = typeRegistry.methods.get(methodName);
  const parametersInfo = methodSignature?.methodParams;
  const returnTypeInfo = methodSignature?.returnType;

  if (!parametersInfo) {
    throw new Error(`Method ${methodName} not found in metadata`);
  }

  if (!returnTypeInfo) {
    throw new Error(`Method ${methodName} not found in metadata`);
  }

  const witValues = parameterName.map(([paramName, value]) => {
    const paramType: TsType | undefined = parametersInfo.get(paramName);

    if (!paramType) {
      throw new Error(
        `Parameter type for ${paramName} not found in method ${methodName} metadata`,
      );
    }

    const r = Either.getOrThrowWith(
      WitValue.fromTsValue(value, paramType),
      (error) => new Error(error),
    );

    const vv = Value.fromWitValue(r);
    console.log("sss " + JSON.stringify(vv));

    console.log(JSON.stringify(Value.toTsValue(vv, paramType)))

    return r;
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
      ['return-value', returnTypeInfo],
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
