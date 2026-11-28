// Test double of the `wasm-rquickjs:import-abort` runtime module. The runtime gives the current
// signal to every async import that `fn` calls synchronously; this double approximates that for
// the single import that `fn` returns. As in the runtime, `fn` always runs. When the signal is
// already aborted, or aborts before the import settles, the returned promise rejects with the
// signal's reason and the import is recorded as dropped.

interface ImportSignal {
  readonly aborted: boolean;
  readonly reason: unknown;
  addEventListener(type: 'abort', listener: () => void, options?: { once?: boolean }): void;
  removeEventListener(type: 'abort', listener: () => void): void;
}

let current: ImportSignal | undefined;
let dropped = 0;

/** The signal that is current while `withSignal` runs its function. */
export function currentImportSignal(): ImportSignal | undefined {
  return current;
}

/** The number of imports that an abort dropped since the last call. */
export function takeDroppedImports(): number {
  const count = dropped;
  dropped = 0;
  return count;
}

export function withSignal<T>(signal: ImportSignal | undefined | null, fn: () => T): T {
  const previous = current;
  current = signal ?? undefined;
  try {
    const result = fn();
    if (!signal || !(result instanceof Promise)) return result;
    if (signal.aborted) {
      dropped++;
      result.catch(() => {});
      return Promise.reject(signal.reason) as T;
    }
    return new Promise((resolve, reject) => {
      const onAbort = () => {
        dropped++;
        reject(signal.reason);
      };
      signal.addEventListener('abort', onAbort, { once: true });
      result.then(
        (value) => {
          signal.removeEventListener('abort', onAbort);
          resolve(value);
        },
        (error) => {
          signal.removeEventListener('abort', onAbort);
          reject(error);
        },
      );
    }) as T;
  } finally {
    current = previous;
  }
}
