import { afterEach, describe, expect, it, vi } from 'vitest';
import { Result } from '../src/host/result';

afterEach(() => {
  vi.doUnmock('golem:api/host@1.5.0');
  vi.resetModules();
});

describe('sagas', () => {
  it.each(['fallible', 'infallible'] as const)(
    '%s traps unexpected body and step throws without compensation or retry',
    async (mode) => {
      for (const location of ['body', 'step']) {
        const trapped = new Error('host trap');
        const trap = vi.fn((_reason: string) => {
          throw trapped;
        });
        vi.doMock('golem:api/host@1.5.0', () => ({ trap }));
        vi.resetModules();
        const { compensable, fallibleSaga, infallibleSaga } = await import('../src/host/saga');
        const events: string[] = [];
        let attempts = 0;
        const reserve = compensable(
          async (name: string) => {
            events.push(`reserve:${name}`);
            return Result.ok(name);
          },
          async (name) => {
            events.push(`release:${name}`);
            return Result.ok(undefined);
          },
        );
        const fail = compensable(
          async () => {
            throw new Error('unexpected failure');
          },
          async () => {
            events.push('unexpected compensation');
            return Result.ok(undefined);
          },
        );
        const body = async (
          saga:
            | Parameters<Parameters<typeof infallibleSaga>[0]>[0]
            | Parameters<Parameters<typeof fallibleSaga>[0]>[0],
        ) => {
          attempts++;
          if (attempts > 1) return Result.ok('unexpected retry');
          await saga.execute(reserve, 'flight');
          await saga.execute(reserve, 'hotel');
          if (location === 'body') throw new Error('unexpected failure');
          await saga.execute(fail, undefined);
          return Result.ok('unexpected success');
        };
        await expect(mode === 'fallible' ? fallibleSaga(body) : infallibleSaga(body)).rejects.toBe(
          trapped,
        );
        expect(attempts).toBe(1);
        expect(events).toEqual(['reserve:flight', 'reserve:hotel']);
        expect(trap).toHaveBeenCalledExactlyOnceWith(
          expect.stringContaining(`${mode}Saga failed:`),
        );
        expect(trap.mock.calls[0]?.[0]).toContain('unexpected failure');
      }
    },
  );

  it('allows fallible bodies to handle a failed step and continue', async () => {
    vi.doMock('golem:api/host@1.5.0', () => ({ trap: vi.fn() }));
    const { compensable, fallibleSaga } = await import('../src/host/saga');
    let undos = 0;
    const step = compensable(
      async (fail: boolean) => (fail ? Result.err('handled') : Result.ok(42)),
      async () => {
        undos++;
        return Result.ok(undefined);
      },
    );
    const result = await fallibleSaga(async (saga) => {
      expect((await saga.execute(step, true)).isErr()).toBe(true);
      return saga.execute(step, false);
    });
    expect(result.isOk()).toBe(true);
    expect(result.val).toBe(42);
    expect(undos).toBe(0);
  });

  it.each([false, true])('retries swallowed failure with follow-up=%s', async (followUp) => {
    vi.doMock('golem:api/host@1.5.0', () => ({ trap: vi.fn() }));
    const { compensable, infallibleSaga } = await import('../src/host/saga');
    let attempts = 0;
    let runs = 0;
    let undos = 0;
    const step = compensable(
      async () => {
        runs++;
        return Result.ok(1);
      },
      async () => {
        undos++;
        return Result.ok(undefined);
      },
    );
    const fail = compensable(
      async () => Result.err('retry'),
      async () => Result.ok(undefined),
    );
    const result = await infallibleSaga(async (saga) => {
      attempts++;
      await saga.execute(step, undefined);
      if (attempts === 1) {
        await saga.execute(fail, undefined).catch(() => undefined);
        if (followUp) {
          await expect(saga.execute(step, undefined)).rejects.toThrow();
          await expect(saga.execute(fail, undefined)).rejects.toThrow();
        }
        return 'swallowed';
      }
      return 'done';
    });
    expect(result).toBe('done');
    expect([attempts, runs, undos]).toEqual([2, 2, 1]);
  });

  it.each(['err', 'throw'])('fails loud after swallowed compensation %s', async (mode) => {
    const trap = vi.fn();
    vi.doMock('golem:api/host@1.5.0', () => ({ trap }));
    const { compensable, infallibleSaga } = await import('../src/host/saga');
    let attempts = 0;
    let undos = 0;
    let runs = 0;
    const step = compensable(
      async () => {
        runs++;
        return Result.ok(1);
      },
      async () => {
        undos++;
        if (mode === 'throw') throw new Error('broken compensation');
        return Result.err('broken compensation');
      },
    );
    const fail = compensable(
      async () => Result.err('retry'),
      async () => Result.ok(undefined),
    );
    await expect(
      infallibleSaga(async (saga) => {
        attempts++;
        await saga.execute(step, undefined);
        await saga.execute(fail, undefined).catch(() => undefined);
        await saga.execute(step, undefined).catch(() => undefined);
        return 'swallowed';
      }),
    ).rejects.toThrow();
    expect(trap).toHaveBeenCalledOnce();
    expect([attempts, runs, undos]).toEqual([1, 1, 1]);
  });

  it('compensates successive attempts in reverse and executes fresh operations', async () => {
    vi.doMock('golem:api/host@1.5.0', () => ({ trap: vi.fn() }));
    const { compensable, infallibleSaga } = await import('../src/host/saga');
    const trace: string[] = [];
    let identity = 0;
    let attempt = 0;
    const step = compensable(
      async (name: string) => {
        const id = ++identity;
        trace.push(`execute:${name}:${id}`);
        return Result.ok(id);
      },
      async (name, id) => {
        trace.push(`compensate:${name}:${id}`);
        return Result.ok(undefined);
      },
    );
    const settle = compensable(
      async () => (++attempt < 3 ? Result.err('retry') : Result.ok('done')),
      async () => Result.ok(undefined),
    );
    const result = await infallibleSaga(async (saga) => {
      await saga.execute(step, 'a');
      await saga.execute(step, 'b');
      return saga.execute(settle, undefined);
    });
    expect(result).toBe('done');
    expect(trace).toEqual([
      'execute:a:1',
      'execute:b:2',
      'compensate:b:2',
      'compensate:a:1',
      'execute:a:3',
      'execute:b:4',
      'compensate:b:4',
      'compensate:a:3',
      'execute:a:5',
      'execute:b:6',
    ]);
  });

  it('stops fallible compensation at the first failed compensation', async () => {
    vi.doMock('golem:api/host@1.5.0', () => ({ trap: vi.fn() }));
    const { compensable, fallibleSaga } = await import('../src/host/saga');
    const trace: number[] = [];
    const step = compensable(
      async (id: number) => Result.ok(id),
      async (id) => {
        trace.push(id);
        return Result.err('compensation');
      },
    );
    const result = await fallibleSaga(async (saga) => {
      await saga.execute(step, 1);
      await saga.execute(step, 2);
      return Result.err('body');
    });
    expect(result.val).toEqual({
      type: 'FailedAndRolledBackPartially',
      error: 'body',
      compensationFailure: 'compensation',
    });
    expect(trace).toEqual([2]);
  });

  it('keeps compensation stacks separate for overlapping sagas', async () => {
    vi.doMock('golem:api/host@1.5.0', () => ({ trap: vi.fn() }));
    const { compensable, infallibleSaga } = await import('../src/host/saga');
    const trace: string[] = [];
    const run = (name: string, failures: number) => {
      let attempt = 0;
      const step = compensable(
        async () => Result.ok(`${name}:${++attempt}`),
        async (_input, id) => {
          trace.push(id);
          return Result.ok(undefined);
        },
      );
      const settle = compensable(
        async () => (attempt <= failures ? Result.err('retry') : Result.ok(name)),
        async () => Result.ok(undefined),
      );
      return infallibleSaga(async (saga) => {
        await saga.execute(step, undefined);
        return saga.execute(settle, undefined);
      });
    };
    expect(await Promise.all([run('a', 2), run('b', 1)])).toEqual(['a', 'b']);
    expect(trace.filter((id) => id.startsWith('a:'))).toEqual(['a:1', 'a:2']);
    expect(trace.filter((id) => id.startsWith('b:'))).toEqual(['b:1']);
  });
});
