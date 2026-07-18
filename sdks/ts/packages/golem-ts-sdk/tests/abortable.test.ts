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
import { awaitAbortable, throwIfAborted } from '../src/internal/pollableUtils';

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

describe('awaitAbortable', () => {
  it('rejects immediately with pre-aborted signal', async () => {
    const signal = AbortSignal.abort('pre-aborted');
    const onAbort = vi.fn();

    await expect(awaitAbortable(new Promise<void>(() => {}), signal, onAbort)).rejects.toBe(
      'pre-aborted',
    );
    expect(onAbort).not.toHaveBeenCalled();
  });

  it('resolves the supplied promise when no signal is provided', async () => {
    await expect(awaitAbortable(Promise.resolve('done'))).resolves.toBe('done');
  });

  it('rejects with the abort reason and invokes cancellation', async () => {
    const controller = new AbortController();
    const onAbort = vi.fn();
    const result = awaitAbortable(new Promise<void>(() => {}), controller.signal, onAbort);

    controller.abort('cancelled');
    await expect(result).rejects.toBe('cancelled');
    expect(onAbort).toHaveBeenCalledOnce();
  });

  it('keeps the abort reason when cancellation rejects the operation', async () => {
    const controller = new AbortController();
    const abortReason = new Error('caller aborted');
    const cancellationError = new Error('host cancelled');
    let rejectOperation!: (reason: unknown) => void;
    const operation = new Promise<never>((_, reject) => {
      rejectOperation = reject;
    });
    const result = awaitAbortable(operation, controller.signal, () => {
      rejectOperation(cancellationError);
    });

    controller.abort(abortReason);

    await expect(result).rejects.toBe(abortReason);
  });

  it('removes cancellation after the operation settles', async () => {
    const controller = new AbortController();
    const onAbort = vi.fn();

    await expect(awaitAbortable(Promise.resolve('done'), controller.signal, onAbort)).resolves.toBe(
      'done',
    );
    controller.abort('too late');
    expect(onAbort).not.toHaveBeenCalled();
  });

  it('propagates rejection from the supplied promise', async () => {
    const err = new Error('operation failed');
    const controller = new AbortController();

    await expect(awaitAbortable(Promise.reject(err), controller.signal)).rejects.toBe(err);
  });

  it('observes a supplied operation rejection when the signal is already aborted', async () => {
    const operationError = new Error('operation failed after abort');
    let rejectOperation!: (reason: unknown) => void;
    const operation = new Promise<never>((_, reject) => {
      rejectOperation = reject;
    });
    const unhandled: unknown[] = [];
    const recordUnhandled = (reason: unknown) => unhandled.push(reason);
    process.on('unhandledRejection', recordUnhandled);

    try {
      await expect(awaitAbortable(operation, AbortSignal.abort('pre-aborted'))).rejects.toBe(
        'pre-aborted',
      );
      rejectOperation(operationError);
      await new Promise((resolve) => setTimeout(resolve, 0));

      expect(unhandled).toEqual([]);
    } finally {
      process.off('unhandledRejection', recordUnhandled);
    }
  });
});
