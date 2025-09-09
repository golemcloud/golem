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

// This is a set of invalid agents
// Note that this file is not "imported" anywhere like `sampleAgents.ts`
// as decorators will fail and none of the tests will run

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
  ): Types.PromiseType {
    return Promise.resolve(`Weather in ${location} is sunny!`);
  }
}
