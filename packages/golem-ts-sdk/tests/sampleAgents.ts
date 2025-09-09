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

import { agent, BaseAgent, UnstructuredText } from '../src';
import * as Types from './testTypes';
import { EitherX, EitherY, EitherZ, UnionOfLiterals } from './testTypes';

@agent()
class WeatherAgent extends BaseAgent {
  constructor(readonly input: string) {
    super();
    this.input = input;
  }

  async fun1(location: string): Types.PromiseType {
    return Promise.resolve(`Weather in ${location} is sunny!`);
  }

  async fun2(data: { value: number; data: string }): Types.PromiseType {
    return Promise.resolve(`Weather in ${data.data} is sunny!`);
  }

  async fun3(param2: CustomData): Types.PromiseType {
    return Promise.resolve(`Weather in ${param2.data} is sunny!`);
  }

  async fun4(location: CustomData) {
    return;
  }

  fun5 = (location: string) => {
    return Promise.resolve(`Weather in ${location} is sunny!`);
  };

  fun6 = (location: string) => {
    return;
  };
}

export interface CustomData {
  data: string;
  value: number;
}

@agent()
class AssistantAgent extends BaseAgent {
  constructor(readonly testInterfaceType: Types.TestInterfaceType) {
    super();
    this.testInterfaceType = testInterfaceType;
  }

  async getWeather(
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
    UnionOfLiterals: UnionOfLiterals,
    voidType: void,
    nullType: null,
    undefinedType: undefined,
    textType: UnstructuredText,
    eitherXType: EitherX,
    eitherYType: EitherY,
    eitherZType: EitherZ,
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

  async fun14(text: string): Promise<Types.UnionOfLiterals> {
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

  async fun18(text: string): Promise<UnstructuredText> {
    throw new Error('Unimplemented');
  }

  async fun1(text: string): Promise<EitherX> {
    return { ok: 'hello' };
  }

  async fun2(text: string): Promise<EitherY> {
    return { tag: 'ok', val: 'hello' };
  }

  async fun19(text: string): Promise<EitherZ> {
    return { tag: 'ok', val: 'hello' };
  }

  async fun20(text: string) {
    console.log('Hello World');
  }

  fun21 = (text: string) => {
    console.log('Hello World');
  };
}
