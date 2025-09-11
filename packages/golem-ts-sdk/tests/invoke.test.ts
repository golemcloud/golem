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
import {
  getDataValueFromReturnValueWit,
  getWitValueFromDataValue,
} from '../src/decorators';
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
import { interfaceArb, unionArb } from './arbitraries';
import { ResolvedAgent } from '../src/internal/resolvedAgent';
import { DataValue } from 'golem:agent/common';
import * as Value from '../src/internal/mapping/values/Value';

test("ComplexAgent can be successfully initiated and the methods can be invoked'", () => {
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

        const arg0 = typeRegistry.constructorArgs[0].type;
        const arg1 = typeRegistry.constructorArgs[1].type;
        const arg2 = typeRegistry.constructorArgs[2].type;

        expect(arg0.optional).toEqual(false);
        expect(arg1.optional).toEqual(false);
        expect(arg2.optional).toEqual(false);

        const interfaceWit = Either.getOrThrowWith(
          WitValue.fromTsValue(interfaceValue, arg0),
          (error) =>
            new Error(`Failed to convert interface to WitValue. ${error}`),
        );

        const optionalStringWit = Either.getOrThrowWith(
          WitValue.fromTsValue(stringValue, arg1),
          (error) =>
            new Error(`Failed to convert interface to WitValue. ${error}`),
        );

        expect(Value.fromWitValue(optionalStringWit).kind).toEqual('option');

        const optionalUnionWit = Either.getOrThrowWith(
          WitValue.fromTsValue(unionValue, arg2),
          (error) => new Error(`Failed to convert union to WitValue. ${error}`),
        );


        expect(Value.fromWitValue(optionalUnionWit).kind).toEqual('option');

        const dataValue: DataValue = {
          tag: 'tuple',
          val: [
            { tag: 'component-model', val: interfaceWit },
            { tag: 'component-model', val: optionalStringWit },
            { tag: 'component-model', val: optionalUnionWit },
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

test('SimpleAgent can be successfully initiated and the methods can be invoked', () => {
  fc.assert(
    fc.property(
      fc.string(),
      fc.string(),
      fc.integer(),
      (arbData, locationValue, number) => {
        overrideSelfMetadataImpl(SimpleAgentName.value);

        const typeRegistry = TypeMetadata.get(SimpleAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('SimpleAgent type metadata not found');
        }

        const constructorInfo = typeRegistry.constructorArgs[0].type;

        const witValue = Either.getOrThrowWith(
          WitValue.fromTsValue(arbData, constructorInfo),
          (error) =>
            new Error(
              `Failed to convert constructor arg to WitValue. ${error}`,
            ),
        );

        const constructorParams = getDataValueFromReturnValueWit(witValue);

        const agentInitiator = Option.getOrThrowWith(
          AgentInitiatorRegistry.lookup(SimpleAgentName),
          () => new Error('SimpleAgent not found in AgentInitiatorRegistry'),
        );

        const result = agentInitiator.initiate(
          SimpleAgentName.value,
          constructorParams,
        );

        expect(result.tag).toEqual('ok');

        const resolvedAgent =
          result.tag === 'ok'
            ? result.val
            : (() => {
                throw new Error('Agent initiation failed');
              })();

        testInvoke(
          typeRegistry,
          'fun1',
          'location',
          locationValue,
          resolvedAgent,
          'Weather in ' + locationValue + ' is sunny!',
        );

        testInvoke(
          typeRegistry,
          'fun2',
          'data',
          {
            value: number,
            data: locationValue,
          },
          resolvedAgent,
          `Weather in ${locationValue} is sunny!`,
        );

        testInvoke(
          typeRegistry,
          'fun3',
          'param2',
          {
            data: locationValue,
            value: number,
          },
          resolvedAgent,
          `Weather in ${locationValue} is sunny!`,
        );

        testInvoke(
          typeRegistry,
          'fun4',
          'location',
          {
            data: locationValue,
            value: number,
          },
          resolvedAgent,
          undefined,
        );

        testInvoke(
          typeRegistry,
          'fun5',
          'location',
          locationValue,
          resolvedAgent,
          `Weather in ${locationValue} is sunny!`,
        );

        testInvoke(
          typeRegistry,
          'fun6',
          'location',
          locationValue,
          resolvedAgent,
          undefined,
        );
      },
    ),
  );
});

function testInvoke(
  typeRegistry: ClassMetadata,
  methodName: string,
  parameterName: string,
  arbInput: any,
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

  const parameterType = parametersInfo.get(parameterName);

  if (!parameterType) {
    throw new Error(
      'Parameter location not found in method getWeather metadata',
    );
  }

  const parameterWitValue = Either.getOrThrowWith(
    WitValue.fromTsValue(arbInput, parameterType),
    (error) => new Error('Test error ' + error),
  );

  resolvedAgent
    .invoke(methodName, getDataValueFromReturnValueWit(parameterWitValue))
    .then((invokeResult) => {
      const invokeDataValue =
        invokeResult.tag === 'ok'
          ? invokeResult.val
          : (() => {
              throw new Error('Failed to convert method arg to WitValue. ');
            })();
      const witValue = getWitValueFromDataValue(invokeDataValue)[0];
      const result = WitValue.toTsValue(witValue, returnTypeInfo);

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
