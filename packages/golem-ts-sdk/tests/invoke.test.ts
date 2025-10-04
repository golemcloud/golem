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
import * as Option from '../src/newTypes/option';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { expect } from 'vitest';
import * as GolemApiHostModule from 'golem:api/host@1.1.7';
import {
  BarAgentClassName,
  BarAgentCustomClassName,
  FooAgentClassName,
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
  unstructuredBinaryWithMimeTypeArb,
  unstructuredTextArb,
  unstructuredTextWithLCArb,
} from './arbitraries';
import { ResolvedAgent } from '../src/internal/resolvedAgent';
import * as Value from '../src/internal/mapping/values/Value';
import { DataValue, ElementValue } from 'golem:agent/common';
import * as util from 'node:util';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from '../src/internal/registry/agentMethodParamRegistry';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';
import { deserializeDataValue } from '../src/decorators';
import { UnstructuredText } from '../src';
import { AgentClassName } from '../src';
import {
  castTsValueToBinaryReference,
  castTsValueToTextReference,
} from '../src/internal/mapping/values/serializer';
import { convertTsValueToDataValue } from '../src/internal/mapping/values/dataValue';

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
      unstructuredTextArb,
      unstructuredTextWithLCArb,
      unstructuredBinaryWithMimeTypeArb,
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
        unstructuredText,
        unstructuredTextWithLC,
        unstructuredBinaryWithMimeType,
      ) => {
        overrideSelfMetadataImpl(FooAgentClassName);

        const typeRegistry = TypeMetadata.get(FooAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('FooAgent type metadata not found');
        }

        const resolvedAgent = initiateFooAgent(arbString, typeRegistry);

        // Invoking function with string type
        testInvoke(
          'fun1',
          [['param', arbString]],
          resolvedAgent,
          'Weather in ' + arbString + ' is sunny!',
        );

        // Invoking function with multiple primitive types
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

        // Invoking function with object type
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

        // Invoking function with return type not specified
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

        // Arrow function
        testInvoke(
          'fun5',
          [['param', arbString]],
          resolvedAgent,
          `Weather in ${arbString} is sunny!`,
        );

        // Void return type
        testInvoke('fun6', [['param', arbString]], resolvedAgent, undefined);

        // Invoking with various kind of optional types embedded in union type
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

        // Invoking with union with literals
        testInvoke(
          'fun8',
          [['a', unionWithLiterals]],
          resolvedAgent,
          unionWithLiterals,
        );

        // Invoking with tagged union
        testInvoke(
          'fun9',
          [['param', taggedUnion]],
          resolvedAgent,
          taggedUnion,
        );

        // Invoking with union with only literals
        testInvoke(
          'fun10',
          [['param', unionWithOnlyLiterals]],
          resolvedAgent,
          unionWithOnlyLiterals,
        );

        // Invoking with result type
        testInvoke(
          'fun11',
          [['param', resultTypeExactBoth]],
          resolvedAgent,
          resultTypeExactBoth,
        );

        // invoking with result-like type
        testInvoke(
          'fun12',
          [['param', resultTypeNonExact]],
          resolvedAgent,
          resultTypeNonExact,
        );

        // invoking with another result-like type
        testInvoke(
          'fun13',
          [['param', resultTypeNonExact2]],
          resolvedAgent,
          resultTypeNonExact2,
        );

        // Invoking with unstructured text
        testInvoke(
          'fun15',
          [['param', unstructuredText]],
          resolvedAgent,
          unstructuredText,
        );

        // Invoking with unstructured text with language code
        testInvoke(
          'fun16',
          [['param', unstructuredTextWithLC]],
          resolvedAgent,
          unstructuredTextWithLC,
        );

        // Invoking with unstructured binary with mime type
        testInvoke(
          'fun17',
          [['param', unstructuredBinaryWithMimeType]],
          resolvedAgent,
          unstructuredBinaryWithMimeType,
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
        overrideSelfMetadataImpl(BarAgentCustomClassName);

        const typeRegistry = TypeMetadata.get(BarAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('BarAgent type metadata not found');
        }

        // TestInterfaceType
        const arg0 = AgentConstructorParamRegistry.getParamType(
          BarAgentClassName,
          typeRegistry.constructorArgs[0].name,
        );

        // string | null
        const arg1 = AgentConstructorParamRegistry.getParamType(
          BarAgentClassName,
          typeRegistry.constructorArgs[1].name,
        );

        // UnionType | null
        const arg2 = AgentConstructorParamRegistry.getParamType(
          BarAgentClassName,
          typeRegistry.constructorArgs[2].name,
        );

        if (
          !arg0 ||
          !arg1 ||
          !arg2 ||
          arg0.tag !== 'analysed' ||
          arg1.tag !== 'analysed' ||
          arg2.tag !== 'analysed'
        ) {
          throw new Error(
            'Test failure: unresolved type in BarAgent constructor',
          );
        }

        const interfaceWit = Either.getOrThrowWith(
          WitValue.fromTsValueDefault(interfaceValue, arg0.val),
          (error) => new Error(error),
        );

        const optionalStringWit = Either.getOrThrowWith(
          WitValue.fromTsValueDefault(stringValue, arg1.val),
          (error) => new Error(error),
        );

        expect(Value.fromWitValue(optionalStringWit).kind).toEqual('option');

        const optionalUnionWit = Either.getOrThrowWith(
          WitValue.fromTsValueDefault(unionValue, arg2.val),
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
          AgentInitiatorRegistry.lookup(BarAgentCustomClassName.value),
          () => new Error('BarAgent not found in AgentInitiatorRegistry'),
        );

        const result = agentInitiator.initiate(dataValue);

        expect(result.tag).toEqual('ok');
      },
    ),
  );
});

