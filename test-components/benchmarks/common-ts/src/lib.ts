import { withPersistenceLevel, type PersistenceLevel } from "@golemcloud/golem-ts-sdk"

export function cpuIntensive(length: number): number {
    const a: number[][] = Array(length).fill(0).map(() => Array(length).fill(1));
    const b: number[][] = Array(length).fill(0).map(() => Array(length).fill(2));
    const c: number[][] = Array(length).fill(0).map(() => Array(length).fill(0));

    for (let i = 0; i < length; i++) {
        for (let j = 0; j < length; j++) {
            let sum = 0;
            for (let k = 0; k < length; k++) {
                sum = (sum + (a[i]![k]! * b[k]![j]!)) >>> 0;
            }
            c[i]![j]! = sum;
        }
    }

    let result = 0;
    for (let i = 0; i < length; i++) {
        for (let j = 0; j < length; j++) {
            result ^= c[i]![j]!;
        }
    }
    return result;
}

export function echo(input: string): string {
    return input;
}

export function largeInput(input: Uint8Array): number {
    return input.length;
}

export function oplogHeavy(length: number, persistenceOn: boolean): number {
    const level: PersistenceLevel = persistenceOn ? { tag: "smart" } : { tag: "persist-nothing" };
    return withPersistenceLevel(level, () => {

        let result = 0;
        for (let i = 0; i < length; i++) {
            const time = Date.now();
            result ^= time;
        }
        return result;
    });
}