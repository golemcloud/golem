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

import { describe, it, expect } from 'vitest';
import { throwIfAborted, awaitPollable } from '../src/internal/pollableUtils';

describe('throwIfAborted', () => {
  it('does nothing when signal is undefined', () => {
    expect(() => throwIfAborted(undefined)).not.toThrow();
  });

  it('does nothing when signal is not aborted', () => {
    const controller = new AbortController();
    expect(() => throwIfAborted(controller.signal)).not.toThrow();
  });

  it('throws the signal reason when aborted with a reason', () => {
    const controller = new AbortController();
    controller.abort('custom reason');
    expect(() => throwIfAborted(controller.signal)).toThrow('custom reason');
  });

  it('throws an AbortError when aborted without a reason', () => {
    const signal = AbortSignal.abort();
    try {
      throwIfAborted(signal);
      expect.unreachable('should have thrown');
    } catch (e: any) {
      // AbortSignal.abort() sets reason to a DOMException by default in Node
      expect(e).toBeDefined();
    }
  });

  it('throws AbortError fallback when aborted and reason is undefined', () => {
    const signal = { aborted: true, reason: undefined } as AbortSignal;

    try {
      throwIfAborted(signal);
      expect.unreachable('should have thrown');
    } catch (e: any) {
      expect(e).toBeInstanceOf(Error);
      expect(e.name).toBe('AbortError');
      expect(e.message).toBe('The operation was aborted.');
    }
  });

  it('throws the Error object when aborted with an Error reason', () => {
    const error = new Error('test error');
    const controller = new AbortController();
    controller.abort(error);
    expect(() => throwIfAborted(controller.signal)).toThrow(error);
  });
});

describe('awaitPollable', () => {
  it('rejects immediately with pre-aborted signal', async () => {
    const signal = AbortSignal.abort('pre-aborted');
    const fakePollable = {
      promise: () => new Promise<void>(() => {}), // never resolves
      abortablePromise: (_signal: AbortSignal) => new Promise<void>(() => {}),
      ready: () => false,
      block: () => {},
    };

    await expect(awaitPollable(fakePollable as any, signal)).rejects.toBe('pre-aborted');
  });

  it('calls promise() when no signal is provided', async () => {
    let promiseCalled = false;
    const fakePollable = {
      promise: () => {
        promiseCalled = true;
        return Promise.resolve();
      },
      abortablePromise: (_signal: AbortSignal) => Promise.resolve(),
      ready: () => true,
      block: () => {},
    };

    await awaitPollable(fakePollable as any);
    expect(promiseCalled).toBe(true);
  });

  it('calls abortablePromise() when signal is provided and not aborted', async () => {
    let abortablePromiseCalled = false;
    const controller = new AbortController();
    const fakePollable = {
      promise: () => Promise.resolve(),
      abortablePromise: (_signal: AbortSignal) => {
        abortablePromiseCalled = true;
        return Promise.resolve();
      },
      ready: () => true,
      block: () => {},
    };

    await awaitPollable(fakePollable as any, controller.signal);
    expect(abortablePromiseCalled).toBe(true);
  });

  it('does not call abortablePromise when signal is already aborted', async () => {
    let abortablePromiseCalled = false;
    const signal = AbortSignal.abort('pre-aborted');

    const fakePollable = {
      promise: () => Promise.resolve(),
      abortablePromise: (_signal: AbortSignal) => {
        abortablePromiseCalled = true;
        return Promise.resolve();
      },
      ready: () => false,
      block: () => {},
    };

    await expect(awaitPollable(fakePollable as any, signal)).rejects.toBe('pre-aborted');
    expect(abortablePromiseCalled).toBe(false);
  });

  it('passes the same signal to abortablePromise', async () => {
    const controller = new AbortController();
    let receivedSignal: AbortSignal | undefined;

    const fakePollable = {
      promise: () => Promise.resolve(),
      abortablePromise: (signal: AbortSignal) => {
        receivedSignal = signal;
        return Promise.resolve();
      },
      ready: () => true,
      block: () => {},
    };

    await awaitPollable(fakePollable as any, controller.signal);
    expect(receivedSignal).toBe(controller.signal);
  });

  it('propagates rejection from promise()', async () => {
    const err = new Error('poll failed');
    const fakePollable = {
      promise: () => Promise.reject(err),
      abortablePromise: (_signal: AbortSignal) => Promise.resolve(),
      ready: () => false,
      block: () => {},
    };

    await expect(awaitPollable(fakePollable as any)).rejects.toBe(err);
  });

  it('propagates rejection from abortablePromise()', async () => {
    const err = new Error('abortable poll failed');
    const controller = new AbortController();

    const fakePollable = {
      promise: () => Promise.resolve(),
      abortablePromise: (_signal: AbortSignal) => Promise.reject(err),
      ready: () => false,
      block: () => {},
    };

    await expect(awaitPollable(fakePollable as any, controller.signal)).rejects.toBe(err);
  });
});
