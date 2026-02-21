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

import {
  agent,
  BaseAgent,
  MultimodalAdvanced,
  UnstructuredBinary,
  UnstructuredText,
  Result,
  Multimodal,
  endpoint,
  Principal,
  createWebhook,
} from '../src';
import * as Types from './testTypes';
import {
  EitherX,
  EitherY,
  EitherZ,
  ObjectWithUnionWithUndefined1,
  ObjectWithUnionWithUndefined2,
  ObjectWithUnionWithUndefined3,
  ObjectWithUnionWithUndefined4,
  UnionWithLiterals,
  UnionType,
  TaggedUnion,
  UnionWithOnlyLiterals,
  ResultTypeNonExact,
  ResultTypeExactBoth,
  ResultTypeNonExact2,
  ObjectWithOption,
  InterfaceWithUnionWithUndefined1,
  InterfaceWithUnionWithUndefined2,
  InterfaceWithUnionWithUndefined3,
  InterfaceWithUnionWithUndefined4,
  InterfaceWithOption,
  ResultTypeNonExact3,
} from './testTypes';
import { describe } from 'vitest';

@agent()
export class FooAgent extends BaseAgent {
  constructor(readonly input: string) {
    super();
    this.input = input;
  }

  async fun1(param: string): Types.PromiseType {
    return Promise.resolve(`Weather in ${param} is sunny!`);
  }

  async fun2(param: { value: number; data: string }): Types.PromiseType {
    return Promise.resolve(`Weather in ${param.data} is sunny!`);
  }

  async fun3(param: CustomData): Types.PromiseType {
    return Promise.resolve(`Weather in ${param.data} is sunny!`);
  }

  async fun4(param: CustomData) {
    return;
  }

  fun5 = (param: string) => {
    return Promise.resolve(`Weather in ${param} is sunny!`);
  };

  fun6 = (param: string) => {
    return;
  };

  fun7 = async (
    param1: string | number | null,
    param2: ObjectWithUnionWithUndefined1,
    param3: ObjectWithUnionWithUndefined2,
    param4: ObjectWithUnionWithUndefined3,
    param5: ObjectWithUnionWithUndefined4,
    param6: string | undefined,
    param7: UnionType | undefined,
  ) => {
    const concatenatedResult = {
      param1: param1,
      param2: param2.a,
      param3: param3.a,
      param4: param4.a,
      param5: param5.a,
      param6: param6,
      param7: param7,
    };

    return Promise.resolve(concatenatedResult);
  };

  async fun8(a: UnionWithLiterals): Promise<UnionWithLiterals> {
    return a;
  }

  async fun9(param: TaggedUnion): Promise<TaggedUnion> {
    return param;
  }

  // may be rename UnstructuredText to TextInput. too much
  async fun10(param: UnionWithOnlyLiterals): Promise<UnionWithOnlyLiterals> {
    return param;
  }

  async fun11(param: ResultTypeExactBoth): Promise<ResultTypeExactBoth> {
    return param;
  }

  async fun12(param: ResultTypeNonExact): Promise<ResultTypeNonExact> {
    return param;
  }

  async fun13(param: ResultTypeNonExact2): Promise<ResultTypeNonExact2> {
    return param;
  }

  // Ensuring remote call variants compiles
  async fun14(
    testInterfaceType: Types.TestInterfaceType,
    optionalStringType: string | null,
    optionalUnionType: UnionType | null,
    unstructuredText: UnstructuredText,
    unstructuredTextWithLanguageCode: UnstructuredText<['en', 'de']>,
    unstructuredBinary: UnstructuredBinary<['application/json']>,
  ): Promise<void> {
    const remoteClient = BarAgent.get(
      testInterfaceType,
      optionalStringType,
      optionalUnionType,
      unstructuredText,
      unstructuredTextWithLanguageCode,
      unstructuredBinary,
    );

    await remoteClient.fun2('foo');

    await remoteClient.fun2.trigger('foo');

    await remoteClient.fun2.schedule({ seconds: 50000n, nanoseconds: 0 }, 'foo');
  }

  async fun15(param: UnstructuredText): Promise<UnstructuredText> {
    return param;
  }

