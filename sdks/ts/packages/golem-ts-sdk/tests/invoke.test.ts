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
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { expect } from 'vitest';
import { BarAgentClassName, BarAgentCustomClassName, FooAgentClassName } from './testUtils';
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
import { BinaryReference, DataValue, ElementValue, TextReference } from 'golem:agent/common@1.5.0';
import * as util from 'node:util';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from '../src/internal/registry/agentMethodParamRegistry';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';
import {
  AgentId,
  MultimodalAdvanced,
  Multimodal,
  Result,
  UnstructuredBinary,
  UnstructuredText,
  AgentClassName,
} from '../src';
import {
  serializeTsValueToBinaryReference,
  serializeTsValueToTextReference,
} from '../src/internal/mapping/values/serializer';
import {
  deserializeDataValue,
  serializeToDataValue,
} from '../src/internal/mapping/values/dataValue';
import { TextOrImage } from './validAgents';

test('BarAgent can be successfully initiated', () => {
  fc.assert(
    fc.property(
      interfaceArb,
      fc.oneof(fc.string(), fc.constant(null)),
      fc.oneof(unionArb, fc.constant(null)),
      (interfaceValue, stringValue, unionValue) => {
        overrideSelfAgentId(new AgentId('my-complex-agent()'));

        const typeRegistry = TypeMetadata.get(BarAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('BarAgent type metadata not found');
        }

        // TestInterfaceType
        const arg0 = AgentConstructorParamRegistry.getParamType(
          'BarAgent',
          typeRegistry.constructorArgs[0].name,
        );

        // string | null
        const arg1 = AgentConstructorParamRegistry.getParamType(
          'BarAgent',
          typeRegistry.constructorArgs[1].name,
        );

        // UnionType | null
        const arg2 = AgentConstructorParamRegistry.getParamType(
          'BarAgent',
          typeRegistry.constructorArgs[2].name,
        );

        // UnstructuredText,
        const arg3 = AgentConstructorParamRegistry.getParamType(
          'BarAgent',
          typeRegistry.constructorArgs[3].name,
        );

        // UnstructuredText<['en', 'de']>
        const arg4 = AgentConstructorParamRegistry.getParamType(
          'BarAgent',
          typeRegistry.constructorArgs[4].name,
        );

        // UnstructuredBinary<['application/json']>
        const arg5 = AgentConstructorParamRegistry.getParamType(
          'BarAgent',
          typeRegistry.constructorArgs[5].name,
        );

        if (
          !arg0 ||
          !arg1 ||
          !arg2 ||
          !arg3 ||
          !arg4 ||
          !arg5 ||
          arg0.tag !== 'analysed' ||
          arg1.tag !== 'analysed' ||
          arg2.tag !== 'analysed' ||
          arg3.tag !== 'unstructured-text' ||
          arg4.tag !== 'unstructured-text' ||
          arg5.tag !== 'unstructured-binary'
        ) {
          throw new Error('Test failure: unresolved type in BarAgent constructor');
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

        const textReference: TextReference = {
          tag: 'url',
          val: 'https://example.com/sample.txt',
        };

        const binaryReference: BinaryReference = {
          tag: 'url',
          val: 'https://example.com/binary',
        };

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
            {
              tag: 'unstructured-text',
              val: textReference,
            },
            {
              tag: 'unstructured-text',
              val: textReference,
            },
            {
              tag: 'unstructured-binary',
              val: binaryReference,
            },
          ],
        };

        const agentInitiator = AgentInitiatorRegistry.lookup(BarAgentCustomClassName.value);

        if (!agentInitiator) {
          throw new Error('BarAgent not found in AgentInitiatorRegistry');
        }

        const result = agentInitiator.initiate(dataValue, { tag: 'anonymous' });

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
        overrideSelfAgentId(new AgentId('foo-agent()'));

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
        testInvoke('fun6', [['param', arbString]], resolvedAgent, undefined, false);

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
        testInvoke('fun8', [['a', unionWithLiterals]], resolvedAgent, unionWithLiterals, false);

        // Invoking with tagged union
        testInvoke('fun9', [['param', taggedUnion]], resolvedAgent, taggedUnion, false);

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
        testInvoke('fun15', [['param', unstructuredText]], resolvedAgent, unstructuredText, false);

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
  overrideSelfAgentId(new AgentId('foo-agent()'));
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

  testInvoke('fun30', [['param', Result.ok(true)]], resolvedAgent, Result.ok(true), false);

  // aliased result test
  testInvoke('fun31', [['param', Result.ok(true)]], resolvedAgent, Result.ok(true), false);
});

test('Invoke function that returns unit type', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  testInvoke('fun45', [['param', 'foo']], resolvedAgent, undefined, false, {
    tag: 'tuple',
    val: [],
  });
});

test('Invoke function that takes and returns custom result type with void', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  testInvoke(
    'fun43',
    [['param', { tag: 'ok', okValue: undefined }]],
    resolvedAgent,
    { tag: 'ok', okValue: undefined },
    false,
  );

  testInvoke(
    'fun43',
    [['param', { tag: 'err', errValue: undefined }]],
    resolvedAgent,
    { tag: 'err', errValue: undefined },
    false,
  );
});

