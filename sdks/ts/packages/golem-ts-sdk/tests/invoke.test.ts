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

import { ClassMetadata, TypeMetadata } from '@golemcloud/golem-ts-types-core';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { expect } from 'vitest';
import { BarAgentClassName, BarAgentCustomClassName, FooAgentClassName } from './testUtils';
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
import * as util from 'node:util';
import { AgentConstructorParamRegistry } from '../src/internal/registry/agentConstructorParamRegistry';
import { AgentMethodParamRegistry } from '../src/internal/registry/agentMethodParamRegistry';
import { AgentMethodRegistry } from '../src/internal/registry/agentMethodRegistry';
import {
  ParsedAgentId,
  MultimodalAdvanced,
  Multimodal,
  Result,
  UnstructuredBinary,
  UnstructuredText,
} from '../src';
import { SchemaValue } from '../src/internal/schema-model';
import { RuntimeParam } from '../src/internal/typeInfoInternal';
import { decodeOutput, encodeInputRecord } from '../src/internal/mapping/values/boundaryValue';
import { TextOrImage } from './validAgents';

test('BarAgent can be successfully initiated', () => {
  fc.assert(
    fc.property(
      interfaceArb,
      fc.oneof(fc.string(), fc.constant(null)),
      fc.oneof(unionArb, fc.constant(null)),
      (interfaceValue, stringValue, unionValue) => {
        overrideSelfAgentId(new ParsedAgentId('my-complex-agent()'));

        const typeRegistry = TypeMetadata.get(BarAgentClassName.value);

        if (!typeRegistry) {
          throw new Error('BarAgent type metadata not found');
        }

        // The BarAgent constructor takes, in order:
        //   TestInterfaceType, string | null, UnionType | null,
        //   UnstructuredText, UnstructuredText<['en', 'de']>,
        //   UnstructuredBinary<['application/json']>
        // Unstructured text/binary are supplied as their schema-native runtime
        // value shapes (url references here, which carry no language/mime
        // restrictions to satisfy).
        const textReference: UnstructuredText<['en', 'de']> = {
          tag: 'url',
          val: 'https://example.com/sample.txt',
        };

        const binaryReference: UnstructuredBinary<['application/json']> = {
          tag: 'url',
          val: 'https://example.com/binary',
        };

        const names = typeRegistry.constructorArgs.map((arg) => arg.name);
        const values = [
          interfaceValue,
          stringValue,
          unionValue,
          textReference,
          textReference,
          binaryReference,
        ];
        const valuesByName = new Map<string, any>(names.map((n, i) => [n, values[i]]));

        const constructorInput = buildConstructorInput('BarAgent', valuesByName);

        const agentInitiator = AgentInitiatorRegistry.lookup(BarAgentCustomClassName.value);

        if (!agentInitiator) {
          throw new Error('BarAgent not found in AgentInitiatorRegistry');
        }

        const result = agentInitiator.initiate(constructorInput, { tag: 'anonymous' });

        expect(result.tag).toEqual('ok');
      },
    ),
  );
});

