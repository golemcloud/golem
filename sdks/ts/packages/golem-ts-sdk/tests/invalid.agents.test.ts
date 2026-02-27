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

// Interface type indirectly tests primitive types, union, list etc

import { describe, expect } from 'vitest';
import { TypeMetadata } from '@golemcloud/golem-ts-types-core';
import * as AnalysedType from '../src/internal/mapping/types/analysedType';
import { TypeScope } from '../src/internal/mapping/types/scope';
import * as Either from '../src/newTypes/either';
import { agent, AgentId, BaseAgent } from '../src';
import { typeMapper } from '../src/internal/mapping/types/typeMapperImpl';

const invalidAgent = TypeMetadata.getAll().get('InvalidAgent');
const fun1Params = invalidAgent?.methods.get('fun1')?.methodParams;

// We almost let agent run and get the errors from it (yet), because - it doesn't accumulate all errors
// and it doesn't return a value for us to inspect but rather the import fails.
describe('Invalid types in agents', () => {
  it('invalid types in method inputs will return error', () => {
    const dateType = getAnalysedTypeInFun1('date');

    const regExpType = getAnalysedTypeInFun1('regExp');

    const iteratorType = getAnalysedTypeInFun1('iterator');

    const iterableType = getAnalysedTypeInFun1('iterable');

    const asyncIteratorType = getAnalysedTypeInFun1('asyncIterator');

    const asyncIterableType = getAnalysedTypeInFun1('asyncIterable');

    const anyType = getAnalysedTypeInFun1('any');

    const stringType = getAnalysedTypeInFun1('string');

    const booleanType = getAnalysedTypeInFun1('boolean');

    const symbolType = getAnalysedTypeInFun1('symbol');

    const numberType = getAnalysedTypeInFun1('number');

    const bigintType = getAnalysedTypeInFun1('bigint');

    const nullType = getAnalysedTypeInFun1('nullParam');

    const undefinedType = getAnalysedTypeInFun1('undefined');

    const voidType = getAnalysedTypeInFun1('voidParam');

    const unionWithKeyWord = getAnalysedTypeInFun1('unionWithKeyWord');

    const resultTypeInvalid1 = getAnalysedTypeInFun1('resultTypeInvalid1');

    const resultTypeInvalid2 = getAnalysedTypeInFun1('resultTypeInvalid2');

    const resultTypeInvalid3 = getAnalysedTypeInFun1('resultTypeInvalid3');

    expect(dateType.val).toBe('Unsupported type `Date`. Use a `string` if possible');

    expect(regExpType.val).toBe('Unsupported type `RegExp`. Use a `string` if possible');

    expect(iteratorType.val).toBe('Unsupported type `Iterator`. Use `Array` type instead');

    expect(iterableType.val).toBe('Unsupported type `Iterable`. Use `Array` type instead');

    expect(asyncIteratorType.val).toBe('Unsupported type `Iterator`. Use `Array` type instead');

    expect(asyncIterableType.val).toBe(
      'Unsupported type `AsyncIterator`. Use `Array` type instead',
    );

    expect(anyType.val).toBe('Unsupported type `any`. Use a specific type instead');

    expect(stringType.val).toBe('Unsupported type `String`, use `string` instead');

    expect(booleanType.val).toBe('Unsupported type `Boolean`, use `boolean` instead');

    expect(numberType.val).toBe('Unsupported type `Number`, use `number` instead');

    expect(symbolType.val).toBe('Unsupported type `Symbol`, use `string` if possible');

    expect(bigintType.val).toBe('Unsupported type `BigInt`, use `bigint` instead');

    expect(nullType.val).toBe('Unsupported type `null` in fun1 for parameter `nullParam`');

    expect(undefinedType.val).toBe(
      'Unsupported type `undefined` in fun1 for parameter `undefined`',
    );

    expect(voidType.val).toBe('Unsupported type `void` in fun1 for parameter `voidParam`');

    expect(unionWithKeyWord.val).toBe(
      '`ok` is a reserved keyword. The following keywords cannot be used as literals: ok, err, none, some',
    );

    expect(resultTypeInvalid1.val).toBe(
      "The value corresponding to the tag 'ok' cannot be optional. Avoid using the tag names `ok`, `err`. Alternatively, make the value type non optional",
    );

    expect(resultTypeInvalid2.val).toBe(
      "The value corresponding to the tag 'err' cannot be optional. Avoid using the tag names `ok`, `err`. Alternatively, make the value type non optional",
    );

    expect(resultTypeInvalid3.val).toBe(
      "The value corresponding to the tag 'ok' cannot be optional. Avoid using the tag names `ok`, `err`. Alternatively, make the value type non optional",
    );
  });

  // Act as more of a regression test
  it('invalid types in method outputs will return error', () => {
    const fun2ReturnType = invalidAgent?.methods.get('fun2')?.returnType;
    const fun3ReturnType = invalidAgent?.methods.get('fun3')?.returnType;
    const fun4ReturnType = invalidAgent?.methods.get('fun4')?.returnType;
    const fun5ReturnType = invalidAgent?.methods.get('fun5')?.returnType;
    const fun6ReturnType = invalidAgent?.methods.get('fun6')?.returnType;
    const fun7ReturnType = invalidAgent?.methods.get('fun7')?.returnType;
    const fun8ReturnType = invalidAgent?.methods.get('fun8')?.returnType;
    const fun9ReturnType = invalidAgent?.methods.get('fun9')?.returnType;
    const fun10ReturnType = invalidAgent?.methods.get('fun10')?.returnType;
    const fun11ReturnType = invalidAgent?.methods.get('fun11')?.returnType;
    const fun12ReturnType = invalidAgent?.methods.get('fun12')?.returnType;
    const fun13ReturnType = invalidAgent?.methods.get('fun13')?.returnType;
    const fun14ReturnType = invalidAgent?.methods.get('fun14')?.returnType;

    const fun2Type = typeMapper(fun2ReturnType!, undefined);
    const fun3Type = typeMapper(fun3ReturnType!, undefined);
    const fun4Type = typeMapper(fun4ReturnType!, undefined);
    const fun5Type = typeMapper(fun5ReturnType!, undefined);
    const fun6Type = typeMapper(fun6ReturnType!, undefined);
    const fun7Type = typeMapper(fun7ReturnType!, undefined);
    const fun8Type = typeMapper(fun8ReturnType!, undefined);
    const fun9Type = typeMapper(fun9ReturnType!, undefined);
    const fun10Type = typeMapper(fun10ReturnType!, undefined);
    const fun11Type = typeMapper(fun11ReturnType!, undefined);
    const fun12Type = typeMapper(fun12ReturnType!, undefined);
    const fun13Type = typeMapper(fun13ReturnType!, undefined);
    const fun14Type = typeMapper(fun14ReturnType!, undefined);

    expect(fun2Type.val).toBe('Unsupported type `Date`. Use a `string` if possible');

    expect(fun3Type.val).toBe('Unsupported type `Iterator`. Use `Array` type instead');

    expect(fun4Type.val).toBe('Unsupported type `Iterable`. Use `Array` type instead');

    expect(fun5Type.val).toBe('Unsupported type `Iterator`. Use `Array` type instead');

    expect(fun6Type.val).toBe('Unsupported type `AsyncIterator`. Use `Array` type instead');

    expect(fun7Type.val).toBe('Unsupported type `any`. Use a specific type instead');

    expect(fun8Type.val).toBe('Unsupported type `String`, use `string` instead');

    expect(fun9Type.val).toBe('Unsupported type `Number`, use `number` instead');

    expect(fun10Type.val).toBe('Unsupported type `Boolean`, use `boolean` instead');

    expect(fun11Type.val).toBe('Unsupported type `Symbol`, use `string` if possible');

    expect(fun12Type.val).toBe('Unsupported type `BigInt`, use `bigint` instead');

    expect(fun13Type.val).toBe('Unsupported type `Object`');

    expect(fun14Type.val).toBe(
      '`RecursiveType` is recursive.\nRecursive types are not supported yet.\nHelp: Avoid recursion in this type (e.g. using index-based node lists) and try again.',
    );
  });
});

