import {
    BaseAgent,
    agent,
} from '@golemcloud/golem-ts-sdk';

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