test('An agent can be successfully initiated and all of its methods can be invoked', async () => {
  await fc.assert(
    fc.asyncProperty(
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
      async (
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
        overrideSelfAgentId(new ParsedAgentId('FooAgent()'));

        const classMetadata = TypeMetadata.get(FooAgentClassName.value);

        if (!classMetadata) {
          throw new Error('FooAgent type metadata not found');
        }

        const resolvedAgent = initiateFooAgent(arbString, classMetadata);

        // Invoking function with string type
        await testInvoke(
          'fun1',
          [['param', arbString]],
          resolvedAgent,
          'Weather in ' + arbString + ' is sunny!',
        );

        // Invoking function with multiple primitive types
        await testInvoke(
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
        await testInvoke(
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
        await testInvoke(
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
        await testInvoke(
          'fun5',
          [['param', arbString]],
          resolvedAgent,
          `Weather in ${arbString} is sunny!`,
        );

        // Void return type
        await testInvoke('fun6', [['param', arbString]], resolvedAgent, undefined);

        // Invoking with various kind of optional types embedded in union type
        await testInvoke(
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
        await testInvoke('fun8', [['a', unionWithLiterals]], resolvedAgent, unionWithLiterals);

        // Invoking with tagged union
        await testInvoke('fun9', [['param', taggedUnion]], resolvedAgent, taggedUnion);

        // Invoking with union with only literals
        await testInvoke(
          'fun10',
          [['param', unionWithOnlyLiterals]],
          resolvedAgent,
          unionWithOnlyLiterals,
        );

        // Invoking with result type
        await testInvoke(
          'fun11',
          [['param', resultTypeExactBoth]],
          resolvedAgent,
          resultTypeExactBoth,
        );

        // invoking with result-like type
        await testInvoke(
          'fun12',
          [['param', resultTypeNonExact]],
          resolvedAgent,
          resultTypeNonExact,
        );

        // invoking with another result-like type
        await testInvoke(
          'fun13',
          [['param', resultTypeNonExact2]],
          resolvedAgent,
          resultTypeNonExact2,
        );

        // Invoking with unstructured text
        await testInvoke('fun15', [['param', unstructuredText]], resolvedAgent, unstructuredText);

        // Invoking with unstructured text with language code
        await testInvoke(
          'fun16',
          [['param', unstructuredTextWithLC]],
          resolvedAgent,
          unstructuredTextWithLC,
        );

        // Invoking with unstructured binary with mime type
        await testInvoke(
          'fun17',
          [['param', unstructuredBinaryWithMimeType]],
          resolvedAgent,
          unstructuredBinaryWithMimeType,
        );
      },
    ),
  );
});

test('Invoke function that takes and returns inbuilt result type', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  await testInvoke(
    'fun30',
    [['param', Result.err('message')]],
    resolvedAgent,
    Result.err('message'),
  );

  await testInvoke('fun30', [['param', Result.ok(true)]], resolvedAgent, Result.ok(true));

  // aliased result test
  await testInvoke('fun31', [['param', Result.ok(true)]], resolvedAgent, Result.ok(true));
});

test('Invoke function that returns unit type', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  // A unit return materialises as no schema value crossing the boundary.
  await testInvoke('fun45', [['param', 'foo']], resolvedAgent, undefined, (raw) =>
    expect(raw).toBeUndefined(),
  );
});

test('Invoke function that takes and returns custom result type with void', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  await testInvoke('fun43', [['param', { tag: 'ok', okValue: undefined }]], resolvedAgent, {
    tag: 'ok',
    okValue: undefined,
  });

  await testInvoke('fun43', [['param', { tag: 'err', errValue: undefined }]], resolvedAgent, {
    tag: 'err',
    errValue: undefined,
  });
});

test('Invoke function that takes and returns inbuilt result type with void', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  await testInvoke('fun44', [['param', Result.ok(undefined)]], resolvedAgent, Result.ok(undefined));

  await testInvoke(
    'fun44',
    [['param', Result.err(undefined)]],
    resolvedAgent,
    Result.err(undefined),
  );
});

test('Invoke function that takes and returns inbuilt result type with undefined', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));
  const classMetadata = TypeMetadata.get(FooAgentClassName.value);
  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  await testInvoke('fun32', [['param', 'foo']], resolvedAgent, Result.ok(undefined));

  await testInvoke('fun33', [['param', 'foo']], resolvedAgent, Result.err(undefined));

  await testInvoke('fun34', [['param', 'foo']], resolvedAgent, Result.ok(undefined));

  await testInvoke('fun35', [['param', 'foo']], resolvedAgent, Result.ok(undefined));

  await testInvoke('fun36', [['param', 'foo']], resolvedAgent, Result.err(undefined));

  await testInvoke('fun37', [['param', 'foo']], resolvedAgent, Result.ok(undefined));
});

