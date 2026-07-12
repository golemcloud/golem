import { z } from 'zod';
import {
    defineAgent,
    method,
    s,
    clientFor,
    createPromise,
    awaitPromise,
} from '@golemcloud/golem-ts-sdk';
import type { PromiseId } from 'golem:api/host@1.5.0';
import * as process from 'node:process';

const EnvVar = z.object({ key: z.string(), value: z.string() });

// A `PromiseId` is a nested host record carrying bigints; declare it as an
// explicit Standard Schema so it can be returned by a method.
const PromiseIdSchema = z.object({
    agentId: z.object({
        componentId: z.object({
            uuid: z.object({ highBits: s.u64(), lowBits: s.u64() }),
        }),
        agentId: z.string(),
    }),
    oplogIdx: s.u64(),
});

export const ChildAgent = defineAgent({
    name: 'ChildAgent',
    id: { id: z.number() },
    methods: {
        process: method({ input: {}, returns: z.number() }),
        envVars: method({ input: {}, returns: z.array(EnvVar) }),
        longRpcCall: method({ input: { durationInMillis: z.number() }, returns: z.void() }),
    },
});

const childClient = clientFor(ChildAgent);

export const ChildAgentImpl = ChildAgent.implement({
    init: ({ id }) => ({ id: id.id }),
    methods: {
        async process() {
            const sleepAmount = Math.random() * 1000 + 500;
            await sleep(sleepAmount);
            return this.id;
        },
        envVars() {
            return Object.entries(process.env).map(([key, value]) => ({ key, value: value ?? '' }));
        },
        async longRpcCall({ durationInMillis }) {
            console.log(`Starting sleeping ${durationInMillis}ms`);
            await sleep(durationInMillis);
            console.log(`Finished sleeping ${durationInMillis}ms`);
        },
    },
});

export const TestAgent = defineAgent({
    name: 'TestAgent',
    id: { id: z.string() },
    methods: {
        run: method({ input: { n: z.number() }, returns: z.array(z.number()) }),
        envVarTest: method({
            input: {},
            returns: z.object({ parent: z.array(EnvVar), child: z.array(EnvVar) }),
        }),
        longRpcCall: method({ input: { durationInMillis: z.number() }, returns: z.void() }),
    },
});

export const TestAgentImpl = TestAgent.implement({
    init: ({ id }) => ({ id: id.id }),
    methods: {
        async run({ n }) {
            const ids = Array.from({ length: n }, (_, i) => i);
            const chunks = arrayChunks(ids, 5);

            const result: number[] = [];
            for (const chunk of chunks) {
                console.log(`Processing chunk ${chunk}`);
                const promises = chunk.map(async (id) => await childClient({ id }).process());
                result.push(...(await Promise.all(promises)));
            }
            return result;
        },
        async envVarTest() {
            const child = await childClient({ id: 0 }).envVars();
            const parent = Object.entries(process.env).map(([key, value]) => ({ key, value: value ?? '' }));
            return {
                parent,
                child,
            };
        },
        async longRpcCall({ durationInMillis }) {
            await childClient({ id: 1000 }).longRpcCall({ durationInMillis });
        },
    },
});

export const SimpleChildAgent = defineAgent({
    name: 'SimpleChildAgent',
    id: { name: z.string() },
    methods: {
        value: method({ input: {}, returns: z.number() }),
    },
});

export const SimpleChildAgentImpl = SimpleChildAgent.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        async value() {
            return 1;
        },
    },
});

export const SelfRpcAgent = defineAgent({
    name: 'SelfRpcAgent',
    id: { name: z.string() },
    methods: {
        doWork: method({ input: {}, returns: z.void() }),
        selfRpc: method({ input: {}, returns: z.void() }),
    },
});

const selfRpcClient = clientFor(SelfRpcAgent);

export const SelfRpcAgentImpl = SelfRpcAgent.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        async doWork() {
            return;
        },
        async selfRpc() {
            return selfRpcClient({ name: this.name }).doWork();
        },
    },
});

export const TsCounter = defineAgent({
    name: 'TsCounter',
    id: { name: z.string() },
    methods: {
        incBy: method({ input: { value: z.number() }, returns: z.void() }),
        getValue: method({ input: {}, returns: z.number() }),
        slowIncBy: method({ input: { value: z.number(), delayMs: z.number() }, returns: z.void() }),
    },
});

