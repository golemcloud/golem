import { agent, BaseAgent } from '../src';
import { Principal } from 'golem:agent/common';

@agent()
export class AgentWithPrincipalAutoInjection extends BaseAgent {

  constructor(name: string, readonly principal: Principal) {
    super();
  }

  foo(name: string,): Promise<string> {
    return Promise.resolve(name);
  }
}


@agent()
export class RemoteAgentWithPrincipal extends BaseAgent {
  constructor(name: string, readonly principal: Principal) {
    super();
  }

  foo(name: string,): Promise<string> {
    const client = AgentWithPrincipalAutoInjection.get(name);
    return client.foo(name);
  }
}