test('Invoke function that takes and returns multimodal default', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));

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

  await testInvoke('fun18', [['param', multimodalInput]], resolvedAgent, multimodalInput);
});

test('Invoke function that takes and returns multimodal basic', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));

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

  await testInvoke('fun38', [['param', multimodalInput]], resolvedAgent, multimodalInput);
});

test('Invoke function that takes and returns typed array', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('foo', classMetadata);

  await fc.assert(
    fc.asyncProperty(
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
      async (uint8, uint16, uint32, uint64, int8, int16, int32, int64, float32, float64) => {
        await testInvoke('fun19', [['param', uint8]], resolvedAgent, uint8);

        await testInvoke('fun20', [['param', uint16]], resolvedAgent, uint16);

        await testInvoke('fun27', [['param', uint32]], resolvedAgent, uint32);

        await testInvoke('fun23', [['param', uint64]], resolvedAgent, uint64);

        await testInvoke('fun24', [['param', int8]], resolvedAgent, int8);

        await testInvoke('fun25', [['param', int16]], resolvedAgent, int16);

        await testInvoke('fun26', [['param', int32]], resolvedAgent, int32);

        await testInvoke('fun29', [['param', int64]], resolvedAgent, int64);

        await testInvoke('fun21', [['param', float32]], resolvedAgent, float32);

        await testInvoke('fun28', [['param', float64]], resolvedAgent, float64);
      },
    ),
  );
});

test('Invoke function that takes any unstructured-binary and returns any unstructured-binary', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));

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

  await testInvoke('fun40', [['param', binary]], resolvedAgent, binary);
});

test('Invoke function that takes json unstructured-binary and returns json unstructured-binary', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));

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

  await testInvoke('fun40', [['param', binary]], resolvedAgent, binary);
});

// This is already in the above big test, but we keep it separate to have a clearer
// view of how unstructured text is handled.
test('Invoke method with optional parameter using question syntax', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('test', classMetadata);

  // Test with optional parameter provided
  await testInvoke(
    'fun41',
    [
      ['required', 'hello'],
      ['optional', 42],
    ],
    resolvedAgent,
    { required: 'hello', optional: 42 },
  );

  // Test with optional parameter omitted
  await testInvoke('fun41', [['required', 'world']], resolvedAgent, {
    required: 'world',
    optional: undefined,
  });
});

test('Invoke method with optional parameter using | undefined syntax', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));

  const classMetadata = TypeMetadata.get(FooAgentClassName.value);

  if (!classMetadata) {
    throw new Error('FooAgent type metadata not found');
  }

  const resolvedAgent = initiateFooAgent('test', classMetadata);

  // Test with optional parameter provided
  await testInvoke(
    'fun42',
    [
      ['required', 'hello'],
      ['optional', 123],
    ],
    resolvedAgent,
    { required: 'hello', optional: 123 },
  );

  // Test with optional parameter omitted
  await testInvoke('fun42', [['required', 'world']], resolvedAgent, {
    required: 'world',
    optional: undefined,
  });
});

