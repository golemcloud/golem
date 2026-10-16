import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Result } from '../src/host/result';
import type { PredicateValue, RetryDecision, RetryPolicy } from 'golem:api/retry@1.5.0';

vi.hoisted(() => vi.resetModules());
const boundary = vi.hoisted(() => ({
  resolve: vi.fn<(...args: unknown[]) => RetryPolicy | undefined>(() => undefined),
  next: vi.fn<(...args: unknown[]) => RetryDecision>(),
  time: 0n,
}));
vi.mock('../src/host/hostapi', () => ({
  getSelfMetadata: () => ({ agentId: { componentId: 'component', agentId: 'agent' } }),
}));
vi.mock('golem:api/retry@1.5.0', () => ({
  resolveRetryPolicy: boundary.resolve,
  nextRetryDelay: boundary.next,
}));
vi.mock('node:process', () => ({
  default: { hrtime: { bigint: () => boundary.time } },
}));
import { compensable, fallibleSaga, infallibleSaga, SagaRetryFailure } from '../src/host/saga';

beforeEach(() => {
  boundary.time = 0n;
  boundary.resolve.mockReset().mockReturnValue(undefined);
  boundary.next.mockReset().mockReturnValue({ state: new Uint8Array([42]), delay: undefined });
});
afterEach(() => vi.useRealTimers());

