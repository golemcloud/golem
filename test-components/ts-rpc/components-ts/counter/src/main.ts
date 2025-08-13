import type * as World from 'rpc:counters/counters'

import { getArguments, getEnvironment } from 'wasi:cli/environment@0.2.3'

class CounterResource implements World.api.Counter {
    value: bigint
    constructor(private name: string) {
        this.value = BigInt(0)
    }

    incBy(value: bigint) {
        this.value += value
    }
    getValue(): bigint {
        return this.value
    }
    getArgs(): string[] {
        return getArguments()
    }
    getEnv(): [string, string][] {
        return getEnvironment()
    }

}

let globalValue: bigint = BigInt(0)


const api: typeof World.api = {
    Counter: CounterResource,
    incGlobalBy(value: bigint) {
        globalValue += value
    },
    getGlobalValue() {
        return globalValue
    },
}

export { api }