  async fun16(param: UnstructuredText<['en', 'de']>): Promise<UnstructuredText<['en', 'de']>> {
    return param;
  }

  async fun17(
    param: UnstructuredBinary<['application/json']>,
  ): Promise<UnstructuredBinary<['application/json']>> {
    return param;
  }

  async fun18(param: MultimodalAdvanced<TextOrImage>): Promise<MultimodalAdvanced<TextOrImage>> {
    return param;
  }

  async fun19(param: Uint8Array): Promise<Uint8Array> {
    return param;
  }

  async fun20(param: Uint16Array): Promise<Uint16Array> {
    return param;
  }

  async fun21(param: Float32Array): Promise<Float32Array> {
    return param;
  }

  async fun22(param: BigInt64Array): Promise<BigInt64Array> {
    return param;
  }

  async fun23(param: BigUint64Array): Promise<BigUint64Array> {
    return param;
  }

  async fun24(param: Int8Array): Promise<Int8Array> {
    return param;
  }

  async fun25(param: Int16Array): Promise<Int16Array> {
    return param;
  }

  async fun26(param: Int32Array): Promise<Int32Array> {
    return param;
  }

  async fun27(param: Uint32Array): Promise<Uint32Array> {
    return param;
  }

  async fun28(param: Float64Array): Promise<Float64Array> {
    return param;
  }

  async fun29(param: BigInt64Array): Promise<BigInt64Array> {
    return param;
  }

  async fun30(param: Result<boolean, string>): Promise<Result<boolean, string>> {
    return param;
  }

  async fun31(param: MyResult): Promise<MyResult> {
    return param;
  }

  // Result types with void
  async fun32(param: string): Promise<Result<void, string>> {
    return Result.ok(undefined);
  }

  async fun33(param: string): Promise<Result<string, void>> {
    return Result.err(undefined);
  }

  async fun34(param: string): Promise<Result<void, void>> {
    return Result.ok(undefined);
  }

  fun35(param: string): Result<void, string> {
    return Result.ok(undefined);
  }

  fun36(param: string): Result<string, void> {
    return Result.err(undefined);
  }

  fun37(param: string): Result<void, void> {
    return Result.ok(undefined);
  }

  fun38(param: Multimodal): Promise<Multimodal> {
    return Promise.resolve(param);
  }

  fun39(param: Multimodal): Multimodal {
    return param;
  }

  fun40(param: UnstructuredBinary): UnstructuredBinary {
    return param;
  }

  async fun41(
    required: string,
    optional?: number,
  ): Promise<{ required: string; optional?: number }> {
    return { required, optional };
  }

  async fun42(
    required: string,
    optional: number | undefined,
  ): Promise<{ required: string; optional?: number }> {
    return { required, optional };
  }

  async fun43(param: ResultTypeNonExact3): Promise<ResultTypeNonExact3> {
    return param;
  }

  async fun44(param: Result<void, void>): Promise<Result<void, void>> {
    return param;
  }

  async fun45(param: string): Promise<void> {
    return;
  }

  // Overridden methods should be  not be considered as agent methods
  // without override keyword
  loadSnapshot(bytes: Uint8Array): Promise<void> {
    return super.loadSnapshot(bytes);
  }

  // With override keyword
  override async saveSnapshot(): Promise<Uint8Array> {
    return super.saveSnapshot();
  }

  // Without override keyword, and existing as an arrow function
  getId = () => {
    return super.getId();
  };
}

export type MyResult = Result<boolean, string>;

export interface CustomData {
  data: string;
  value: number;
}

// Used in invoke.test.ts
@agent({ name: 'my-complex-agent' })
class BarAgent extends BaseAgent {
  constructor(
    readonly testInterfaceType: Types.TestInterfaceType,
    readonly optionalStringType: string | null,
    readonly optionalUnionType: UnionType | null,
    readonly unstructuredText: UnstructuredText,
    readonly unstructuredTextWithLanguageCode: UnstructuredText<['en', 'de']>,
    readonly unstructuredBinary: UnstructuredBinary<['application/json']>,
  ) {
    super();
  }