describe('sagas', () => {
  for (const mode of ['throw', 'result'] as const) {
    it(`distinguishes undefined compensation ${mode} from exhaustion`, async () => {
      const step = compensable(
        async () => Result.ok(1),
        async () => {
          if (mode === 'throw') throw undefined;
          return Result.err(undefined);
        },
      );
      const result = await infallibleSaga(async (saga) => {
        await saga.execute(step, undefined);
        throw 'original';
      });
      expect(result.isErr()).toBe(true);
      expect(result.val).toMatchObject({
        error: 'original',
        reason: {
          type: 'compensation-failure',
          compensationFailure: undefined,
        },
      });
      expect(boundary.resolve).not.toHaveBeenCalled();
    });
  }

  it('distinguishes an undefined policy exception from exhaustion', async () => {
    boundary.resolve.mockImplementation(() => {
      throw undefined;
    });
    const result = await infallibleSaga(async () => {
      throw undefined;
    });
    expect(result.val).toMatchObject({
      error: undefined,
      reason: {
        type: 'policy-error',
        policyError: undefined,
      },
    });
    expect(boundary.next).not.toHaveBeenCalled();
  });

  for (const kind of ['returned', 'thrown', 'interrupted'] as const) {
    it(`compensates ${kind} failures in reverse and exhausts through the host`, async () => {
      const trace: string[] = [];
      const original = new Error('original');
      if (kind === 'interrupted') original.name = 'AbortError';
      const reserve = compensable(
        async (id: string) => {
          trace.push(id);
          return Result.ok(id);
        },
        async (id) => {
          trace.push(`undo:${id}`);
          boundary.time += 1_000_000n;
          return Result.ok(undefined);
        },
      );
      const fail = compensable(
        async () => Result.err(original),
        async () => Result.ok(undefined),
      );
      const result = await infallibleSaga(async (saga) => {
        await saga.execute(reserve, 'a');
        await saga.execute(reserve, 'b');
        boundary.time += 5_000_000n;
        if (kind !== 'returned') throw original;
        return saga.execute(fail, undefined);
      });
      expect(result.isErr()).toBe(true);
      expect(result.val).toMatchObject({ error: original });
      expect(trace).toEqual(['a', 'b', 'undo:b', 'undo:a']);
      const props: [string, PredicateValue][] = [
        ['attempt', { tag: 'integer', val: 1n }],
        [
          'error-kind',
          {
            tag: 'text',
            val:
              kind === 'thrown'
                ? 'exception'
                : kind === 'interrupted'
                  ? 'interrupted'
                  : 'operation-error',
          },
        ],
      ];
      expect(boundary.resolve).toHaveBeenCalledWith('saga', 'urn:worker:component/agent', props);
      expect(boundary.next).toHaveBeenCalledWith(
        undefined,
        new Uint8Array(),
        7_000_000n,
        'saga',
        'urn:worker:component/agent',
        props,
      );
    });
  }

  it('carries opaque host state and waits for host backoff without interpreting a tree', async () => {
    vi.useFakeTimers();
    const token = new Uint8Array([9, 8, 7]);
    boundary.next.mockReturnValueOnce({ state: token, delay: 25_000_000n });
    let attempts = 0;
    const running = infallibleSaga(async () => {
      attempts++;
      throw 'failed';
    });
    await vi.advanceTimersByTimeAsync(24);
    expect(attempts).toBe(1);
    boundary.time = 25_000_000n;
    await vi.advanceTimersByTimeAsync(1);
    const result = await running;
    expect(result.isErr()).toBe(true);
    expect(attempts).toBe(2);
    expect(boundary.next.mock.calls[1]?.[1]).toBe(token);
    expect(boundary.next.mock.calls[1]?.[2]).toBe(25_000_000n);
    expect(boundary.resolve).toHaveBeenCalledTimes(2);
  });

  it('returns a typed failure on host policy error', async () => {
    boundary.next.mockImplementation(() => {
      throw 'changed policy';
    });
    const result = await infallibleSaga(async () => {
      throw 'original';
    });
    expect(result.val).toMatchObject({
      error: 'original',
      reason: { type: 'policy-error', policyError: 'changed policy' },
    });
  });

  it('forwards a selected policy untouched instead of interpreting its tree', async () => {
    const policy = { nodes: [] };
    boundary.resolve.mockReturnValue(policy);
    boundary.next.mockReturnValueOnce({ state: new Uint8Array([1]), delay: 0n });
    let attempts = 0;
    const result = await infallibleSaga(async () => {
      if (++attempts === 1) throw 'retry';
      return 42;
    });
    expect(result.val).toBe(42);
    expect(boundary.next.mock.calls[0]?.[0]).toBe(policy);
  });

  it('rethrows the original fallible exception after compensation', async () => {
    const original = new Error('original');
    const trace: number[] = [];
    const step = compensable(
      async (id: number) => Result.ok(id),
      async (id) => {
        trace.push(id);
        return Result.ok(undefined);
      },
    );
    await expect(
      fallibleSaga(async (saga) => {
        await saga.execute(step, 1);
        await saga.execute(step, 2);
        throw original;
      }),
    ).rejects.toBe(original);
    expect(trace).toEqual([2, 1]);
    expect(boundary.next).not.toHaveBeenCalled();
  });

  for (const thrown of [false, true]) {
    it(`does not retry partial compensation (thrown=${thrown})`, async () => {
      const step = compensable(
        async () => Result.ok(1),
        async () => {
          if (thrown) throw 'compensation';
          return Result.err('compensation');
        },
      );
      const fail = compensable(
        async () => Result.err('original'),
        async () => Result.ok(undefined),
      );
      const result = await infallibleSaga(async (saga) => {
        await saga.execute(step, undefined);
        await saga.execute(fail, undefined).catch(() => undefined);
        return 'swallowed';
      });
      expect(result.isErr()).toBe(true);
      expect(result.val).toBeInstanceOf(SagaRetryFailure);
      expect(result.val).toMatchObject({
        error: 'original',
        reason: { type: 'compensation-failure', compensationFailure: 'compensation' },
      });
      expect(boundary.next).not.toHaveBeenCalled();
    });
  }

  it('returns fallible partial compensation metadata without retry', async () => {
    const step = compensable(
      async () => Result.ok(1),
      async () => Result.err('undo'),
    );
    const result = await fallibleSaga(async (saga) => {
      await saga.execute(step, undefined);
      return Result.err('body');
    });
    expect(result.val).toEqual({
      type: 'FailedAndRolledBackPartially',
      error: 'body',
      compensationFailure: 'undo',
    });
  });

  it('allows a handled fallible step failure and skips compensation on success', async () => {
    const undo = vi.fn(async () => Result.ok(undefined));
    const step = compensable(
      async (fail: boolean) => (fail ? Result.err('handled') : Result.ok(42)),
      undo,
    );
    const result = await fallibleSaga(async (saga) => {
      await saga.execute(step, true);
      return saga.execute(step, false);
    });
    expect(result.val).toBe(42);
    expect(undo).not.toHaveBeenCalled();
  });
});
