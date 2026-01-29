# Golem TypeScript SDK

```ts
import { BaseAgent, agent, prompt, description } from '@golemcloud/golem-ts-sdk';

@agent()
class CounterAgent extends BaseAgent {
  private readonly name: string;
  private value: number = 0;

  constructor(name: string) {
    super();
    this.name = name;
  }

  @prompt('Increase the count by one')
  @description('Increases the count by one and returns the new value')
  async increment(): Promise<number> {
    this.value += 1;
    return this.value;
  }
}
```
