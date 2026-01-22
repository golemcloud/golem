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

  foo(name: string): Promise<string> {
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

  foo(name: string): Promise<string> {
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
    const client1 = AgentWithPrincipalAutoInjection1.get(name);
    await client1.foo(name);
    const client2 = AgentWithPrincipalAutoInjection2.get(name, 1, 'baz');
    return client2.foo(name);
  }
}
