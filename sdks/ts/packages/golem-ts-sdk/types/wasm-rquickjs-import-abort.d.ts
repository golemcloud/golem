/**
 * The abort signal of generated async imports.
 *
 * `withSignal(signal, fn)` makes `signal` current while `fn` runs synchronously. Every async
 * import that is called meanwhile takes the signal with it. When the signal aborts before the
 * import finishes, the import's promise rejects with `signal.reason` and the runtime drops the
 * import, which cancels its component-model subtask: the host stops the call or learns that its
 * caller stopped waiting. A signal that is already aborted rejects without starting the import.
 * Imports that `fn` starts after an `await` do not take the signal.
 */
declare module 'wasm-rquickjs:import-abort' {
  /** The part of `AbortSignal` the runtime uses. */
  export interface ImportSignal {
    readonly aborted: boolean;
    readonly reason: unknown;
    addEventListener(type: 'abort', listener: () => void, options?: { once?: boolean }): void;
    removeEventListener(type: 'abort', listener: () => void): void;
  }

  /**
   * Calls `fn` with `signal` current, then restores the previous signal. `undefined` and `null`
   * clear the current signal.
   */
  export function withSignal<T>(signal: ImportSignal | undefined | null, fn: () => T): T;
}
