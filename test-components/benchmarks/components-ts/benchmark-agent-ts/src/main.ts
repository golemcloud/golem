import {
    BaseAgent,
    agent,
} from '@golemcloud/golem-ts-sdk';

import * as common from "common/lib";

@agent()
class BenchmarkAgent extends BaseAgent {
    private readonly name: string;

    constructor(name: string) {
        super()
        this.name = name;
    }

    echo(message: string): string {
        return common.echo(message);
    }

    largeInput(input: Uint8Array): number {
        return common.largeInput(input);
    }

    cpuIntensive(length: number): number {
        return common.cpuIntensive(length);
    }

    oplogHeavy(length: number, persistenceOn: boolean): number {
        return common.oplogHeavy(length, persistenceOn);
    }
}

@agent()
class RpcBenchmarkAgent extends BaseAgent {
    private readonly name: string;

    constructor(name: string) {
        super()
        this.name = name;
    }

    async echo(message: string): Promise<string> {
        const client = BenchmarkAgent.get(this.name);
        return await client.echo(message);
    }

    async largeInput(input: Uint8Array): Promise<number> {
        const client = BenchmarkAgent.get(this.name);
        return await client.largeInput(input);
    }

    async cpuIntensive(length: number): Promise<number> {
        const client = BenchmarkAgent.get(this.name);
        return await client.cpuIntensive(length);
    }

    async oplogHeavy(length: number, persistenceOn: boolean): Promise<number> {
        const client = BenchmarkAgent.get(this.name);
        return await client.oplogHeavy(length, persistenceOn);
    }
}