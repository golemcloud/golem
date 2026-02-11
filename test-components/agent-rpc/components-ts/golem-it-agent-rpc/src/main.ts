import {
    BaseAgent,
    agent,
} from '@golemcloud/golem-ts-sdk';
import * as process from "node:process";

type EnvVar = {
    key: string,
    value: string
}

@agent()
class TestAgent extends BaseAgent {
    private readonly id: string;

    constructor(id: string) {
        super()
        this.id = id;
    }

    async run(n: number): Promise<number[]> {
        const ids = Array.from({length: n}, (_, i) => i);
        const chunks = arrayChunks(ids, 5);

        const result = [];
        for (const chunk of chunks) {
            console.log(`Processing chunk ${chunk}`);
            const promises = chunk.map(async id => await ChildAgent.get(id).process());
            result.push(...await Promise.all(promises));
        }
        return result;
    }

    async envVarTest(): Promise<{ parent: EnvVar[], child: EnvVar[] }> {
        const childAgent = ChildAgent.get(0);
        const child = await childAgent.envVars();
        const parent = Object.entries(process.env).map(([key, value]) => ({key, value: value ?? ''}));
        return {
            parent,
            child
        }
    }

    async longRpcCall(durationInMillis: number): Promise<void> {
      const childAgent = ChildAgent.get(1000);
      await childAgent.longRpcCall(durationInMillis);
    }
}

@agent()
class ChildAgent extends BaseAgent {
    private readonly id: number;

    constructor(id: number) {
        super()
        this.id = id;
    }

    async process(): Promise<number> {
        const sleepAmount = Math.random() * 1000 + 500;
        await sleep(sleepAmount);
        return this.id;
    }

    envVars(): EnvVar[] {
        return Object.entries(process.env).map(([key, value]) => ({key, value: value ?? ''}));
    }

    async longRpcCall(durationInMillis: number): Promise<void> {
      console.log(`Starting sleeping ${durationInMillis}ms`);
      await sleep(durationInMillis);
      console.log(`Finished sleeping ${durationInMillis}ms`);
    }
}

@agent()
class SimpleChildAgent extends BaseAgent {
    private readonly name: string;

    constructor(name: string) {
        super();
        this.name = name;
    }

    async value(): Promise<number> {
        return 1;
    }
}

function sleep(ms: number) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

function arrayChunks<T>(array: T[], chunkSize: number): T[][] {
    const chunks: T[][] = [];

    for (let i = 0; i < array.length; i += chunkSize) {
        chunks.push(array.slice(i, i + chunkSize));
    }

    return chunks;
}