test('Undecorated agent that extends BaseAgent throws reasonable error messages', () => {
  // This is to simulate the idea that a non-agentic class extends BaseAgent and using its functionalities

  expect(() => UndecoratedAgent.get()).toThrowError(
    'Remote client creation failed: `UndecoratedAgent` must be decorated with @agent()',
  );

  const undecorated = new UndecoratedAgent();

  expect(() => undecorated.getId()).toThrowError(
    'AgentId is not available for `UndecoratedAgent`. Ensure the class is decorated with @agent()',
  );

  expect(() => undecorated.getAgentType()).toThrowError(
    'Agent type metadata is not available for \`UndecoratedAgent\`. Ensure the class is decorated with @agent()',
  );
});

test('Agent without BaseAgent throws reasonable error messages', () => {
  // Calling agent() directly as a function to test the runtime check,
  // since TypeScript's type system now correctly prevents @agent() on non-BaseAgent classes
  expect(() => {
    class AgentWithoutBaseAgent {
      async foo(): Promise<void> {
        return;
      }
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any -- testing runtime error for non-BaseAgent class
    (agent() as (ctor: new (...args: unknown[]) => unknown) => void)(AgentWithoutBaseAgent);
  }).toThrowError(
    'Invalid agent declaration: `AgentWithoutBaseAgent` must extend `BaseAgent` to be decorated with @agent()',
  );
});

test('Agent with $ in method name is rejected', async () => {
  await expect(async () => {
    await import('./agentWithDollarInMethodName');
  }).rejects.toThrowError(
    "Schema generation failed for agent class AgentWithDollarInMethodName. Invalid method name `foo$`: cannot contain '$'",
  );
});

test('Agent with reserved method name is rejected', async () => {
  await expect(async () => {
    await import('./agentWithReservedMethodName');
  }).rejects.toThrowError(
    'Schema generation failed for agent class AgentWithReservedMethodName. Invalid method name `initialize`: reserved method name',
  );
});

test('Agent method with empty tuple return type is rejected', async () => {
  await expect(async () => {
    await import('./agentWithEmptyTuple');
  }).rejects.toThrowError(
    'Schema generation failed for agent class AgentWithEmptyTuple. Failed to construct output schema for method mysteriousArray with return type undefined: Empty tuple types are not supported',
  );
});

test('Agent with with unsatisfied http mount is rejected', async () => {
  await expect(async () => {
    await import('./agentWithInvalidHttpMount1');
  }).rejects.toThrowError(
    "Agent constructor variable 'bar' is not provided by the HTTP mount path.",
  );
});

test('Agent with with http mount variable bound to Principal is rejected', async () => {
  await expect(async () => {
    await import('./agentWithInvalidHttpMount2');
  }).rejects.toThrowError(
    "HTTP mount path variable 'bar' cannot be used for constructor parameters of type 'Principal'",
  );
});

test('Agent with with http mount variable bound to UnstructuredBinary is rejected', async () => {
  await expect(async () => {
    await import('./agentWithInvalidHttpMount3');
  }).rejects.toThrowError(
    "HTTP mount path variable 'bar' cannot be used for constructor parameters of type 'UnstructuredBinary'",
  );
});

test('Agent with with http mount variable with catch-all variable', async () => {
  await expect(async () => {
    await import('./agentWithInvalidHttpMount4');
  }).rejects.toThrowError(
    "HTTP mount for agent 'AgentWithInvalidHttpMount4' cannot contain catch-all path variable 'filePath'",
  );
});

test('Agent with with http endpoint query variables referring to Principal is rejected', async () => {
  await expect(async () => {
    await import('./agentWithInvalidHttpEndpoint1');
  }).rejects.toThrowError(
    "HTTP endpoint query variable 'user' cannot be used for parameters of type 'Principal'",
  );
});

test('Agent with with http endpoint path variables referring to Principal is rejected', async () => {
  await expect(async () => {
    await import('./agentWithInvalidHttpEndpoint2');
  }).rejects.toThrowError(
    "HTTP endpoint path variable 'user' cannot be used for parameters of type 'Principal'",
  );
});

function getAnalysedTypeInFun1(
  parameterName: string,
): Either.Either<AnalysedType.AnalysedType, string> {
  const type = fun1Params?.get(parameterName)!;
  return typeMapper(type, TypeScope.method('fun1', parameterName, type.optional));
}

export class UndecoratedAgent extends BaseAgent {
  async foo(): Promise<AgentId> {
    return this.getId();
  }
}
