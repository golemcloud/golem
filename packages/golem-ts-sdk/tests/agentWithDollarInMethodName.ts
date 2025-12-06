import { agent, BaseAgent } from '../src';

@agent()
export class AgentWithDollarInMethodName extends BaseAgent {
  async foo$(): Promise<void> {
    return;
  }
}