test('Invoke function that takes unstructured-text and returns unstructured-text', async () => {
  overrideSelfAgentId(new ParsedAgentId('FooAgent()'));

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

  await testInvoke(
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

  const input = buildMethodInput('fun16', [['param', invalidUnstructuredText]]);

  const invokeResult = await resolvedAgent.invoke('fun16', input, { tag: 'anonymous' });

  if (invokeResult.tag === 'ok') {
    throw new Error('Test failure: invocation should have failed');
  } else {
    expect(JSON.stringify(invokeResult.val)).toContain(
      'Failed to deserialize arguments for method fun16 in agent FooAgent: Invalid value for parameter param. Language code `pl` is not allowed. Allowed codes: en, de',
    );
  }
});

function initiateFooAgent(constructorParam: string, simpleAgentClassMeta: ClassMetadata) {
  const constructorInfo = simpleAgentClassMeta.constructorArgs[0];

  const valuesByName = new Map<string, any>([[constructorInfo.name, constructorParam]]);
  const constructorParams = buildConstructorInput('FooAgent', valuesByName);

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

async function testInvoke(
  methodName: string,
  parameterNameAndValues: [string, any][],
  resolvedAgent: ResolvedAgent,
  expectedOutput: any,
  assertRawOutput?: (raw: SchemaValue | undefined) => void,
) {
  // Build the schema-native input record and invoke dynamically.
  const input = buildMethodInput(methodName, parameterNameAndValues);

  const invokeResult = await resolvedAgent.invoke(methodName, input, { tag: 'anonymous' });

  const resultSchemaValue =
    invokeResult.tag === 'ok'
      ? invokeResult.val
      : (() => {
          throw new Error('Test failure: ' + JSON.stringify(invokeResult.val));
        })();

  if (assertRawOutput) {
    assertRawOutput(resultSchemaValue);
  }

  // Deserialize the result so we can assert it round-trips back to the original
  // TypeScript value.
  const result = deserializeReturnValue(methodName, resultSchemaValue);

  expect(result).toEqual(expectedOutput);
}

/**
 * Build the schema-native input record for a method invocation. Fields are laid
 * out in the registry's parameter order (which is what the boundary decoder
 * expects); any parameter not present in `parameterNameAndValues` is encoded as
 * `undefined` (an omitted trailing optional becomes `option none`).
 */
function buildMethodInput(
  methodName: string,
  parameterNameAndValues: [string, any][],
): SchemaValue {
  const valueByName = new Map(parameterNameAndValues);
  const paramTypes = AgentMethodParamRegistry.getParametersAndType('FooAgent', methodName);

  const userParams: RuntimeParam[] = [];
  const args: any[] = [];
  for (const [name, type] of paramTypes) {
    userParams.push({ name, type });
    args.push(valueByName.has(name) ? valueByName.get(name) : undefined);
  }

  return encodeInputRecord(args, userParams);
}

/**
 * Build the schema-native constructor input record. `principal` / `config`
 * parameters do not consume a record field; every other constructor parameter
 * does, in declaration order.
 */
function buildConstructorInput(
  agentClassName: string,
  valuesByName: Map<string, any>,
): SchemaValue {
  const classMeta = AgentConstructorParamRegistry.get(agentClassName);
  if (!classMeta) {
    throw new Error(`Constructor metadata for ${agentClassName} not found`);
  }

  const userParams: RuntimeParam[] = [];
  const args: any[] = [];
  for (const [name, meta] of classMeta) {
    const type = meta.typeInfo;
    if (!type) {
      throw new Error(`Unresolved type for constructor parameter ${name} in ${agentClassName}`);
    }
    if (type.tag === 'principal' || type.tag === 'config') {
      continue;
    }
    userParams.push({ name, type });
    args.push(valuesByName.has(name) ? valuesByName.get(name) : undefined);
  }

  return encodeInputRecord(args, userParams);
}

// Only in tests, we end up having to convert the result of dynamic invoke back to typescript value.
// In reality, only constructor arguments and method arguments which come in as a schema value are
// converted to a typescript value. This functionality helps ensure the `SchemaValue` returned by
// invoke is a properly serialised version of the typescript method result.
function deserializeReturnValue(methodName: string, returnValue: SchemaValue | undefined): any {
  const returnType = AgentMethodRegistry.getReturnType('FooAgent', methodName);

  if (!returnType) {
    throw new Error(`Unsupported return type for method ${methodName}`);
  }

  return decodeOutput(returnValue, returnType);
}

function overrideSelfAgentId(agentId: ParsedAgentId) {
  (globalThis as any).currentAgentId = agentId.value;
}
