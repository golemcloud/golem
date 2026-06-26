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

import { afterEach, describe, expect, it, vi } from 'vitest';

describe('withReservation', () => {
  let commitMock: ReturnType<typeof vi.fn>;

  afterEach(() => {
    vi.doUnmock('golem:quota/types@1.5.0');
    vi.resetModules();
  });

  /**
   * Loads `quota.ts` with a fresh module registry and a controlled mock for
   * `golem:quota/types@1.5.0`.
   *
   * The quota host module now uses the free functions `new-token` / `reserve` /
   * `split` / `merge` plus the `reservation` resource; the opaque
   * `quota-token` handle lives in `golem:core/types`. `reserveImpl` controls
   * what the `reserve(token, amount)` free function does. The default returns a
   * raw reservation object; pass a custom function to make it throw.
   */
  async function loadQuotaModule(reserveImpl?: (amount: bigint) => unknown): Promise<{
    withReservation: typeof import('../src/host/quota').withReservation;
    acquireQuotaToken: typeof import('../src/host/quota').acquireQuotaToken;
  }> {
    vi.resetModules();

    commitMock = vi.fn();

    // A minimal stand-in for RawReservation.  quota.ts only ever calls
    // `RawReservation.commit(this.raw, used)` (static-style), so we expose a
    // static `commit` that delegates to our spy.
    const mockRawReservation = {};
    const MockRawReservationClass = {
      commit: (rawReservation: unknown, used: bigint) => commitMock(rawReservation, used),
    };

    const defaultReserveImpl = (_amount: bigint) => mockRawReservation;

    vi.doMock('golem:quota/types@1.5.0', () => ({
      Reservation: MockRawReservationClass,
      // `new-token` returns the opaque owned `quota-token` handle; an empty
      // object is a sufficient stand-in for it.
      newToken: vi.fn().mockImplementation(() => ({})),
      reserve: (_token: unknown, amount: bigint) => (reserveImpl ?? defaultReserveImpl)(amount),
      split: vi.fn().mockImplementation(() => ({})),
      merge: vi.fn(),
    }));

    return import('../src/host/quota');
  }

  it('sync success: commits actual used amount and returns Result.ok', async () => {
    const { withReservation, acquireQuotaToken } = await loadQuotaModule();
    const token = acquireQuotaToken('my-resource', 1000n);

    const result = withReservation(token, 500n, (_reservation) => ({
      used: 42n,
      value: 'hello',
    }));

    expect(result.isOk()).toBe(true);
    expect(result.unwrap()).toBe('hello');
    expect(commitMock).toHaveBeenCalledOnce();
    expect(commitMock).toHaveBeenCalledWith(expect.anything(), 42n);
  });

  it('sync throw: commits zero and re-throws', async () => {
    const { withReservation, acquireQuotaToken } = await loadQuotaModule();
    const token = acquireQuotaToken('my-resource', 1000n);
    const error = new Error('boom');

    expect(() =>
      withReservation(token, 500n, () => {
        throw error;
      }),
    ).toThrow(error);

    expect(commitMock).toHaveBeenCalledOnce();
    expect(commitMock).toHaveBeenCalledWith(expect.anything(), 0n);
  });

  it('async success: commits actual used amount and resolves with Result.ok', async () => {
    const { withReservation, acquireQuotaToken } = await loadQuotaModule();
    const token = acquireQuotaToken('my-resource', 1000n);

    const result = await withReservation(token, 500n, async (_reservation) => ({
      used: 99n,
      value: 'async-value',
    }));

    expect(result.isOk()).toBe(true);
    expect(result.unwrap()).toBe('async-value');
    expect(commitMock).toHaveBeenCalledOnce();
    expect(commitMock).toHaveBeenCalledWith(expect.anything(), 99n);
  });

  it('async rejection: commits zero and propagates the rejection', async () => {
    const { withReservation, acquireQuotaToken } = await loadQuotaModule();
    const token = acquireQuotaToken('my-resource', 1000n);
    const error = new Error('async-boom');

    const promise = withReservation(token, 500n, async () => {
      throw error;
    });

    await expect(promise).rejects.toThrow(error);

    expect(commitMock).toHaveBeenCalledOnce();
    expect(commitMock).toHaveBeenCalledWith(expect.anything(), 0n);
  });

  it('failed reservation: returns Result.err without calling commit', async () => {
    const failedReservation = { estimatedWaitNanos: 5000n };

    const { withReservation, acquireQuotaToken } = await loadQuotaModule(() => {
      throw failedReservation;
    });

    const token = acquireQuotaToken('my-resource', 1000n);

    const result = withReservation(token, 500n, (_reservation) => ({
      used: 1n,
      value: 'unreachable',
    }));

    expect(result.isErr()).toBe(true);
    expect(result.unwrapErr()).toBe(failedReservation);
    expect(commitMock).not.toHaveBeenCalled();
  });

  it('toJSON: serializing a quota token throws', async () => {
    const { acquireQuotaToken } = await loadQuotaModule();
    const token = acquireQuotaToken('my-resource', 1000n);

    expect(() => JSON.stringify(token)).toThrow(/cannot be serialized/);
    expect(() => JSON.stringify({ nested: token })).toThrow(/cannot be serialized/);
  });

  it('merge: merging a token into itself throws without consuming it', async () => {
    const { acquireQuotaToken } = await loadQuotaModule();
    const token = acquireQuotaToken('my-resource', 1000n);

    expect(() => token.merge(token)).toThrow(/itself/);

    // The handle must still be usable after the rejected self-merge.
    expect(token.reserve(1n).isOk()).toBe(true);
  });
});
