export interface Api {
  Counter: CounterStatic
  incGlobalBy(value: bigint): void,
  getGlobalValue(): bigint,
}

export interface CounterStatic {
  new(name: string): CounterInstance,
}
export interface CounterInstance {
  incby(value: bigint): void,
  getvalue(): bigint,
  getargs(): string[],
  getenv(): [string, string][],
}

export interface CountersWorld {
  api: Api,
}
