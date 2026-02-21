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

import { agent, BaseAgent, endpoint } from '../src';
import { Principal } from 'golem:agent/common';

@agent({
  mount: '/chats/{agent-type}/{foo}',
})
class AgentWithInvalidHttpEndpoint1 extends BaseAgent {
  constructor(readonly foo: string) {
    super();
  }

  // 'user' parameter of type Principal cannot be passed via query parameter
  @endpoint({ post: '/greet/{name}?u={user}' })
  async myPrincipal(name: string, user: Principal): Promise<string> {
    return Promise.resolve(`Hello, ${name}!`);
  }
}
