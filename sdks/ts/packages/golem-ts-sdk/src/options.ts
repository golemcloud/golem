import { AgentMode } from 'golem:agent/common';

type AllowedPattern = string;

export type AgentDecoratorOptions = {
  name?: string;
  mode?: AgentMode;
  mount?: string;
  cors?: AllowedPattern[];
  auth?: boolean;
  headers?: Record<string, string>;
  webhook?: string;
};
