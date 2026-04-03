import {
    BaseAgent,
    agent,
    awaitPromise,
} from '@golemcloud/golem-ts-sdk';
import { createPromise, PromiseId } from 'golem:api/host@1.5.0';
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

@agent()
class TsCounter extends BaseAgent {
    private readonly name: string;
    private count: number = 0;

    constructor(name: string) {
        super();
        this.name = name;
    }

    incBy(value: number): void {
        this.count += value;
    }

    getValue(): number {
        return this.count;
    }

    async slowIncBy(value: number, delayMs: number): Promise<void> {
        await sleep(delayMs);
        this.count += value;
    }
}

@agent()
class TsCancelTester extends BaseAgent {
    private readonly name: string;

    constructor(name: string) {
        super();
        this.name = name;
    }

    /**
     * Starts an abortable RPC call to TsCounter.slowIncBy,
     * aborts after a short delay, and returns "aborted" if AbortError caught.
     */
    async testAbortBeforeAwait(counterName: string): Promise<string> {
        const counter = TsCounter.get(counterName);
        const controller = new AbortController();

        // Abort after 100ms — the slowIncBy takes 5000ms so it will still be pending
        setTimeout(() => controller.abort("cancelled by test"), 100);

        try {
            // Start abortable RPC to slowIncBy (5s delay so it's still pending when abort fires)
            await counter.slowIncBy.abortable(controller.signal, 1, 5000);
            return "unexpected:completed";
        } catch (e: any) {
            if (e === "cancelled by test" || e?.name === "AbortError") {
                return "aborted";
            }
            return `unexpected:error:${String(e)}`;
        }
    }

    /**
     * Starts an abortable RPC call, awaits completion, then aborts (should be no-op).
     * Returns the counter value.
     */
    async testAbortAfterComplete(counterName: string): Promise<number> {
        const counter = TsCounter.get(counterName);
        const controller = new AbortController();

        // Call incBy via abortable - this should complete quickly
        await counter.incBy.abortable(controller.signal, 5);

        // Abort after completion - should be a no-op
        controller.abort("late abort");

        // Verify counter value
        return await counter.getValue();
    }
}

@agent()
class TsBlockingAgent extends BaseAgent {
    private readonly name: string;
    private storedPromiseId: PromiseId | undefined;
    private completedCount: number = 0;

    constructor(name: string) {
        super();
        this.name = name;
    }

    prepareBlock(): PromiseId {
        this.storedPromiseId = createPromise();
        return this.storedPromiseId;
    }

    async doBlock(): Promise<number> {
        if (!this.storedPromiseId) {
            throw new Error("prepareBlock() must be called first");
        }
        await awaitPromise(this.storedPromiseId);
        this.completedCount += 1;
        return this.completedCount;
    }

    getCompletedCount(): number {
        return this.completedCount;
    }
}

@agent()
class TsCancelCallerAgent extends BaseAgent {
    private readonly name: string;
    private lastOutcome: string = "none";

    constructor(name: string) {
        super();
        this.name = name;
    }

    async callAndAbort(targetName: string, delayMs: number): Promise<string> {
        const blocker = TsBlockingAgent.get(targetName);
        const controller = new AbortController();

        const timer = setTimeout(() => controller.abort("cancelled by test"), delayMs);

        try {
            await blocker.doBlock.abortable(controller.signal);
            this.lastOutcome = "unexpected:completed";
        } catch (e: any) {
            if (e === "cancelled by test" || e?.name === "AbortError") {
                this.lastOutcome = "aborted";
            } else {
                this.lastOutcome = `unexpected:error:${String(e)}`;
            }
        } finally {
            clearTimeout(timer);
        }
        return this.lastOutcome;
    }

    getLastOutcome(): string {
        return this.lastOutcome;
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
