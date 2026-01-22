import { agent, BaseAgent } from '../src';

@agent({
  mount: '/chats/{agent-type}/{foo}',
})
class AgentWithInvalidHttpMount extends BaseAgent {
  constructor(
    readonly foo: string,
    readonly bar: string,
    // baz is neither satisfied by the path variable or headers
    readonly baz: string,
  ) {
    super();
  }

  async greet(name: string): Promise<string> {
    return Promise.resolve(`Hello, ${name}!`);
  }
}
