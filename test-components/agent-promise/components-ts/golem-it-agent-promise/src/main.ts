import {
    BaseAgent,
    agent,
    PromiseId,
    createPromise,
    awaitPromise,
    fork,
    completePromise
} from '@golemcloud/golem-ts-sdk';

@agent()
class PromiseAgent extends BaseAgent {
    private readonly name: string;

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

    async forkAndSyncWithPromise(): Promise<string> {
      const promiseId = createPromise();
      const forkResult = fork();
      switch (forkResult.tag) {
        case "original":
          const result = await awaitPromise(promiseId);
          const string = new TextDecoder().decode(result);
          return string;
        case "forked":
          completePromise(promiseId, new TextEncoder().encode("Hello from forked agent!"));
          return "forked result";
      }
    }
}