test('Invoke function that takes and returns inbuilt result type with void', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  testInvoke(
    'fun44',
    [['param', Result.ok(undefined)]],
    resolvedAgent,
    Result.ok(undefined),
    false,
  );

  testInvoke(
    'fun44',
    [['param', Result.err(undefined)]],
    resolvedAgent,
    Result.err(undefined),
    false,
  );
});

test('Invoke function that takes and returns inbuilt result type with undefined', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  testInvoke('fun32', [['param', 'foo']], resolvedAgent, Result.ok(undefined), false);

  testInvoke('fun33', [['param', 'foo']], resolvedAgent, Result.err(undefined), false);

  testInvoke('fun34', [['param', 'foo']], resolvedAgent, Result.ok(undefined), false);

  testInvoke('fun35', [['param', 'foo']], resolvedAgent, Result.ok(undefined), false);

  testInvoke('fun36', [['param', 'foo']], resolvedAgent, Result.err(undefined), false);

  testInvoke('fun37', [['param', 'foo']], resolvedAgent, Result.ok(undefined), false);
});

test('Invoke function that takes and returns multimodal default', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  const multimodalInput: MultimodalAdvanced<TextOrImage> = [
    { tag: 'un-text', val: { tag: 'inline', val: 'data' } },
    { tag: 'un-binary', val: { tag: 'url', val: 'https://foo.bar/image.png' } },
    { tag: 'text', val: 'foo' },
    { tag: 'image', val: new Uint8Array([137, 80, 78, 71]) },
    { tag: 'un-text', val: { tag: 'url', val: 'https://foo.bar/image.png' } },
    {
      tag: 'un-binary',
      val: {
        tag: 'inline',
        val: new Uint8Array([1, 2, 3]),
        mimeType: 'application/json',
      },
    },
  ];

  testInvoke('fun18', [['param', multimodalInput]], resolvedAgent, multimodalInput, true);
});

test('Invoke function that takes and returns multimodal basic', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  const multimodalInput: Multimodal = [
    { tag: 'binary', val: { tag: 'url', val: 'https://foo.bar/image.png' } },
    {
      tag: 'binary',
      val: {
        tag: 'inline',
        val: new Uint8Array([1, 2, 3]),
        mimeType: 'application/json',
      },
    },
    { tag: 'text', val: { tag: 'inline', val: 'some text' } },
    { tag: 'text', val: { tag: 'url', val: 'https://foo.bar/some-text.txt' } },
  ];

  testInvoke('fun38', [['param', multimodalInput]], resolvedAgent, multimodalInput, true);
});

test('Invoke function that takes and returns typed array', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));

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
      (uint8, uint16, uint32, uint64, int8, int16, int32, int64, float32, float64) => {
        testInvoke('fun19', [['param', uint8]], resolvedAgent, uint8, false);

        testInvoke('fun20', [['param', uint16]], resolvedAgent, uint16, false);

        testInvoke('fun27', [['param', uint32]], resolvedAgent, uint32, false);

        testInvoke('fun23', [['param', uint64]], resolvedAgent, uint64, false);

        testInvoke('fun24', [['param', int8]], resolvedAgent, int8, false);

        testInvoke('fun25', [['param', int16]], resolvedAgent, int16, false);

        testInvoke('fun26', [['param', int32]], resolvedAgent, int32, false);

        testInvoke('fun29', [['param', int64]], resolvedAgent, int64, false);

        testInvoke('fun21', [['param', float32]], resolvedAgent, float32, false);

        testInvoke('fun28', [['param', float64]], resolvedAgent, float64, false);
      },
    ),
  );
});

test('Invoke function that takes any unstructured-binary and returns any unstructured-binary', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  const binary: UnstructuredBinary = {
    tag: 'inline',
    val: new Uint8Array([1, 2, 3]),
    mimeType: 'application/json',
  };

  testInvoke('fun40', [['param', binary]], resolvedAgent, binary, false);
});

test('Invoke function that takes json unstructured-binary and returns json unstructured-binary', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  const binary: UnstructuredBinary<['application/json']> = {
    tag: 'inline',
    val: new Uint8Array([1, 2, 3]),
    mimeType: 'application/json',
  };

  testInvoke('fun40', [['param', binary]], resolvedAgent, binary, false);
});

// This is already in the above big test, but we keep it separate to have a clearer
// view of how unstructured text is handled.
test('Invoke method with optional parameter using question syntax', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('test', classMetadata);

  // Test with optional parameter provided
  testInvoke(
    'fun41',
    [
      ['required', 'hello'],
      ['optional', 42],
    ],
    resolvedAgent,
    { required: 'hello', optional: 42 },
    false,
  );

  // Test with optional parameter omitted
  testInvoke(
    'fun41',
    [['required', 'world']],
    resolvedAgent,
    { required: 'world', optional: undefined },
    false,
  );
});

