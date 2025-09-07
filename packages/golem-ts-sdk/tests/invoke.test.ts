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
  getDataValueFromWitValue,
  getWitValueFromDataValue,
} from '../src/decorators';
import * as Option from '../src/newTypes/option';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { expect, it } from 'vitest';
import * as GolemApiHostModule from 'golem:api/host@1.1.7';
import {
  AssistantAgentClassName,
  AssistantAgentName,
  WeatherAgentClassName,
  WeatherAgentName,
} from './testUtils';
import * as WitValue from '../src/internal/mapping/values/WitValue';
import * as fc from 'fast-check';
import { interfaceArb } from './arbitraries';
import { ResolvedAgent } from '../src/internal/resolvedAgent';

test("AssistantAgent can be successfully initiated and the methods can be invoked'", () => {
  fc.assert(
    fc.property(
      interfaceArb,
      fc.string(),
      (weatherAgentConstructorValue, locationValue) => {
        overrideSelfMetadataImpl();

        const typeRegistry = TypeMetadata.get(AssistantAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('WeatherAgent type metadata not found');
        }

        const constructorInfo = typeRegistry.constructorArgs[0].type;

        const witValue = Either.getOrThrowWith(
          WitValue.fromTsValue(weatherAgentConstructorValue, constructorInfo),
          (error) =>
            new Error(
              `Failed to convert constructor arg to WitValue. ${error}`,
            ),
        );

        const agentInitiator = Option.getOrThrowWith(
          AgentInitiatorRegistry.lookup(AssistantAgentName),
          () => new Error('WeatherAgent not found in AgentInitiatorRegistry'),
        );

        const result = agentInitiator.initiate(
          WeatherAgentName.value,
          getDataValueFromWitValue(witValue),
        );

        expect(result.tag).toEqual('ok');
      },
    ),
  );
});

test('WeatherAgent can be successfully initiated and the methods can be invoked', () => {
  fc.assert(
    fc.property(
      fc.string(),
      fc.string(),
      fc.integer(),
      (arbData, locationValue, number) => {
        overrideSelfMetadataImpl();

        const typeRegistry = TypeMetadata.get(WeatherAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('WeatherAgent type metadata not found');
        }

        const constructorInfo = typeRegistry.constructorArgs[0].type;

        const witValue = Either.getOrThrowWith(
          WitValue.fromTsValue(arbData, constructorInfo),
          (error) =>
            new Error(
              `Failed to convert constructor arg to WitValue. ${error}`,
            ),
        );

        const constructorParams = getDataValueFromWitValue(witValue);

        const agentInitiator = Option.getOrThrowWith(
          AgentInitiatorRegistry.lookup(WeatherAgentName),
          () => new Error('WeatherAgent not found in AgentInitiatorRegistry'),
        );

        const result = agentInitiator.initiate(
          WeatherAgentName.value,
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
          'getWeather',
          'location',
          locationValue,
          resolvedAgent,
          'Weather in ' + locationValue + ' is sunny!',
        );

        testInvoke(
          typeRegistry,
          'getWeatherV2',
          'data',
          { value: number, data: locationValue },
          resolvedAgent,
          `Weather in ${locationValue} is sunny!`,
        );

        testInvoke(
          typeRegistry,
          'getWeatherV3',
          'param2',
          { data: locationValue, value: number },
          resolvedAgent,
          `Weather in ${locationValue} is sunny!`,
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
  expectedOutput: string,
) {
  const methodSignature = typeRegistry.methods.get(methodName);
  const parametersInfo = methodSignature?.methodParams;
  const returnTypeInfo = methodSignature?.returnType;

  if (!parametersInfo) {
    throw new Error('Method getWeather not found in metadata');
  }

  if (!returnTypeInfo) {
    throw new Error('Method getWeather not found in metadata');
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
    .invoke(methodName, getDataValueFromWitValue(parameterWitValue))
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

function overrideSelfMetadataImpl() {
  vi.spyOn(GolemApiHostModule, 'getSelfMetadata').mockImplementation(() => ({
    workerId: {
      componentId: { uuid: { highBits: 42n, lowBits: 99n } },
      workerName: 'weather-agent',
    },
    args: [],
    env: [],
    wasiConfigVars: [],
    status: 'running',
    componentVersion: 0n,
    retryCount: 0n,
  }));
}
