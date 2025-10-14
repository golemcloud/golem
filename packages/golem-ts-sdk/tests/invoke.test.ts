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
  float32ArrayArb,
  float64ArrayArb,
  int16ArrayArb,
  int32ArrayArb,
  int64ArrayArb,
  int8ArrayArb,
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
  uint16ArrayArb,
  uint32ArrayArb,
  uint64ArrayArb,
  uint8ArrayArb,
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
import { Multimodal, Result, UnstructuredText } from '../src';
import { AgentClassName } from '../src';
import {
  castTsValueToBinaryReference,
  castTsValueToTextReference,
} from '../src/internal/mapping/values/serializer';
import {
  deserializeDataValue,
  serializeToDataValue,
} from '../src/internal/mapping/values/dataValue';
import { Image, Text } from './sampleAgents';

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

        const classMetadata = TypeMetadata.get(FooAgentClassName.value);

        if (!classMetadata) {
          throw new Error('FooAgent type metadata not found');
        }

        const resolvedAgent = initiateFooAgent(arbString, classMetadata);

        // Invoking function with string type
        testInvoke(
          'fun1',
          [['param', arbString]],
          resolvedAgent,
          'Weather in ' + arbString + ' is sunny!',
          false,
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
          false,
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
          false,
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
          false,
        );

        // Arrow function
        testInvoke(
          'fun5',
          [['param', arbString]],
          resolvedAgent,
          `Weather in ${arbString} is sunny!`,
          false,
        );

        // Void return type
        testInvoke(
          'fun6',
          [['param', arbString]],
          resolvedAgent,
          undefined,
          false,
        );

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
          false,
        );

        // Invoking with union with literals
        testInvoke(
          'fun8',
          [['a', unionWithLiterals]],
          resolvedAgent,
          unionWithLiterals,
          false,
        );

        // Invoking with tagged union
        testInvoke(
          'fun9',
          [['param', taggedUnion]],
          resolvedAgent,
          taggedUnion,
          false,
        );

        // Invoking with union with only literals
        testInvoke(
          'fun10',
          [['param', unionWithOnlyLiterals]],
          resolvedAgent,
          unionWithOnlyLiterals,
          false,
        );

        // Invoking with result type
        testInvoke(
          'fun11',
          [['param', resultTypeExactBoth]],
          resolvedAgent,
          resultTypeExactBoth,
          false,
        );

        // invoking with result-like type
        testInvoke(
          'fun12',
          [['param', resultTypeNonExact]],
          resolvedAgent,
          resultTypeNonExact,
          false,
        );

        // invoking with another result-like type
        testInvoke(
          'fun13',
          [['param', resultTypeNonExact2]],
          resolvedAgent,
          resultTypeNonExact2,
          false,
        );

        // Invoking with unstructured text
        testInvoke(
          'fun15',
          [['param', unstructuredText]],
          resolvedAgent,
          unstructuredText,
          false,
        );

        // Invoking with unstructured text with language code
        testInvoke(
          'fun16',
          [['param', unstructuredTextWithLC]],
          resolvedAgent,
          unstructuredTextWithLC,
          false,
        );

        // Invoking with unstructured binary with mime type
        testInvoke(
          'fun17',
          [['param', unstructuredBinaryWithMimeType]],
          resolvedAgent,
          unstructuredBinaryWithMimeType,
          false,
        );
      },
    ),
  );
});

test('Invoke function that takes and returns inbuilt result type', () => {
  overrideSelfMetadataImpl(FooAgentClassName);
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  testInvoke(
    'fun30',
    [['param', Result.err('message')]],
    resolvedAgent,
    Result.err('message'),
    false,
  );

  testInvoke(
    'fun30',
    [['param', Result.ok(true)]],
    resolvedAgent,
    Result.ok(true),
    false,
  );

  testInvoke(
    'fun31',
    [['param', Result.ok(true)]],
    resolvedAgent,
    Result.ok(true),
    false,
  );
});

test('Invoke function that takes and returns multimodal types', () => {
  overrideSelfMetadataImpl(FooAgentClassName);

  overrideSelfMetadataImpl(FooAgentClassName);

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  const multimodalInput: Multimodal<Text | Image> = [
    'my-string-input',
    new Uint8Array([137, 80, 78, 71]),
  ];

  fc.assert(
    fc.property(
      fc.string(),
      fc.uint8Array({ minLength: 1, maxLength: 10 }),
      (text, imageData) => {
        const multimodalInput: Multimodal<Text | Image> = [text, imageData];

        testInvoke(
          'fun18',
          [['param', multimodalInput]],
          resolvedAgent,
          multimodalInput,
          true,
        );
      },
    ),
  );

  testInvoke(
    'fun18',
    [['param', multimodalInput]],
    resolvedAgent,
    multimodalInput,
    true,
  );
});

