import { z } from 'zod';
import { defineAgent, method } from '@golemcloud/golem-ts-sdk';

export const SelfRpcAgent = defineAgent({
    name: 'SelfRpcAgent',
    id: { name: z.string() },
    methods: {
        doWork: method({ input: {}, returns: z.void() }),
        selfRpc: method({ input: {}, returns: z.void() }),
    },
});

const selfClient = SelfRpcAgent.client;

export const SelfRpcAgentImpl = SelfRpcAgent.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        async doWork() {
            return;
        },
        async selfRpc() {
            return selfClient.get({ name: this.name }).doWork();
        },
    },
});
