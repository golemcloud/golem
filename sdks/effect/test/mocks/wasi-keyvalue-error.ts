/**
 * Vitest mock for `wasi:keyvalue/wasi-keyvalue-error@0.1.0`.
 */

export class Error {
  constructor(private readonly _trace: string) {}
  trace(): string {
    return this._trace
  }
}

export const __makeError = (trace: string): Error => new Error(trace)
