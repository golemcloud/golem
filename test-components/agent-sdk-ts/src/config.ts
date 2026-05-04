import { agent, BaseAgent, Config, Secret } from "@golemcloud/golem-ts-sdk";

type AliasedNestedConfig = {
  c?: number;
};

type ConfigAgentConfig = {
  foo: number;
  bar: string;
  secret: Secret<string>;
  nested: {
    nestedSecret: Secret<number>;
    a: boolean;
    b: number[];
  };
  aliasedNested: AliasedNestedConfig;
};

@agent()
export class ConfigAgent extends BaseAgent {
  constructor(_name: string, readonly config: Config<ConfigAgentConfig>) {
    super();
  }

  echoLocalConfig(): string {
    const config = this.config.value;
    return JSON.stringify({
      foo: config.foo,
      bar: config.bar,
      secret: config.secret.get(),
      nested: {
        nestedSecret: config.nested.nestedSecret.get(),
        a: config.nested.a,
        b: config.nested.b,
      },
      aliasedNested: {
        c: config.aliasedNested.c
      }
    })
  }
}

type LocalConfigAgentConfig = {
  foo: number;
  bar: string;
  nested: {
    a: boolean;
    b: number[];
  };
  aliasedNested: AliasedNestedConfig;
};

@agent()
export class LocalConfigAgent extends BaseAgent {
  constructor(_name: string, readonly config: Config<LocalConfigAgentConfig>) {
    super();
  }

  echoLocalConfig(): string {
    const config = this.config.value;
    return JSON.stringify({
      foo: config.foo,
      bar: config.bar,
      nested: {
        a: config.nested.a,
        b: config.nested.b,
      },
      aliasedNested: {
        c: config.aliasedNested.c
      }
    })
  }
}

type ComplexSecret = {
  foo: string,
  bar: number
};

type SharedConfigAgentConfig = {
  secret: Secret<string>,
  complexSecret: Secret<ComplexSecret>
};

@agent()
export class SharedConfigAgent extends BaseAgent {
  constructor(_name: string, readonly config: Config<SharedConfigAgentConfig>) {
    super();
  }

  echoLocalConfig(): string {
    const config = this.config.value;
    return JSON.stringify({
      secret: config.secret.get(),
      complexSecret: config.complexSecret.get()
    })
  }
}

type LocalCasingSharedConfigAgentConfig = {
  secretPath: Secret<string>,
};

@agent()
export class LocalCasingSharedConfigAgent extends BaseAgent {
  constructor(_name: string, readonly config: Config<LocalCasingSharedConfigAgentConfig>) {
    super();
  }

  echoLocalConfig(): string {
    const config = this.config.value;
    return JSON.stringify({
      secretPath: config.secretPath.get(),
    })
  }
}

type RpcLocalConfigAgentConfig = {
  foo: number;
  nested_a?: boolean,
};

@agent()
export class RpcLocalConfigAgent extends BaseAgent {
  constructor(readonly name: string, readonly config: Config<RpcLocalConfigAgentConfig>) {
    super();
  }

  async echoLocalConfig(): Promise<string> {
    const config = this.config.value;
    let client = LocalConfigAgent.getWithConfig(
      this.name,
      {
        foo: config.foo,
        nested: {
          a: config.nested_a
        }
      }
    )
    return await client.echoLocalConfig()
  }
}
