declare module "rpc:counters-stub/stub-counters" {
  import type { Uri } from "golem:rpc/types@0.1.0";
  import type { Pollable } from "wasi:io/poll@0.2.0";
  export class FutureGetGlobalValueResult {
    subscribe(): Pollable;
    get(): bigint | undefined;
  }
  export class FutureCounterGetvalueResult {
    subscribe(): Pollable;
    get(): bigint | undefined;
  }
  export class FutureCounterGetargsResult {
    subscribe(): Pollable;
    get(): string[] | undefined;
  }
  export class FutureCounterGetenvResult {
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
    blockingIncby(value: bigint): void;
    incby(value: bigint): void;
    blockingGetvalue(): bigint;
    getvalue(): FutureCounterGetvalueResult;
    blockingGetargs(): string[];
    getargs(): FutureCounterGetargsResult;
    blockingGetenv(): [string, string][];
    getenv(): FutureCounterGetenvResult;
  }
}
