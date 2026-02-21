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

import { agent, BaseAgent } from '../src';
import * as Types from './testTypes';
import { RecursiveType } from './testTypes';

// !!! This is a set of invalid agents
// Note that this file is not (and shouldn't be) "imported" anywhere else directly
// as decorators will fail and none of the tests will run.

@agent()
class InvalidAgent extends BaseAgent {
  constructor(readonly input: Date) {
    super();
    this.input = input;
  }

  async fun1(
    date: Date,
    regExp: RegExp,
    iterator: Iterator<string>,
    iterable: Iterable<string>,
    asyncIterator: AsyncIterator<string>,
    asyncIterable: AsyncIterable<string>,
    any: any,
    string: String,
    boolean: Boolean,
    symbol: Symbol,
    number: Number,
    bigint: BigInt,
    voidParam: void,
    undefined: undefined,
    nullParam: null,
    unionWithKeyWord: 'foo' | 'bar' | 'ok',
    resultTypeInvalid1: Types.ResultTypeInvalid1,
    resultTypeInvalid2: Types.ResultTypeInvalid2,
    resultTypeInvalid3: Types.ResultTypeInvalid3,
  ): Types.PromiseType {
    return Promise.resolve(`Weather is sunny!`);
  }

  async fun2(input: string): Promise<Date> {
    return new Date();
  }

  async fun3(input: string): Promise<Iterator<string>> {
    const array = ['a', 'b', 'c'];
    return array[Symbol.iterator]();
  }

  async fun4(input: string): Promise<Iterable<string>> {
    const array = ['a', 'b', 'c'];
    return array;
  }

  async fun5(input: string): Promise<AsyncIterator<string>> {
    throw new Error('Unimplemented');
  }

  async fun6(input: string): Promise<AsyncIterable<string>> {
    throw new Error('Unimplemented');
  }

  async fun7(input: string): Promise<any> {
    throw new Error('Unimplemented');
  }

  async fun8(input: string): Promise<String> {
    throw new Error('Unimplemented');
  }

  async fun9(input: string): Promise<Number> {
    throw new Error('Unimplemented');
  }

  async fun10(input: string): Promise<Boolean> {
    throw new Error('Unimplemented');
  }

  async fun11(input: string): Promise<Symbol> {
    throw new Error('Unimplemented');
  }

  async fun12(input: string): Promise<BigInt> {
    throw new Error('Unimplemented');
  }

  async fun13(input: string): Promise<Object> {
    throw new Error('Unimplemented');
  }

  async fun14(input: string): Promise<RecursiveType> {
    throw new Error('Unimplemented');
  }
}
