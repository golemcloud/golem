import { agent, BaseAgent, Config, Secret } from "@golemcloud/golem-ts-sdk";

type AliasedNestedConfig = {
  c: number;
};

type AgentConfig = {
  foo: number;
  bar: string;
  secret: Secret<boolean>;
  nested: {
    nestedSecret: Secret<number>;
    a: boolean;
    b: number[];
  };
  aliasedNested: AliasedNestedConfig;
};

@agent()
export class ConfigAgent extends BaseAgent {
  constructor(readonly config: Config<AgentConfig>) {
    super();
  }
}