export const TsCounterImpl = TsCounter.implement({
    init: () => ({ count: 0 }),
    methods: {
        incBy({ value }) {
            this.count += value;
        },
        getValue() {
            return this.count;
        },
        async slowIncBy({ value, delayMs }) {
            await sleep(delayMs);
            this.count += value;
        },
    },
});

export const TsBlockingAgent = defineAgent({
    name: 'TsBlockingAgent',
    id: { name: z.string() },
    methods: {
        prepareBlock: method({ input: {}, returns: PromiseIdSchema }),
        doBlock: method({ input: {}, returns: z.number() }),
        getCompletedCount: method({ input: {}, returns: z.number() }),
    },
});

export const TsBlockingAgentImpl = TsBlockingAgent.implement({
    init: () => ({ storedPromiseId: undefined as PromiseId | undefined, completedCount: 0 }),
    methods: {
        prepareBlock() {
            const id = createPromise();
            this.storedPromiseId = id;
            return id;
        },
        async doBlock() {
            if (!this.storedPromiseId) {
                throw new Error('prepareBlock() must be called first');
            }
            await awaitPromise(this.storedPromiseId);
            this.completedCount += 1;
            return this.completedCount;
        },
        getCompletedCount() {
            return this.completedCount;
        },
    },
});

const tsCounterClient = clientFor(TsCounter);
const tsBlockingClient = clientFor(TsBlockingAgent);

export const TsCancelTester = defineAgent({
    name: 'TsCancelTester',
    id: { name: z.string() },
    methods: {
        testAbortBeforeAwait: method({ input: { counterName: z.string() }, returns: z.string() }),
        testAbortAfterComplete: method({ input: { counterName: z.string() }, returns: z.number() }),
    },
});

export const TsCancelTesterImpl = TsCancelTester.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        /**
         * Starts an abortable RPC call to TsCounter.slowIncBy, aborts after a
         * short delay, and returns "aborted" if the AbortError is caught.
         */
        async testAbortBeforeAwait({ counterName }) {
            const counter = tsCounterClient({ name: counterName });
            const controller = new AbortController();

            // Abort after 100ms — slowIncBy takes 5000ms so it is still pending.
            setTimeout(() => controller.abort('cancelled by test'), 100);

            try {
                await counter.slowIncBy(
                    { value: 1, delayMs: 5000 },
                    { signal: controller.signal },
                );
                return 'unexpected:completed';
            } catch (e: any) {
                if (e === 'cancelled by test' || e?.name === 'AbortError') {
                    return 'aborted';
                }
                return `unexpected:error:${String(e)}`;
            }
        },

        /**
         * Starts an abortable RPC call, awaits completion, then aborts (a no-op).
         * Returns the counter value.
         */
        async testAbortAfterComplete({ counterName }) {
            const counter = tsCounterClient({ name: counterName });
            const controller = new AbortController();

            // Completes quickly.
            await counter.incBy({ value: 5 }, { signal: controller.signal });

            // Abort after completion — a no-op.
            controller.abort('late abort');

            return await counter.getValue();
        },
    },
});

export const TsCancelCallerAgent = defineAgent({
    name: 'TsCancelCallerAgent',
    id: { name: z.string() },
    methods: {
        callAndAbort: method({
            input: { targetName: z.string(), delayMs: z.number() },
            returns: z.string(),
        }),
        getLastOutcome: method({ input: {}, returns: z.string() }),
    },
});

export const TsCancelCallerAgentImpl = TsCancelCallerAgent.implement({
    init: ({ id }) => ({ name: id.name, lastOutcome: 'none' }),
    methods: {
        async callAndAbort({ targetName, delayMs }) {
            const blocker = tsBlockingClient({ name: targetName });
            const controller = new AbortController();

            const timer = setTimeout(() => controller.abort('cancelled by test'), delayMs);

            try {
                await blocker.doBlock({ signal: controller.signal });
                this.lastOutcome = 'unexpected:completed';
            } catch (e: any) {
                if (e === 'cancelled by test' || e?.name === 'AbortError') {
                    this.lastOutcome = 'aborted';
                } else {
                    this.lastOutcome = `unexpected:error:${String(e)}`;
                }
            } finally {
                clearTimeout(timer);
            }
            return this.lastOutcome;
        },
        getLastOutcome() {
            return this.lastOutcome;
        },
    },
});

function sleep(ms: number) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

function arrayChunks<T>(array: T[], chunkSize: number): T[][] {
    const chunks: T[][] = [];

    for (let i = 0; i < array.length; i += chunkSize) {
        chunks.push(array.slice(i, i + chunkSize));
    }

    return chunks;
}
