declare module "rpc:counters-stub/stub-counters" {
  import type { Uri } from "golem:rpc/types@0.1.0";
  import type { Pollable } from "wasi:io/poll@0.2.0";
  export class FutureGetGlobalValueResult {
    subscribe(): Pollable;
    get(): bigint | undefined;
  }
  export class FutureCounterGetValueResult {
    subscribe(): Pollable;
    get(): bigint | undefined;
  }
  export class FutureCounterGetArgsResult {
    subscribe(): Pollable;
    get(): string[] | undefined;
  }
  export class FutureCounterGetEnvResult {
    subscribe(): Pollable;
    get(): [string, string][] | undefined;
  }
  export class Api {
    constructor(location: Uri)
    blockingIncGlobalBy(value: bigint): void;
    incGlobalBy(value: bigint): void;
    blockingGetGlobalValue(): bigint;
    getGlobalValue(): FutureGetGlobalValueResult;
  }
  export class Counter {
    constructor(location: Uri, name: string)
    blockingIncBy(value: bigint): void;
    incBy(value: bigint): void;
    blockingGetValue(): bigint;
    getValue(): FutureCounterGetValueResult;
    blockingGetArgs(): string[];
    getArgs(): FutureCounterGetArgsResult;
    blockingGetEnv(): [string, string][];
    getEnv(): FutureCounterGetEnvResult;
  }
}