test('Invoke method with optional parameter using | undefined syntax', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('test', classMetadata);

  // Test with optional parameter provided
  testInvoke(
    'fun42',
    [
      ['required', 'hello'],
      ['optional', 123],
    ],
    resolvedAgent,
    { required: 'hello', optional: 123 },
    false,
  );

  // Test with optional parameter omitted
  testInvoke(
    'fun42',
    [['required', 'world']],
    resolvedAgent,
    { required: 'world', optional: undefined },
    false,
  );
});

test('Invoke function that takes unstructured-text and returns unstructured-text', () => {
  overrideSelfAgentId(new AgentId('foo-agent()'));

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

  const dataValue = createInputDataValue([['param', invalidUnstructuredText]], 'fun16', false);

  resolvedAgent.invoke('fun16', dataValue, { tag: 'anonymous' }).then((invokeResult) => {
    if (invokeResult.tag === 'ok') {
      throw new Error('Test failure: invocation should have failed');
    } else {
      expect(invokeResult.val.val).toContain(
        'Failed to deserialize arguments for method fun16 in agent FooAgent: Invalid value for parameter param. Language code `pl` is not allowed. Allowed codes: en, de',
      );
    }
  });
});

function initiateFooAgent(constructorParam: string, simpleAgentClassMeta: ClassMetadata) {
  const constructorInfo = simpleAgentClassMeta.constructorArgs[0];

  const constructorParamTypeInfoInternal = AgentConstructorParamRegistry.getParamType(
    'FooAgent',
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

  const agentInitiator = AgentInitiatorRegistry.lookup(FooAgentClassName.value);

  if (!agentInitiator) {
    throw new Error('FooAgent not found in AgentInitiatorRegistry');
  }

  const result = agentInitiator.initiate(constructorParams, {
    tag: 'anonymous',
  });

  if (result.tag !== 'ok') {
    throw new Error(util.format('Agent initiation failed: %s', JSON.stringify(result.val)));
  }

  return result.val;
}

function testInvoke(
  methodName: string,
  parameterNameAndValues: [string, any][],
  resolvedAgent: ResolvedAgent,
  expectedOutput: any,
  multimodal: boolean,
  expectedDataValueOutput?: DataValue,
) {
  // We need to first manually form the data-value to test the dynamic invoke.
  // For this, we first convert the original ts-value to data value and do a round trip to ensure
  // data matches exact.
  const dataValue = createInputDataValue(parameterNameAndValues, methodName, multimodal);

  resolvedAgent.invoke(methodName, dataValue, { tag: 'anonymous' }).then((invokeResult) => {
    const resultDataValue =
      invokeResult.tag === 'ok'
        ? invokeResult.val
        : (() => {
            throw new Error('Test failure: ' + JSON.stringify(invokeResult.val));
          })();

    if (expectedDataValueOutput !== undefined) {
      expect(resultDataValue).toEqual(expectedDataValueOutput);
    }

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
      'FooAgent',
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

    const result = serializeToDataValue(value, paramAnalysedType);

    return Either.getOrThrowWith(result, (error) => new Error(error));
  }

  const elementValues: ElementValue[] = parameterNameAndValues.map(([paramName, value]) => {
    const paramAnalysedType = AgentMethodParamRegistry.getParamType(
      'FooAgent',
      methodName,
      paramName,
    );

    if (!paramAnalysedType) {
      throw new Error(`Unresolved type for \`${paramName}\` in method \`${methodName}\``);
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
        const textReference = serializeTsValueToTextReference(value);
        return {
          tag: 'unstructured-text',
          val: textReference,
        };

      case 'unstructured-binary':
        const binaryReference = serializeTsValueToBinaryReference(value);
        return {
          tag: 'unstructured-binary',
          val: binaryReference,
        };

      case 'principal':
        throw new Error('Test failure: principal types should never be part of method parameters');

      case 'config':
        throw new Error('Test failure: config types should never be part of method parameters');

      case 'multimodal':
        throw new Error('Test failure: multimodal types should not be part of other parameters');
    }
  });

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
  const returnType = TypeMetadata.get(FooAgentClassName.value)?.methods.get(methodName)?.returnType;

  if (!returnType) {
    throw new Error(`Method ${methodName} not found in metadata`);
  }

  const returnTypeAnalysedType = AgentMethodRegistry.getReturnType('FooAgent', methodName);

  if (!returnTypeAnalysedType) {
    throw new Error(`Unsupported return type for method ${methodName}`);
  }

  const result = deserializeDataValue(
    returnValue,
    [
      {
        name: 'return-value',
        type: returnTypeAnalysedType,
      },
    ],
    {
      tag: 'anonymous',
    },
  );

  // typescript compiles even if you don't index it by 0
  // any[] === any
  return Either.map(result, (r) => r[0]);
}

function overrideSelfAgentId(agentId: AgentId) {
  (globalThis as any).currentAgentId = agentId.value;
  // vi.mock('wasi:cli/environment@0.2.3', () => ({
  //   getEnvironment: (): [string, string][] => {
  //     return [['GOLEM_AGENT_ID', agentId.value]];
  //   },
  // }));
}