  async fun0(
    complexType: Types.ObjectComplexType,
    unionType: Types.UnionType,
    unionComplexType: Types.UnionComplexType,
    numberType: Types.NumberType,
    stringType: Types.StringType,
    booleanType: Types.BooleanType,
    mapType: Types.MapType,
    tupleComplexType: Types.TupleComplexType,
    tupleType: Types.TupleType,
    listComplexType: Types.ListComplexType,
    objectType: Types.ObjectType,
    unionWithLiterals: UnionWithLiterals,
    textType: UnstructuredText,
    eitherXType: EitherX,
    eitherYType: EitherY,
    eitherZType: EitherZ,
    unionWithNull: string | number | null,
    objectWithUnionWithUndefined1: ObjectWithUnionWithUndefined1,
    objectWithUnionWithUndefined2: ObjectWithUnionWithUndefined2,
    objectWithUnionWithUndefined3: ObjectWithUnionWithUndefined3,
    objectWithUnionWithUndefined4: ObjectWithUnionWithUndefined4,
    objectWithOption: ObjectWithOption,
    interfaceWithUnionWithUndefined1: InterfaceWithUnionWithUndefined1,
    interfaceWithUnionWithUndefined2: InterfaceWithUnionWithUndefined2,
    interfaceWithUnionWithUndefined3: InterfaceWithUnionWithUndefined3,
    interfaceWithUnionWithUndefined4: InterfaceWithUnionWithUndefined4,
    interfaceWithOption: InterfaceWithOption,
    optionalStringType: string | undefined,
    optionalUnionType: UnionType | undefined,
    taggedUnionType: TaggedUnion,
    unionWithOnlyLiterals: UnionWithOnlyLiterals,
    resultTypeExact: ResultTypeExactBoth,
    resultTypeNonExact: ResultTypeNonExact,
    resultTypeNonExact2: ResultTypeNonExact2,
    unstructuredText: UnstructuredText,
    unstructuredTextWithLanguageCode: UnstructuredText<['en', 'de']>,
    unstructuredBinary: UnstructuredBinary<['application/json']>,
    unstructuredBinaryWithMimeType: UnstructuredBinary<['application/json']>,
  ): Types.PromiseType {
    return Promise.resolve(`Weather for ${location} is sunny!`);
  }

  async fun3(text: string): Promise<Types.ObjectComplexType> {
    throw new Error('Unimplemented');
  }

  async fun4(text: string): Promise<Types.UnionType> {
    throw new Error('Unimplemented');
  }

  async fun5(text: string): Promise<Types.UnionComplexType> {
    throw new Error('Unimplemented');
  }

  async fun6(text: string): Promise<Types.NumberType> {
    throw new Error('Unimplemented');
  }

  async fun7(text: string): Promise<Types.StringType> {
    throw new Error('Unimplemented');
  }

  async fun8(text: string): Promise<Types.BooleanType> {
    throw new Error('Unimplemented');
  }

  async fun9(text: string): Promise<Types.MapType> {
    throw new Error('Unimplemented');
  }

  async fun10(text: string): Promise<Types.TupleComplexType> {
    throw new Error('Unimplemented');
  }

  async fun11(text: string): Promise<Types.TupleType> {
    throw new Error('Unimplemented');
  }

  async fun12(text: string): Promise<Types.ListComplexType> {
    throw new Error('Unimplemented');
  }

  async fun13(text: string): Promise<Types.ObjectType> {
    throw new Error('Unimplemented');
  }

  async fun14(text: string): Promise<Types.UnionWithLiterals> {
    throw new Error('Unimplemented');
  }

  async fun15(text: string): Promise<void> {
    throw new Error('Unimplemented');
  }

  async fun16(text: string): Promise<null> {
    throw new Error('Unimplemented');
  }

  async fun17(text: string): Promise<undefined> {
    throw new Error('Unimplemented');
  }

  async fun18(text: string): Promise<UnstructuredText<['en', 'de']>> {
    throw new Error('Unimplemented');
  }

  async fun19(text: string): Promise<UnstructuredBinary<['application/json']>> {
    throw new Error('Unimplemented');
  }

  async fun1(text: string): Promise<EitherX> {
    return { ok: 'hello' };
  }

