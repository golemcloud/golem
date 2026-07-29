import { z } from 'zod';
import { defineAgent, method, http, s, clientFor } from '@golemcloud/golem-ts-sdk';

import * as common from 'common/lib';

export const BenchmarkAgent = defineAgent({
    name: 'BenchmarkAgent',
    id: { name: z.string() },
    http: http.mount('/{name}'),
    methods: {
        echo: method({
            input: { message: z.string() },
            returns: z.string(),
            http: http.post('/echo/{message}'),
        }),
        largeInput: method({
            input: { input: s.uint8Array() },
            returns: z.number(),
            http: http.post('/large-input'),
        }),
        cpuIntensive: method({
            input: { length: z.number() },
            returns: z.number(),
            http: http.post('/cpu-intensive'),
        }),
        oplogHeavy: method({
            input: { length: z.number(), persistenceOn: z.boolean() },
            returns: z.number(),
        }),
    },
});

export const BenchmarkAgentImpl = BenchmarkAgent.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        echo({ message }) {
            return common.echo(message);
        },
        largeInput({ input }) {
            return common.largeInput(input);
        },
        cpuIntensive({ length }) {
            return common.cpuIntensive(length);
        },
        oplogHeavy({ length, persistenceOn }) {
            return common.oplogHeavy(length, persistenceOn);
        },
    },
});

const benchmarkClient = clientFor(BenchmarkAgent);

export const RpcBenchmarkAgent = defineAgent({
    name: 'RpcBenchmarkAgent',
    id: { name: z.string() },
    methods: {
        echo: method({ input: { message: z.string() }, returns: z.string() }),
        largeInput: method({ input: { input: s.uint8Array() }, returns: z.number() }),
        cpuIntensive: method({ input: { length: z.number() }, returns: z.number() }),
        oplogHeavy: method({ input: { length: z.number(), persistenceOn: z.boolean() }, returns: z.number() }),
    },
});

export const RpcBenchmarkAgentImpl = RpcBenchmarkAgent.implement({
    init: ({ id }) => ({ name: id.name }),
    methods: {
        async echo({ message }) {
            return await benchmarkClient({ name: this.name }).echo({ message });
        },
        async largeInput({ input }) {
            return await benchmarkClient({ name: this.name }).largeInput({ input });
        },
        async cpuIntensive({ length }) {
            return await benchmarkClient({ name: this.name }).cpuIntensive({ length });
        },
        async oplogHeavy({ length, persistenceOn }) {
            return await benchmarkClient({ name: this.name }).oplogHeavy({ length, persistenceOn });
        },
    },
});
