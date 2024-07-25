export interface Api {
  Counter: CounterStatic
  incGlobalBy(value: bigint): void,
  getGlobalValue(): bigint,
}

export interface CounterStatic {
  new(name: string): CounterInstance,
}
export interface CounterInstance {
  incBy(value: bigint): void,
  getValue(): bigint,
  getArgs(): string[],
  getEnv(): [string, string][],
}

export interface CountersWorld {
  api: Api,
}
