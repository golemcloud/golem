import { getArguments, getEnvironment } from 'wasi:cli/environment@0.2.0'
import { CountersWorld, Api, CounterInstance } from './generated/counters'

class CounterResource implements CounterInstance {
    value: bigint
    constructor(private name: string) {
        this.value = BigInt(0)
    }

    incBy(value: bigint): void {
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


const api: Api = {
    Counter: CounterResource,
    incGlobalBy(value: bigint) {
        globalValue += value
    },
    getGlobalValue() {
        return globalValue
    },
}

const _: CountersWorld  = {
    api
}

export { api }