// This is already in the above big test, but we keep it separate to have a clearer
// view of how unstructured text is handled.
test('Invoke function that takes unstructured-text and returns unstructured-text', () => {
  overrideSelfMetadataImpl(FooAgentClassName);

  const typeRegistry = TypeMetadata.get(FooAgentClassName.value);

  if (!typeRegistry) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', typeRegistry);

  const validUnstructuredText: UnstructuredText<['en', 'de']> = {
    tag: 'inline',
    val: 'foo',
    languageCode: 'de',
  };

  testInvoke(
    'fun16',
    [['param', validUnstructuredText]],
    resolvedAgent,
    validUnstructuredText,
  );

  // fun16 doesn't support language code `pl`. We dynamically invoke with it to see
  // if the error is properly thrown.
  const invalidUnstructuredText: UnstructuredText<['en', 'pl']> = {
    tag: 'inline',
    val: 'foo',
    languageCode: 'pl',
  };

  const dataValue = createInputDataValue(
    [['param', invalidUnstructuredText]],
    'fun16',
  );

  resolvedAgent.invoke('fun16', dataValue).then((invokeResult) => {
    if (invokeResult.tag === 'ok') {
      throw new Error('Test failure: invocation should have failed');
    } else {
      expect(invokeResult.val.val).toContain(
        'Failed to deserialize arguments for method fun16 in agent FooAgent: Invalid value for parameter param. Language code `pl` is not allowed. Allowed codes: en, de',
      );
    }
  });
});

function initiateFooAgent(
  constructorParam: string,
  simpleAgentClassMeta: ClassMetadata,
) {
  const constructorInfo = simpleAgentClassMeta.constructorArgs[0];

  const constructorParamTypeInfoInternal =
    AgentConstructorParamRegistry.getParamType(
      FooAgentClassName,
      constructorInfo.name,
    );

  if (!constructorParamTypeInfoInternal) {
    throw new Error(
      `Test failure: unresolved type for ${constructorParam} in ${FooAgentClassName.value}`,
    );
  }

  const constructorParams = Either.getOrThrowWith(
    convertTsValueToDataValue(
      constructorParam,
      constructorParamTypeInfoInternal,
    ),
    (error) => new Error(error),
  );

  const agentInitiator = Option.getOrThrowWith(
    AgentInitiatorRegistry.lookup(FooAgentClassName.value),
    () => new Error('FooAgent not found in AgentInitiatorRegistry'),
  );

  const result = agentInitiator.initiate(constructorParams);

  if (result.tag !== 'ok') {
    throw new Error('Agent initiation failed');
  }

  return result.val;
}

