import { agent, BaseAgent } from '../src';

@agent()
export class AgentWithReservedMethodName extends BaseAgent {
  async initialize(): Promise<void> {
    return;
  }
}