  async fun2(text: string): Promise<EitherY> {
    return {
      tag: 'okay',
      val: 'hello',
    };
  }

  async fun20(text: string): Promise<EitherZ> {
    return {
      tag: 'okay',
      val: 'hello',
    };
  }

  async fun21(text: string) {
    console.log('Hello World');
  }

  fun22 = (text: string) => {
    console.log('Hello World');
  };

  async fun23(
    multimodalInput: MultimodalAdvanced<TextOrImage>,
  ): Promise<MultimodalAdvanced<TextOrImage>> {
    return multimodalInput;
  }

  async fun24(multimodalInput: Multimodal): Promise<Multimodal> {
    return multimodalInput;
  }
}

export type TextOrImage =
  | { tag: 'text'; val: string }
  | { tag: 'image'; val: Uint8Array }
  | { tag: 'un-text'; val: UnstructuredText }
  | { tag: 'un-binary'; val: UnstructuredBinary<['application/json']> };

@agent({ mode: 'ephemeral' })
class EphemeralAgent extends BaseAgent {
  constructor(readonly input: string) {
    super();
    this.input = input;
  }

  async greet(name: string): Promise<string> {
    return Promise.resolve(`Hello, ${name}!`);
  }
}

@agent({
  mount: '/chats/{agent-type}',
})
class SimpleHttpAgent extends BaseAgent {
  constructor() {
    super();
  }

  @endpoint({ get: '/greet/{name}' })
  async greet(name: string): Promise<string> {
    return Promise.resolve(`Hello, ${name}!`);
  }
}

@agent({
  mount: '/chats/{agent-type}/{foo}/{bar}',
  cors: ['https://app.acme.com', 'https://staging.acme.com'],
  auth: true,
  webhookSuffix: '/{agent-type}/events/{foo}/{bar}',
  phantom: true,
})
class ComplexHttpAgent extends BaseAgent {
  constructor(
    readonly foo: string,
    readonly bar: string,
  ) {
    super();
  }

  @endpoint({ get: '/greet?l={location}&n={name}' })
  async greet(location: string, name: string): Promise<string> {
    return Promise.resolve(`Hello, ${name}!`);
  }

  // Multiple endpoint decorators
  @endpoint({ get: '/greet?l={location}&n={name}' })
  @endpoint({
    get: '/greet?lx={location}&nm={name}',
    cors: ['*'],
    auth: true,
    headers: { 'X-Foo': 'location', 'X-Bar': 'name' },
  })

  // Endpoint with custom http method
  @endpoint({
    custom: { method: 'patch', path: '/greet?l={location}&n={name}' },
  })
  async greetCustom(location: string, name: string): Promise<string> {
    return Promise.resolve(`Hello, ${name}!`);
  }

  // Endpoint with POST method with leftover parameters
  // The 'name' parameter is expected to be in the request body and hence should be a valid endpoint definition
  @endpoint({ post: '/greet?l={location}' })
  async greetPost(location: string, name: string): Promise<string> {
    return Promise.resolve(`Hello, ${name}!`);
  }

  // Endpoint with catch-all var
  @endpoint({ get: '/greet/{name}/{*filePath}' })
  async catchAllFun(name: string, filePath: string): Promise<string> {
    return Promise.resolve(`Hello, ${name}!`);
  }

  // Endpoint with just root path
  @endpoint({ get: '/' })
  async rootPathFun() {}
}

@agent()
class WebhookAgent extends BaseAgent {
  constructor(
    readonly foo: string,
    readonly bar: string,
  ) {
    super();
  }

  async greet(name: string): Promise<string> {
    const webhook = createWebhook();
    console.log(webhook.getUrl());
    const result = await webhook;
    return Promise.resolve(result.json<string>());
  }
}

// If this class is decorated with agent, it will fail
// This is kept here to ensure that any internal user class is not part of metadata generation.
// See package.json for metadata generation command.
class InternalClass {
  async fun1(input: string): Promise<Iterator<string>> {
    const array = ['a', 'b', 'c'];
    return array[Symbol.iterator]();
  }
}

@agent()
class SingletonAgent extends BaseAgent {
  constructor() {
    super();
  }

  test(): string {
    return 'test';
  }
}