test('Invoke function that takes and returns typed array', () => {
  overrideSelfMetadataImpl(FooAgentClassName);

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  fc.assert(
    fc.property(
      uint8ArrayArb,
      uint16ArrayArb,
      uint32ArrayArb,
      uint64ArrayArb,
      int8ArrayArb,
      int16ArrayArb,
      int32ArrayArb,
      int64ArrayArb,
      float32ArrayArb,
      float64ArrayArb,
      (
        uint8,
        uint16,
        uint32,
        uint64,
        int8,
        int16,
        int32,
        int64,
        float32,
        float64,
      ) => {
        testInvoke('fun19', [['param', uint8]], resolvedAgent, uint8, false);

        testInvoke('fun20', [['param', uint16]], resolvedAgent, uint16, false);

        testInvoke('fun27', [['param', uint32]], resolvedAgent, uint32, false);

        testInvoke('fun23', [['param', uint64]], resolvedAgent, uint64, false);

        testInvoke('fun24', [['param', int8]], resolvedAgent, int8, false);

        testInvoke('fun25', [['param', int16]], resolvedAgent, int16, false);

        testInvoke('fun26', [['param', int32]], resolvedAgent, int32, false);

        testInvoke('fun29', [['param', int64]], resolvedAgent, int64, false);

        testInvoke(
          'fun21',
          [['param', float32]],
          resolvedAgent,
          float32,
          false,
        );

        testInvoke(
          'fun28',
          [['param', float64]],
          resolvedAgent,
          float64,
          false,
        );
      },
    ),
  );
});

// This is already in the above big test, but we keep it separate to have a clearer
// view of how unstructured text is handled.
test('Invoke function that takes unstructured-text and returns unstructured-text', () => {
  overrideSelfMetadataImpl(FooAgentClassName);

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

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
    false,
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
    false,
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
    serializeToDataValue(constructorParam, constructorParamTypeInfoInternal),
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
  multimodal: boolean,
) {
  // We need to first manually form the data-value to test the dynamic invoke.
  // For this, we first convert the original ts-value to data value and do a round trip to ensure
  // data matches exact.
  const dataValue = createInputDataValue(
    parameterNameAndValues,
    methodName,
    multimodal,
  );

  resolvedAgent.invoke(methodName, dataValue).then((invokeResult) => {
    const resultDataValue =
      invokeResult.tag === 'ok'
        ? invokeResult.val
        : (() => {
            throw new Error('Test failure: ' + util.format(invokeResult.val));
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
  multimodal: boolean,
): DataValue {
  if (multimodal) {
    expect(parameterNameAndValues.length).toBe(1);

    const [paramName, value] = parameterNameAndValues[0];
    const paramAnalysedType = AgentMethodParamRegistry.getParamType(
      FooAgentClassName,
      methodName,
      paramName,
    );

    if (!paramAnalysedType) {
      throw new Error(
        `Unresolved multimodal type for  \`${paramName}\` in method \`${methodName}\``,
      );
    }

    if (paramAnalysedType.tag !== 'multimodal') {
      throw new Error(
        `Test failure: expected multimodal type for parameter \`${paramName}\` in method \`${methodName}\``,
      );
    }

    return Either.getOrThrowWith(
      serializeToDataValue(value, paramAnalysedType),
      (error) => new Error(error),
    );
  }

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

        case 'multimodal':
          throw new Error(
            'Test failure: multimodal types should not be part of other parameters',
          );
      }
    },
  );

  return {
    tag: 'tuple',
    val: elementValues,
  };
}

// Only in tests, we end up having to convert the result of dynamic invoke back to typescript value.
// In reality, only constructor arguments and method arugments which comes in as data-value is converted to
// a typescript value. This functionality will help ensure
// the `DataValue` returned by invoke is a properly serialised version
// of the typescript method result.
function deserializeReturnValue(
  methodName: string,
  returnValue: DataValue,
): Either.Either<any, string> {
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

  const result = deserializeDataValue(returnValue, [
    {
      parameterName: 'return-value',
      parameterTypeInfo: returnTypeAnalysedType,
    },
  ]);

  // typescript compiles even if you don't index it by 0
  // any[] === any
  return Either.map(result, (r) => r[0]);
}

function overrideSelfMetadataImpl(agentClassName: AgentClassName) {
  vi.spyOn(GolemApiHostModule, 'getSelfMetadata').mockImplementation(() => ({
    agentId: {
      componentId: {
        uuid: {
          highBits: 42n,
          lowBits: 99n,
        },
      },
      agentId: agentClassName.asWit,
    },
    args: [],
    env: [],
    configVars: [],
    status: 'running',
    componentVersion: 0n,
    retryCount: 0n,
  }));
}
