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

import type { RetryPolicy } from 'golem:api/retry@1.5.0';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Duration, Policy, Predicate } from '../src/host/retryBuilder';
import { retry, RetryPolicyError } from '../src/host/retryExecutor';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function withFakeTime<T>(run: () => Promise<T>): Promise<T> {
  vi.useFakeTimers();
  vi.setSystemTime(0);
  const result = run();
  await vi.runAllTimersAsync();
  return result;
}

describe('user-space retry execution', () => {
  it('retries synchronous throws until eventual success and reports physical attempt numbers', async () => {
    const attempts: number[] = [];

    const result = await retry(Policy.immediate().maxRetries(3), (attempt) => {
      attempts.push(attempt);
      if (attempt < 2) throw new Error(`failure ${attempt}`);
      return 'done';
    });

    expect(result).toBe('done');
    expect(attempts).toEqual([0, 1, 2]);
  });

  it('retries Promise rejections and preserves the final rejected value by identity', async () => {
    const first = { failure: 1 };
    const final = { failure: 2 };
    const errors = [first, final];

    await expect(
      retry(Policy.immediate().maxRetries(1), async (attempt) => Promise.reject(errors[attempt])),
    ).rejects.toBe(final);
  });

  it('uses exponential and fibonacci state independently', async () => {
    const exponentialTimes: number[] = [];
    const fibonacciTimes: number[] = [];

    await withFakeTime(async () => {
      await expect(
        retry(Policy.exponential(Duration.milliseconds(10), 2).maxRetries(3), (attempt) => {
          exponentialTimes.push(Date.now());
          if (attempt === 3) return 'ok';
          throw attempt;
        }),
      ).resolves.toBe('ok');
    });

    await withFakeTime(async () => {
      await expect(
        retry(Policy.fibonacci(Duration.milliseconds(5), Duration.milliseconds(10)), (attempt) => {
          fibonacciTimes.push(Date.now());
          if (attempt === 3) return 'ok';
          throw attempt;
        }),
      ).resolves.toBe('ok');
    });

    expect(exponentialTimes).toEqual([0, 10, 30, 70]);
    expect(fibonacciTimes).toEqual([0, 5, 15, 30]);
  });

  it('matches Rust floating-duration rounding and accepts a raw zero exponential factor', async () => {
    const roundedTimes: number[] = [];
    const belowTieTimes: number[] = [];
    const zeroFactorTimes: number[] = [];

    await withFakeTime(async () => {
      await retry(
        Policy.exponential(Duration.seconds(1), 1 / 1024)
          .addDelay(23_438n)
          .maxRetries(2),
        (attempt) => {
          roundedTimes.push(Date.now());
          if (attempt === 2) return;
          throw new Error('first');
        },
      );
    });

    await withFakeTime(async () => {
      await retry(
        Policy.exponential(Duration.seconds(1), 1.5e-9).addDelay(999_999n).maxRetries(2),
        (attempt) => {
          belowTieTimes.push(Date.now());
          if (attempt === 2) return;
          throw new Error('first');
        },
      );
    });

    const rawZeroFactor = {
      nodes: [
        { tag: 'count-box' as const, val: { maxRetries: 2, inner: 1 } },
        { tag: 'exponential' as const, val: { baseDelay: 2_000_000n, factor: 0 } },
      ],
    };
    await withFakeTime(async () => {
      await retry(rawZeroFactor, (attempt) => {
        zeroFactorTimes.push(Date.now());
        if (attempt === 2) return;
        throw new Error('failure');
      });
    });

    expect(roundedTimes).toEqual([0, 1001, 1002]);
    expect(belowTieTimes).toEqual([0, 1001, 1002]);
    expect(zeroFactorTimes).toEqual([0, 2, 2]);
  });

  it('applies clamping, delay addition, and deterministic positive jitter in nesting order', async () => {
    vi.spyOn(Math, 'random').mockReturnValue(0.5);
    const times: number[] = [];

    await withFakeTime(async () => {
      await retry(
        Policy.periodic(Duration.milliseconds(50))
          .clamp(Duration.milliseconds(10), Duration.milliseconds(30))
          .addDelay(Duration.milliseconds(5))
          .withJitter(0.2)
          .maxRetries(1),
        (attempt) => {
          times.push(Date.now());
          if (attempt === 1) return;
          throw new Error('retry');
        },
      );
    });

    expect(times).toEqual([0, 39]);
  });

  it('hands and-then to the right immediately and keeps union/intersection states independent', async () => {
    const andThenTimes: number[] = [];
    const unionTimes: number[] = [];
    const intersectTimes: number[] = [];

    await withFakeTime(async () => {
      await retry(
        Policy.periodic(Duration.milliseconds(7))
          .maxRetries(1)
          .andThen(Policy.periodic(Duration.milliseconds(20))),
        (attempt) => {
          andThenTimes.push(Date.now());
          if (attempt === 2) return;
          throw attempt;
        },
      );
    });

    await withFakeTime(async () => {
      await retry(
        Policy.periodic(Duration.milliseconds(10))
          .maxRetries(1)
          .union(Policy.periodic(Duration.milliseconds(20)).maxRetries(3)),
        (attempt) => {
          unionTimes.push(Date.now());
          if (attempt === 3) return;
          throw attempt;
        },
      );
    });

    const final = new Error('intersection stopped');
    await withFakeTime(async () => {
      await expect(
        retry(
          Policy.periodic(Duration.milliseconds(10))
            .maxRetries(1)
            .intersect(Policy.periodic(Duration.milliseconds(20)).maxRetries(3)),
          (attempt) => {
            intersectTimes.push(Date.now());
            throw attempt === 0 ? new Error('first') : final;
          },
        ),
      ).rejects.toBe(final);
    });

    expect(andThenTimes).toEqual([0, 7, 27]);
    expect(unionTimes).toEqual([0, 10, 30, 50]);
    expect(intersectTimes).toEqual([0, 20]);
  });

  it('projects fresh properties from every failure and stops when a filtered predicate changes', async () => {
    const errors = [{ status: 503 }, { status: 503 }, { status: 400 }];

    await expect(
      retry(
        Policy.immediate().onlyWhen(Predicate.eq('status', 503)),
        (attempt) => {
          throw errors[attempt];
        },
        {
          properties: (error) => ({ status: (error as { status: number }).status }),
        },
      ),
    ).rejects.toBe(errors[2]);
  });

  it('evaluates every predicate node with platform coercion semantics', async () => {
    const predicate = Predicate.never().or(
      Predicate.eq('code', 503)
        .and(Predicate.neq('kind', 'fatal'))
        .and(Predicate.gt('code', 500))
        .and(Predicate.gte('code', 503))
        .and(Predicate.lt('code', 600))
        .and(Predicate.lte('code', 503))
        .and(Predicate.exists('service'))
        .and(Predicate.oneOf('kind', ['transient', 'other']))
        .and(Predicate.matchesGlob('service', 'bill?ng-*'))
        .and(Predicate.startsWith('service', 'billing'))
        .and(Predicate.contains('service', '-api'))
        .and(Predicate.always().not().not()),
    );

    await expect(
      retry(
        Policy.immediate().maxRetries(1).onlyWhen(predicate),
        (attempt) => {
          if (attempt === 1) return 'matched';
          throw new Error('first');
        },
        { properties: () => ({ code: '503', kind: 'transient', service: 'billing-api' }) },
      ),
    ).resolves.toBe('matched');
  });

  it.each([
    ['billing-*', 'billing-api', true],
    ['bill?ng-*', 'billing-api', true],
    ['billing-*', 'payments-api', false],
    ['service-[a-c]', 'service-b', false],
    ['?x', 'éx', true],
    ['*', 'first\nsecond', false],
    ['?', '😀', false],
  ])('matches the local minimal glob syntax: %s against %s', async (pattern, value, expected) => {
    let attempts = 0;
    const result = retry(
      Policy.immediate().maxRetries(1).onlyWhen(Predicate.matchesGlob('value', pattern)),
      () => {
        attempts += 1;
        throw new Error('failure');
      },
      { properties: () => ({ value }) },
    );

    await expect(result).rejects.toThrow('failure');
    expect(attempts).toBe(expected ? 2 : 1);
  });

  it('never retries and preserves a synchronously thrown value', async () => {
    const failure = { kind: 'sync failure' };
    await expect(
      retry(Policy.never(), () => {
        throw failure;
      }),
    ).rejects.toBe(failure);
  });

  it('matches signed integer text and orders text as UTF-8 bytes', async () => {
    await expect(
      retry(
        Policy.immediate()
          .maxRetries(1)
          .onlyWhen(Predicate.eq('code', 503).and(Predicate.gt('text', '\uE000'))),
        (attempt) => {
          if (attempt === 1) return 'matched';
          throw new Error('first');
        },
        { properties: () => ({ code: '+503', text: '\u{10000}' }) },
      ),
    ).resolves.toBe('matched');

    await expect(
      retry(
        Policy.immediate().onlyWhen(Predicate.eq('code', 503)),
        () => {
          throw new Error('first');
        },
        { properties: () => ({ code: '503\n' }) },
      ),
    ).rejects.toBeInstanceOf(RetryPolicyError);
  });

  it('gives up at the elapsed-time boundary but retries immediately before it', async () => {
    const atBoundary: number[] = [];
    const beforeBoundary: number[] = [];

    await withFakeTime(async () => {
      await expect(
        retry(Policy.periodic(Duration.milliseconds(10)).within(Duration.milliseconds(10)), () => {
          atBoundary.push(Date.now());
          throw new Error('failure');
        }),
      ).rejects.toThrow('failure');
    });

    await withFakeTime(async () => {
      await expect(
        retry(
          Policy.periodic(Duration.milliseconds(9)).within(Duration.milliseconds(10)),
          (attempt) => {
            beforeBoundary.push(Date.now());
            if (attempt === 2) return 'ok';
            throw new Error('failure');
          },
        ),
      ).resolves.toBe('ok');
    });

    expect(atBoundary).toEqual([0, 10]);
    expect(beforeBoundary).toEqual([0, 9, 18]);
  });

  it('rejects malformed and cyclic raw ASTs before invoking user code', async () => {
    const operation = vi.fn();
    const malformed = { nodes: [{ tag: 'count-box', val: { maxRetries: 1, inner: 4 } }] };
    const cyclic = { nodes: [{ tag: 'count-box', val: { maxRetries: 1, inner: 0 } }] };
    const cyclicPredicate = {
      nodes: [
        {
          tag: 'filtered-on',
          val: {
            inner: 1,
            predicate: { nodes: [{ tag: 'pred-not', val: 0 }] },
          },
        },
        { tag: 'immediate' },
      ],
    };

    for (const raw of [{ nodes: [] }, malformed, cyclic, cyclicPredicate]) {
      await expect(retry(raw as RetryPolicy, operation)).rejects.toBeInstanceOf(RetryPolicyError);
    }
    expect(operation).not.toHaveBeenCalled();
  });

  it('cancels before starting, during an attempt, and while waiting for a timer', async () => {
    const operation = vi.fn();
    await expect(
      retry(Policy.immediate(), operation, { signal: AbortSignal.abort('pre-aborted') }),
    ).rejects.toBe('pre-aborted');
    expect(operation).not.toHaveBeenCalled();

    const duringAttempt = new AbortController();
    const pending = retry(Policy.immediate(), () => new Promise<never>(() => {}), {
      signal: duringAttempt.signal,
    });
    await Promise.resolve();
    duringAttempt.abort('attempt-aborted');
    await expect(pending).rejects.toBe('attempt-aborted');

    vi.useFakeTimers();
    const duringDelay = new AbortController();
    const delayed = retry(
      Policy.periodic(Duration.seconds(30)),
      () => {
        throw new Error('first');
      },
      { signal: duringDelay.signal },
    );
    await vi.advanceTimersByTimeAsync(0);
    duringDelay.abort('delay-aborted');
    await expect(delayed).rejects.toBe('delay-aborted');
    expect(vi.getTimerCount()).toBe(0);
  });

  it('does not starve or leak cancellation around immediate retry scheduling', async () => {
    const beforeStart = new AbortController();
    const operation = vi.fn(() => {
      throw new Error('should not run');
    });
    const notStarted = retry(Policy.immediate(), operation, { signal: beforeStart.signal });
    beforeStart.abort('before-start');
    await expect(notStarted).rejects.toBe('before-start');
    expect(operation).not.toHaveBeenCalled();

    const immediate = new AbortController();
    const cancelled = retry(
      Policy.immediate(),
      () => {
        throw new Error('retry');
      },
      { signal: immediate.signal },
    );
    globalThis.setTimeout(() => immediate.abort('timer-abort'), 0);
    await expect(cancelled).rejects.toBe('timer-abort');

    vi.useFakeTimers();
    const fromProjection = new AbortController();
    const projected = retry(
      Policy.periodic(Duration.seconds(30)),
      () => {
        throw new Error('retry');
      },
      {
        signal: fromProjection.signal,
        properties: () => {
          fromProjection.abort('projection-abort');
          return {};
        },
      },
    );
    await expect(projected).rejects.toBe('projection-abort');
    expect(vi.getTimerCount()).toBe(0);
  });

  it('does not allocate the next timer chunk after cancellation', async () => {
    vi.useFakeTimers();
    const controller = new AbortController();
    const pending = retry(
      Policy.periodic(Duration.milliseconds(0x8000_0000)),
      () => {
        throw new Error('retry');
      },
      { signal: controller.signal },
    );

    await vi.advanceTimersByTimeAsync(0);
    vi.advanceTimersByTime(0x7fff_ffff);
    await Promise.resolve();
    await Promise.resolve();
    controller.abort('between-chunks');

    await expect(pending).rejects.toBe('between-chunks');
    expect(vi.getTimerCount()).toBe(0);
  });
});
