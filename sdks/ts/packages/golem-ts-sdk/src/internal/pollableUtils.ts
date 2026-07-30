// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

export function throwIfAborted(signal?: AbortSignal): void {
  if (!signal?.aborted) return;

  if (signal.reason !== undefined) {
    throw signal.reason;
  }

  const err = new Error('The operation was aborted.');
  err.name = 'AbortError';
  throw err;
}

export async function awaitAbortable<T>(
  promise: Promise<T>,
  signal?: AbortSignal,
  onAbort?: () => void,
): Promise<T> {
  if (!signal) {
    return promise;
  }

  if (signal.aborted) {
    promise.catch(() => undefined);
  }
  throwIfAborted(signal);

  let abortListener: (() => void) | undefined;
  const abort = new Promise<never>((_, reject) => {
    abortListener = () => {
      try {
        throwIfAborted(signal);
      } catch (reason) {
        reject(reason);
      }

      try {
        onAbort?.();
      } catch {
        // Cancellation is best-effort; the signal reason determines the rejection.
      }
    };
    signal.addEventListener('abort', abortListener, { once: true });
  });

  try {
    return await Promise.race([promise, abort]);
  } finally {
    if (abortListener) {
      signal.removeEventListener('abort', abortListener);
    }
  }
}
