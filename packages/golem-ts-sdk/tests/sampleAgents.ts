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

import { agent, BaseAgent, UnstructuredBinary, UnstructuredText } from '../src';
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
} from './testTypes';
import { multimodal } from '../src/decorators';

@agent()
class FooAgent extends BaseAgent {
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
    unstructuredBinary: UnstructuredBinary,
  ): Promise<void> {
    const remoteClient = BarAgent.get(
      testInterfaceType,
      optionalStringType,
      optionalUnionType,
      unstructuredText,
      unstructuredTextWithLanguageCode,
      unstructuredBinary,
      unstructuredBinary,
    );

    await remoteClient.fun2('foo');

    await remoteClient.fun2.trigger('foo');

    await remoteClient.fun2.schedule(
      { seconds: 50000n, nanoseconds: 0 },
      'foo',
    );
  }

  async fun15(param: UnstructuredText): Promise<UnstructuredText> {
    return param;
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

export interface CustomData {
  data: string;
  value: number;
}

@agent('my-complex-agent')
class BarAgent extends BaseAgent {
  constructor(
    readonly testInterfaceType: Types.TestInterfaceType,
    readonly optionalStringType: string | null,
    readonly optionalUnionType: UnionType | null,
    readonly unstructuredText: UnstructuredText,
    readonly unstructuredTextWithLanguageCode: UnstructuredText<['en', 'de']>,
    readonly unstructuredBinary: UnstructuredBinary,
    readonly unstructuredBinaryWithMimeType: UnstructuredBinary<
      ['application/json']
    >,
  ) {
    super();
    this.testInterfaceType = testInterfaceType;
    this.optionalStringType = optionalStringType;
    this.optionalUnionType = optionalUnionType;
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
    optionalStringType: string | undefined,
    optionalUnionType: UnionType | undefined,
    taggedUnionType: TaggedUnion,
    unionWithOnlyLiterals: UnionWithOnlyLiterals,
    resultTypeExact: ResultTypeExactBoth,
    resultTypeNonExact: ResultTypeNonExact,
    resultTypeNonExact2: ResultTypeNonExact2,
    unstructuredText: UnstructuredText,
    unstructuredTextWithLanguageCode: UnstructuredText<['en', 'de']>,
    unstructuredBinary: UnstructuredBinary,
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

  @multimodal()
  async fun23(text: [string]): Promise<string> {
    return this.getId().value;
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