function testInvoke(
  methodName: string,
  parameterNameAndValues: [string, any][],
  resolvedAgent: ResolvedAgent,
  expectedOutput: any,
) {
  // We need to first manually form the data-value to test the dynamic invoke.
  // For this, we first convert the original ts-value to data value and do a round trip to ensure
  // data matches exact.
  const dataValue = createInputDataValue(parameterNameAndValues, methodName);

  resolvedAgent.invoke(methodName, dataValue).then((invokeResult) => {
    const resultDataValue =
      invokeResult.tag === 'ok'
        ? invokeResult.val
        : (() => {
            throw new Error(util.format(invokeResult.val));
          })();

    // Unless it is an RPC call, we don't really need to deserialize the result
    // But to ensure the data-value returned above corresponds to the original input
    // we deserialize and assert if the input is same as output.
    const result = deserializeReturnValue(methodName, resultDataValue);

    expect(result).toEqual(Either.right(expectedOutput));
  });
}

function createInputDataValue(
  parameterNameAndValues: [string, any][],
  methodName: string,
): DataValue {
  const elementValues: ElementValue[] = parameterNameAndValues.map(
    ([paramName, value]) => {
      const paramAnalysedType = AgentMethodParamRegistry.getParamType(
        FooAgentClassName,
        methodName,
        paramName,
      );

      if (!paramAnalysedType) {
        throw new Error(
          `Unresolved type for \`${paramName}\` in method \`${methodName}\``,
        );
      }

      switch (paramAnalysedType.tag) {
        case 'analysed':
          const witValue = Either.getOrThrowWith(
            WitValue.fromTsValueDefault(value, paramAnalysedType.val),
            (error) => new Error(error),
          );
          return {
            tag: 'component-model',
            val: witValue,
          };

        case 'unstructured-text':
          const textReference = castTsValueToTextReference(value);
          return {
            tag: 'unstructured-text',
            val: textReference,
          };

        case 'unstructured-binary':
          const binaryReference = castTsValueToBinaryReference(value);
          return {
            tag: 'unstructured-binary',
            val: binaryReference,
          };
      }
    },
  );

  return {
    tag: 'tuple',
    val: elementValues,
  };
}

function deserializeReturnValue(
  methodName: string,
  returnValue: DataValue,
): Either.Either<any[], string> {
  const returnType = TypeMetadata.get(FooAgentClassName.value)?.methods.get(
    methodName,
  )?.returnType;

  if (!returnType) {
    throw new Error(`Method ${methodName} not found in metadata`);
  }

  const returnTypeAnalysedType = AgentMethodRegistry.getReturnType(
    FooAgentClassName,
    methodName,
  );

  if (!returnTypeAnalysedType) {
    throw new Error(`Unsupported return type for method ${methodName}`);
  }

  switch (returnTypeAnalysedType.tag) {
    case 'analysed':
      return Either.map(
        deserializeDataValue(returnValue, [
          [
            'return-value',
            {
              tag: 'analysed',
              val: returnTypeAnalysedType.val,
              tsType: returnType,
            },
          ],
        ]),
        (v) => v[0],
      );
    case 'unstructured-text':
      return Either.map(
        deserializeDataValue(returnValue, [
          [
            'return-value',
            {
              tag: 'unstructured-text',
              val: returnTypeAnalysedType.val,
              tsType: returnType,
            },
          ],
        ]),
        (v) => v[0],
      );
    case 'unstructured-binary':
      return Either.map(
        deserializeDataValue(returnValue, [
          [
            'return-value',
            {
              tag: 'unstructured-binary',
              val: returnTypeAnalysedType.val,
              tsType: returnType,
            },
          ],
        ]),
        (v) => v[0],
      );
  }
}

function overrideSelfMetadataImpl(agentClassName: AgentClassName) {
  vi.spyOn(GolemApiHostModule, 'getSelfMetadata').mockImplementation(() => ({
    workerId: {
      componentId: {
        uuid: {
          highBits: 42n,
          lowBits: 99n,
        },
      },
      workerName: agentClassName.asWit,
    },
    args: [],
    env: [],
    wasiConfigVars: [],
    status: 'running',
    componentVersion: 0n,
    retryCount: 0n,
  }));
}
