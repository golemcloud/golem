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

  foo(
    name: string,
    num: number,
    principal: Principal,
    text: string,
  ): Promise<string> {
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

  foo(
    name: string,
    num: number,
    principal: Principal,
    text?: string,
  ): Promise<string> {
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

  foo(
    name: string,
    num: number,
    principal: Principal,
    text: string | null,
  ): Promise<string> {
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
    await AgentWithPrincipalAutoInjection2.get(name, 1, 'required').foo(
      name,
      1,
      'required',
    );

    // Handles constructor and method with `Principal` in between other parameters that are optional with `?`
    await AgentWithPrincipalAutoInjection3.get(name, 1, 'optional').foo(
      name,
      1,
      'optional',
    );
    await AgentWithPrincipalAutoInjection3.get(name, 1).foo(name, 1);

    // Handles constructor and method with `Principal` in between other parameters that can be null
    await AgentWithPrincipalAutoInjection4.get(name, 1, null).foo(
      name,
      1,
      null,
    );
    await AgentWithPrincipalAutoInjection4.get(name, 1, 'not-null').foo(
      name,
      1,
      'no-null',
    );

    // Handles constructor and method with `Principal` in between other parameters that can be undefined
    await AgentWithPrincipalAutoInjection5.get(name, 1, undefined).foo(
      name,
      1,
      undefined,
    );
    return await AgentWithPrincipalAutoInjection5.get(
      name,
      1,
      'not-undefined',
    ).foo(name, 1, 'not-undefined');
  }
}
