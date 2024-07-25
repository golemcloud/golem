import { getArguments, getEnvironment } from 'wasi:cli/environment@0.2.0'
import { CountersWorld, Api, CounterInstance } from './generated/counters'

class CounterResource implements CounterInstance {
    value: bigint
    constructor(private name: string) {
        this.value = BigInt(0)
    }

    incby(value: bigint): void {
        this.value += value
    }
    getvalue(): bigint {
        return this.value
    }
    getargs(): string[] {
        return getArguments()
    }
    getenv(): [string, string][] {
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