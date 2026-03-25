import {
    BaseAgent,
    agent,
    prompt,
    description,
} from '@golemcloud/golem-ts-sdk';

@agent()
class SelfRpcAgent extends BaseAgent {
    private readonly name: string;
    private value: number = 0;

    constructor(name: string) {
        super()
        this.name = name;
    }

    async doWork(): Promise<void> {
        return
    }

    async selfRpc(): Promise<void> {
      return SelfRpcAgent.get(this.name).doWork()
    }
}
