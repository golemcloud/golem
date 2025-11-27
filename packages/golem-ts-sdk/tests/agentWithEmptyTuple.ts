import { agent, BaseAgent } from '../src';

@agent()
export class AgentWithEmptyTuple extends BaseAgent {
  mysteriousArray(): [] {
    return []
  }
}
