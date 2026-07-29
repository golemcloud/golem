import { z } from 'zod';
import {
    defineAgent,
    method,
    s,
    createPromise,
    awaitPromise,
    fork,
    completePromise,
} from '@golemcloud/golem-ts-sdk';
import type { PromiseId } from 'golem:api/host@1.5.0';

// A `PromiseId` is a nested host record carrying bigints; declare it as an
// explicit Standard Schema so it can be returned / accepted by a method.
const PromiseIdSchema = z.object({
    agentId: z.object({
        componentId: z.object({
            uuid: z.object({ highBits: s.u64(), lowBits: s.u64() }),
        }),
        agentId: z.string(),
    }),
    oplogIdx: s.u64(),
});

export const PromiseAgent = defineAgent({
    name: 'PromiseAgent',
    id: { name: z.string() },
    methods: {
        getPromise: method({ input: {}, returns: PromiseIdSchema }),
        awaitPromise: method({ input: { id: PromiseIdSchema }, returns: z.string() }),
        forkAndSyncWithPromise: method({ input: {}, returns: z.string() }),
    },
});

export const PromiseAgentImpl = PromiseAgent.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        async getPromise() {
            return createPromise();
        },
        async awaitPromise({ id }) {
            const resultBytes = await awaitPromise(id as unknown as PromiseId);
            return new TextDecoder().decode(resultBytes);
        },
        async forkAndSyncWithPromise() {
            const promiseId = createPromise();
            const forkResult = fork();
            switch (forkResult.tag) {
                case 'original': {
                    const result = await awaitPromise(promiseId);
                    const string = new TextDecoder().decode(result);
                    return string;
                }
                case 'forked':
                    completePromise(promiseId, new TextEncoder().encode('Hello from forked agent!'));
                    return 'forked result';
            }
        },
    },
});
