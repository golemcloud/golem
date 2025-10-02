import {
    BaseAgent,
    agent,
    PromiseId,
    createPromise,
    awaitPromise
} from '@golemcloud/golem-ts-sdk';

@agent()
class PromiseAgent extends BaseAgent {
    private readonly name: string;
    private value: number = 0;

    constructor(name: string) {
        super()
        this.name = name;
    }

    async getPromise(): Promise<PromiseId> {
        return createPromise()
    }

    async awaitPromise(id: PromiseId): Promise<string> {
      const resultBytes = await awaitPromise(id)
      return new TextDecoder().decode(resultBytes)
    }
}
