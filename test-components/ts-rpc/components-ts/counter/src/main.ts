import type * as World from 'rpc:counters/counters'

import { getArguments, getEnvironment } from 'wasi:cli/environment@0.2.0'

class CounterResource implements World.api.Counter {
    value: bigint
    constructor(private name: string) {
        this.value = BigInt(0)
    }

    async incBy(value: bigint): Promise<void> {
        this.value += value
    }
    async getValue(): Promise<bigint> {
        return this.value
    }
    async getArgs(): Promise<string[]> {
        return getArguments()
    }
    async getEnv(): Promise<[string, string][]> {
        return getEnvironment()
    }

}

let globalValue: bigint = BigInt(0)


const api: typeof World.api = {
    Counter: CounterResource,
    async incGlobalBy(value: bigint) {
        globalValue += value
    },
    async getGlobalValue() {
        return globalValue
    },
}

export { api }
