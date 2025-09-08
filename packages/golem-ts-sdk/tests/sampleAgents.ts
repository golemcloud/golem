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
import { FooBar, UnionOfLiterals } from './testTypes';

@agent()
class WeatherAgent extends BaseAgent {
  constructor(readonly input: string) {
    super();
    this.input = input;
  }

  async getWeather(location: string): Types.PromiseType {
    return Promise.resolve(`Weather in ${location} is sunny!`);
  }

  async getWeatherV2(data: { value: number; data: string }): Types.PromiseType {
    return Promise.resolve(`Weather in ${data.data} is sunny!`);
  }

  async getWeatherV3(param2: CustomData): Types.PromiseType {
    return Promise.resolve(`Weather in ${param2.data} is sunny!`);
  }
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
  ): Types.PromiseType {
    return Promise.resolve(`Weather for ${location} is sunny!`);
  }
}
