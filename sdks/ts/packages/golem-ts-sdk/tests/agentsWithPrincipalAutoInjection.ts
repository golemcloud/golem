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
import { Principal } from 'golem:agent/common';

@agent()
export class AgentWithPrincipalAutoInjection1 extends BaseAgent {
  // principal at the end of the constructor
  constructor(
    name: string,
    readonly principal: Principal,
  ) {
    super();
  }

  foo(name: string, principal: Principal): Promise<string> {
    return Promise.resolve(name);
  }
}

@agent()
export class AgentWithPrincipalAutoInjection2 extends BaseAgent {
  // Principal in between other parameters in constructor
  constructor(
    readonly name: string,
    readonly text1: number,
    readonly principal: Principal,
    readonly text: string,
  ) {
    super();
  }

  foo(name: string, num: number, principal: Principal, text: string): Promise<string> {
    return Promise.resolve(name);
  }
}

@agent()
export class AgentWithPrincipalAutoInjection3 extends BaseAgent {
  // Principal in between other parameters in constructor
  constructor(
    readonly name: string,
    readonly text1: number,
    readonly principal: Principal,
    readonly text?: string,
  ) {
    super();
  }

  foo(name: string, num: number, text?: string): Promise<string> {
    return Promise.resolve(name);
  }
}

@agent()
export class AgentWithPrincipalAutoInjection4 extends BaseAgent {
  // Principal in between other parameters in constructor
  constructor(
    readonly name: string,
    readonly num: number,
    readonly principal: Principal,
    readonly text: string | null,
  ) {
    super();
  }

  foo(name: string, num: number, principal: Principal, text: string | null): Promise<string> {
    return Promise.resolve(name);
  }
}

@agent()
export class AgentWithPrincipalAutoInjection5 extends BaseAgent {
  // Principal in between other parameters in constructor
  constructor(
    readonly name: string,
    readonly text1: number,
    readonly principal: Principal,
    readonly text: string | undefined,
  ) {
    super();
  }

  foo(
    name: string,
    text1: number,
    principal: Principal,
    text: string | undefined,
  ): Promise<string> {
    return Promise.resolve(name);
  }

  // RPC calls to these functions are tested ensure
  // elimination of Principal doesn't affect other methods with and without optional/union-with-undefined parameters
  fooWithoutPrincipal1(name: string, num: number): Promise<string> {
    return Promise.resolve(name);
  }

  fooWithoutPrincipal2(name: string, num: number, text: string): Promise<string> {
    return Promise.resolve(name);
  }

  fooWithoutPrincipal3(name: string, num: number, text?: string): Promise<string> {
    return Promise.resolve(name);
  }

  fooWithoutPrincipal4(name: string, num: number, text1?: string, text2?: string): Promise<string> {
    return Promise.resolve(name);
  }

  fooWithoutPrincipal5(name: string, num: number, text: string | undefined): Promise<string> {
    return Promise.resolve(name);
  }
}

@agent()
export class RemoteAgentWithPrincipal extends BaseAgent {
  constructor(
    name: string,
    readonly principal: Principal,
  ) {
    super();
  }

  async foo(name: string): Promise<string> {
    // Handles constructor and method with `Principal` as the last parameter
    await AgentWithPrincipalAutoInjection1.get(name).foo(name);

    // Handles constructor and method with `Principal` in between other parameters
    await AgentWithPrincipalAutoInjection2.get(name, 1, 'required').foo(name, 1, 'required');

    // Handles constructor and method with `Principal` in between other parameters that are optional with `?`
    await AgentWithPrincipalAutoInjection3.get(name, 1, 'optional').foo(name, 1, 'optional');
    await AgentWithPrincipalAutoInjection3.get(name, 1).foo(name, 1);

    // Handles constructor and method with `Principal` in between other parameters that can be null
    await AgentWithPrincipalAutoInjection4.get(name, 1, null).foo(name, 1, null);
    await AgentWithPrincipalAutoInjection4.get(name, 1, 'not-null').foo(name, 1, 'no-null');

    // Handles constructor and method with `Principal` in between other parameters that can be undefined
    await AgentWithPrincipalAutoInjection5.get(name, 1, undefined).foo(name, 1, undefined);
    await AgentWithPrincipalAutoInjection5.get(name, 1, 'not-undefined').foo(
      name,
      1,
      'not-undefined',
    );

    await AgentWithPrincipalAutoInjection5.get(name, 1, 'not-undefined').fooWithoutPrincipal1(
      'name',
      1,
    );

    await AgentWithPrincipalAutoInjection5.get(name, 1, 'not-undefined').fooWithoutPrincipal2(
      'foo',
      1,
      'bar',
    );

    await AgentWithPrincipalAutoInjection5.get(name, 1, 'not-undefined').fooWithoutPrincipal3(
      'foo',
      1,
    );

    await AgentWithPrincipalAutoInjection5.get(name, 1, 'not-undefined').fooWithoutPrincipal3(
      'foo',
      1,
      'bar',
    );

    await AgentWithPrincipalAutoInjection5.get(name, 1, 'not-undefined').fooWithoutPrincipal4(
      'foo',
      1,
      undefined,
    );

    await AgentWithPrincipalAutoInjection5.get(name, 1, 'not-undefined').fooWithoutPrincipal4(
      'foo',
      1,
      'bar',
      'baz',
    );

    await AgentWithPrincipalAutoInjection5.get(name, 1, 'not-undefined').fooWithoutPrincipal5(
      'foo',
      1,
      'bar',
    );

    return 'finished';
  }
}
