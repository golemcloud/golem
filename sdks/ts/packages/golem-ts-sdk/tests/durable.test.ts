// `durable()` and `durableAsync()` — persist a side effect's typed result on the live run, replay
// it (without re-running the body) during recovery. The host `golem:durability` interface is
// mocked with an in-memory oplog and live/replayed begin results so we can exercise both
// lifecycle branches and the fallible Result path. `wasm-rquickjs:custom-invocation-context` is
// mocked with one current token; it does not follow `await` like the runtime does, so the scope
// tests observe only the synchronous parts of each step.
//
// Uses the `vi.doMock` + `vi.resetModules()` + dynamic-import pattern (see
// retry.test.ts) — a top-level `vi.mock` does not intercept these aliased WIT
// binding modules.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import type { DurableFunctionType } from 'golem:durability/durability@1.6.0';

interface OplogEntry {
  functionName: string;
  response: unknown;
  functionType: { tag: string };
}

let oplog: OplogEntry[];
let replayCursor: number;
let live: boolean;
let pending: { functionName: string; functionType: { tag: string } } | undefined;
let unfinishedDrops: number;

interface MockToken {
  name: string;
  parent?: MockToken;
  openChildren: number;
  closed: boolean;
}

let root: MockToken;
let scope: { current: MockToken };
/** `step:operation@scope` for every host call, in call order. */
let events: string[];

beforeEach(() => {
  oplog = [];
  replayCursor = 0;
  live = true;
  pending = undefined;
  unfinishedDrops = 0;
  root = { name: 'root', openChildren: 0, closed: false };
  scope = { current: root };
  events = [];
});

afterEach(() => {
  vi.doUnmock('golem:durability/durability@1.6.0');
  vi.doUnmock('wasm-rquickjs:custom-invocation-context');
  vi.doUnmock('golem:api/host@1.5.0');
  vi.doUnmock('golem:api/retry@1.5.0');
  vi.resetModules();
});

async function load() {
  vi.resetModules();
  vi.doMock('golem:durability/durability@1.6.0', () => {
    class MockLiveCustomDurableInvocation {
      private consumed = false;

      constructor(
        readonly functionName = '',
        readonly functionType = { tag: '' },
      ) {}

      enter() {
        events.push(`enter:${this.functionName}@${scope.current.name}`);
      }

      static finishAsync(
        invocation: MockLiveCustomDurableInvocation,
        response: unknown,
        _forcedCommit: boolean,
      ) {
        events.push(`finish:${invocation.functionName}@${scope.current.name}`);
        invocation.consumed = true;
        oplog.push({
          functionName: invocation.functionName,
          functionType: invocation.functionType,
          response,
        });
        return Promise.resolve();
      }

      static finish(
        invocation: MockLiveCustomDurableInvocation,
        response: unknown,
        _forcedCommit: boolean,
      ) {
        invocation.consumed = true;
        oplog.push({ ...pending!, response });
      }

      [Symbol.dispose]() {
        if (this.functionName) events.push(`drop:${this.functionName}@${scope.current.name}`);
        if (!this.consumed) {
          this.consumed = true;
          unfinishedDrops += 1;
        }
      }
    }

    return {
      observeFunctionCall: () => {},
      LiveCustomDurableInvocation: MockLiveCustomDurableInvocation,
      beginCustomDurableInvocation: (
        functionName: string,
        _request: unknown,
        functionType: { tag: string },
      ) => {
        pending = { functionName, functionType };
        return live
          ? { tag: 'live', val: new MockLiveCustomDurableInvocation() }
          : { tag: 'replayed', val: oplog[replayCursor++] };
      },
      beginCustomDurableInvocationAsync: (
        functionName: string,
        _request: unknown,
        functionType: { tag: string },
      ) => {
        events.push(`begin:${functionName}@${scope.current.name}`);
        return Promise.resolve(
          live
            ? { tag: 'live', val: new MockLiveCustomDurableInvocation(functionName, functionType) }
            : { tag: 'replayed', val: oplog[replayCursor++] },
        );
      },
    };
  });
  vi.doMock('wasm-rquickjs:custom-invocation-context', () => ({
    capture: () => scope.current,
    swap: (next: MockToken) => {
      const previous = scope.current;
      scope.current = next;
      return previous;
    },
    run: <T>(token: MockToken, fn: () => T): T => {
      const previous = scope.current;
      scope.current = token;
      try {
        return fn();
      } finally {
        scope.current = previous;
      }
    },
    enterLive: (invocation: { enter(): void; functionName: string }) => {
      invocation.enter();
      const token = {
        name: invocation.functionName,
        parent: scope.current,
        openChildren: 0,
        closed: false,
      };
      scope.current.openChildren += 1;
      scope.current = token;
      return token;
    },
    // Like the runtime: closing a scope with open children throws.
    close: (token: MockToken) => {
      if (token.closed) return;
      if (token.openChildren > 0) throw new Error(`${token.name} closed with open children`);
      token.closed = true;
      token.parent!.openChildren -= 1;
      if (scope.current === token) scope.current = token.parent!;
    },
  }));
  vi.doMock('golem:api/host@1.5.0', () => ({
    getSelfMetadata: () => ({
      agentId: { componentId: { uuid: { highBits: 0n, lowBits: 1n } }, agentId: 'TestAgent()' },
      args: [],
      env: [],
      config: [],
      status: 'idle',
      componentRevision: 0n,
      retryCount: 0n,
      environmentId: { uuid: { highBits: 0n, lowBits: 2n } },
    }),
    trap: () => {
      throw new Error('trap');
    },
  }));
  vi.doMock('golem:api/retry@1.5.0', () => ({
    resolveRetryPolicy: () => undefined,
    nextRetryDelay: (_policy: unknown, state: Uint8Array) => ({
      state: new Uint8Array([(state[0] ?? 0) + 1]),
      delay: state[0] === 2 ? undefined : 0n,
    }),
  }));
  await import('../src/schema/zod'); // side-effect: registers the Zod walker
  const durableMod = await import('../src/host/durable');
  const resultMod = await import('../src/host/result');
  return {
    durable: durableMod.durable,
    durableAsync: durableMod.durableAsync,
    FunctionType: durableMod.FunctionType,
    Result: resultMod.Result,
  };
}

const baseSpec = (FunctionType: { writeRemote: DurableFunctionType }) => ({
  iface: 'host-features',
  function: 'fetchQuote',
  functionType: FunctionType.writeRemote,
  requestSchema: z.object({ symbol: z.string() }),
  success: z.object({ symbol: z.string(), price: z.number() }),
});

describe('durable()', () => {
  it('appends compensated saga attempts and replays their complete history', async () => {
    const { durable, FunctionType, Result } = await load();
    const { compensable, infallibleSaga } = await import('../src/host/saga');
    const effects: string[] = [];
    let identity = 0;
    let settlements = 0;
    const spec = {
      iface: 'saga',
      function: 'reserve',
      functionType: FunctionType.writeRemote,
      requestSchema: z.string(),
      success: z.number(),
    };
    const reserve = compensable(
      async (name: string) =>
        Result.ok(
          durable(spec, name, () => {
            const id = ++identity;
            effects.push(`reserve:${name}:${id}`);
            return id;
          }),
        ),
      async (name, id) => {
        durable({ ...spec, function: 'release' }, `${name}:${id}`, () => {
          effects.push(`release:${name}:${id}`);
          return id;
        });
        return Result.ok(undefined);
      },
    );
    const settle = compensable(
      async (ids: string) =>
        durable({ ...spec, function: 'settle', error: z.string() }, ids, () => {
          settlements++;
          effects.push(`settle:${ids}`);
          return settlements < 3 ? Result.err('declined') : Result.ok(73);
        }),
      async () => Result.ok(undefined),
    );
    const run = () =>
      infallibleSaga(async (saga) => {
        const flight = await saga.execute(reserve, 'flight');
        const hotel = await saga.execute(reserve, 'hotel');
        const receipt = await saga.execute(settle, `${flight},${hotel}`);
        return { flight, hotel, receipt };
      });

    expect((await run()).val).toEqual({ flight: 5, hotel: 6, receipt: 73 });
    expect(effects).toEqual([
      'reserve:flight:1',
      'reserve:hotel:2',
      'settle:1,2',
      'release:hotel:2',
      'release:flight:1',
      'reserve:flight:3',
      'reserve:hotel:4',
      'settle:3,4',
      'release:hotel:4',
      'release:flight:3',
      'reserve:flight:5',
      'reserve:hotel:6',
      'settle:5,6',
    ]);
    expect(oplog.map((entry) => entry.functionName)).toEqual([
      'saga::reserve',
      'saga::reserve',
      'saga::settle',
      'saga::release',
      'saga::release',
      'saga::reserve',
      'saga::reserve',
      'saga::settle',
      'saga::release',
      'saga::release',
      'saga::reserve',
      'saga::reserve',
      'saga::settle',
    ]);
    const history = structuredClone(oplog);
    const liveEffects = [...effects];
    live = false;
    replayCursor = 0;
    expect((await run()).val).toEqual({ flight: 5, hotel: 6, receipt: 73 });
    expect(replayCursor).toBe(13);
    expect(oplog).toEqual(history);
    expect(effects).toEqual(liveEffects);
    expect(unfinishedDrops).toBe(0);
  });

  it('persists the success value on the live run and returns it', async () => {
    const { durable, FunctionType } = await load();
    const out = durable(baseSpec(FunctionType), { symbol: 'GOLEM' }, () => ({
      symbol: 'GOLEM',
      price: 42,
    }));
    expect(out).toEqual({ symbol: 'GOLEM', price: 42 });
    expect(oplog).toHaveLength(1);
    expect(oplog[0]!.functionName).toBe('host-features::fetchQuote');
    expect(oplog[0]!.functionType.tag).toBe('write-remote');
  });

  it('uses a plain function name when the interface name is empty', async () => {
    const { durable, FunctionType } = await load();
    const spec = { ...baseSpec(FunctionType), iface: '', function: 'generate' };

    durable(spec, { symbol: 'GOLEM' }, () => ({ symbol: 'GOLEM', price: 42 }));

    expect(oplog[0]!.functionName).toBe('generate');
  });

  it('replays the persisted value WITHOUT re-running the body', async () => {
    const { durable, FunctionType } = await load();
    const spec = baseSpec(FunctionType);

    // Live run records the (random) price.
    const first = durable(spec, { symbol: 'GOLEM' }, () => ({
      symbol: 'GOLEM',
      price: Math.floor(Math.random() * 1000),
    })) as { price: number };

    // Recovery: rewind the oplog, replay mode; the body must NOT run again.
    replayCursor = 0;
    live = false;
    const body = vi.fn(() => ({ symbol: 'GOLEM', price: -1 }));
    const replayed = durable(spec, { symbol: 'GOLEM' }, body) as { price: number };

    expect(body).not.toHaveBeenCalled();
    expect(replayed).toEqual(first);
  });

  it('persists + replays a fallible Result (both ok and err)', async () => {
    const { durable, FunctionType, Result } = await load();
    const fspec = {
      ...baseSpec(FunctionType),
      function: 'maybeQuote',
      error: z.object({ code: z.string() }),
    };

    const okOut = durable(fspec, { symbol: 'A' }, () => Result.ok({ symbol: 'A', price: 1 }));
    const errOut = durable(fspec, { symbol: 'B' }, () => Result.err({ code: 'UNAVAILABLE' }));
    expect(okOut.isOk()).toBe(true);
    expect(errOut.isErr()).toBe(true);
    expect(oplog).toHaveLength(2);

    // Replay both — bodies would return different values; the persisted ones win.
    replayCursor = 0;
    live = false;
    const rOk = durable(fspec, { symbol: 'A' }, () => Result.ok({ symbol: 'A', price: 999 }));
    const rErr = durable(fspec, { symbol: 'B' }, () => Result.ok({ symbol: 'B', price: 0 }));
    expect(rOk.isOk()).toBe(true);
    expect(rErr.isErr()).toBe(true);
  });

  it('rejects an async body and leaves the operation unfinished', async () => {
    const { durable, FunctionType } = await load();

    expect(() =>
      durable(baseSpec(FunctionType), { symbol: 'X' }, (async () => ({
        symbol: 'X',
        price: 7,
      })) as never),
    ).toThrow(TypeError);
    expect(oplog).toHaveLength(0);
    expect(unfinishedDrops).toBe(1);
  });

  it('drops an unfinished invocation when a synchronous body throws', async () => {
    const { durable, FunctionType } = await load();

    expect(() =>
      durable(baseSpec(FunctionType), { symbol: 'X' }, () => {
        throw new Error('failed');
      }),
    ).toThrow('failed');
    expect(oplog).toHaveLength(0);
    expect(unfinishedDrops).toBe(1);
  });

  it('drops an unfinished invocation when response encoding fails', async () => {
    const { durable, FunctionType } = await load();

    expect(() =>
      durable(baseSpec(FunctionType), { symbol: 'X' }, () => ({
        symbol: 'X',
        price: 'not-a-number' as never,
      })),
    ).toThrow();
    expect(oplog).toHaveLength(0);
    expect(unfinishedDrops).toBe(1);
  });
});

const opSpec = (FunctionType: { writeRemote: DurableFunctionType }, name: string) => ({
  iface: '',
  function: name,
  functionType: FunctionType.writeRemote,
  requestSchema: z.string(),
  success: z.string(),
});

describe('durableAsync()', () => {
  it('runs the body in its own scope and finishes and drops in it', async () => {
    const { durableAsync, FunctionType } = await load();

    const out = await durableAsync(opSpec(FunctionType, 'op'), 'req', async () => {
      events.push(`body@${scope.current.name}`);
      return 'done';
    });

    expect(out).toBe('done');
    expect(events).toEqual([
      'begin:op@root',
      'enter:op@root',
      'body@op',
      'finish:op@op',
      'drop:op@op',
    ]);
    expect(scope.current).toBe(root);
    expect(root.openChildren).toBe(0);
    expect(oplog.map((entry) => entry.functionName)).toEqual(['op']);
    expect(unfinishedDrops).toBe(0);
  });

  it('replays the persisted value without entering a scope or re-running the body', async () => {
    const { durableAsync, FunctionType } = await load();
    const spec = opSpec(FunctionType, 'op');
    const first = await durableAsync(spec, 'req', async () => `live-${Math.random()}`);

    live = false;
    replayCursor = 0;
    events = [];
    const body = vi.fn(async () => 'other');
    const replayed = await durableAsync(spec, 'req', body);

    expect(body).not.toHaveBeenCalled();
    expect(replayed).toBe(first);
    expect(events).toEqual(['begin:op@root']);
  });

  it('drops the unfinished invocation in its scope when the body rejects', async () => {
    const { durableAsync, FunctionType } = await load();

    await expect(
      durableAsync(opSpec(FunctionType, 'op'), 'req', async () => {
        throw new Error('failed');
      }),
    ).rejects.toThrow('failed');
    expect(events).toEqual(['begin:op@root', 'enter:op@root', 'drop:op@op']);
    expect(oplog).toHaveLength(0);
    expect(unfinishedDrops).toBe(1);
    expect(scope.current).toBe(root);
    expect(root.openChildren).toBe(0);
  });

  it('drops the unfinished invocation when response encoding fails', async () => {
    const { durableAsync, FunctionType } = await load();

    await expect(
      durableAsync(opSpec(FunctionType, 'op'), 'req', async () => 42 as never),
    ).rejects.toThrow();
    expect(oplog).toHaveLength(0);
    expect(unfinishedDrops).toBe(1);
  });

  it('persists and replays a fallible Result', async () => {
    const { durableAsync, FunctionType, Result } = await load();
    const spec = { ...opSpec(FunctionType, 'maybe'), error: z.object({ code: z.string() }) };

    const errOut = await durableAsync(spec, 'req', async () => Result.err({ code: 'DOWN' }));
    expect(errOut.isErr()).toBe(true);

    live = false;
    replayCursor = 0;
    const replayed = await durableAsync(spec, 'req', async () => Result.ok('fine'));
    expect(replayed.isErr()).toBe(true);
  });

  it('opens operations started in the body as children and finishes the parent after them', async () => {
    const { durableAsync, FunctionType } = await load();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));

    const parent = durableAsync(opSpec(FunctionType, 'parent'), 'p', async () => {
      // Started and not awaited: the parent must still wait for it.
      void durableAsync(opSpec(FunctionType, 'child'), 'c', async () => {
        await gate;
        return 'c';
      });
      return 'p';
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(events).not.toContain('finish:parent@parent');

    release();
    expect(await parent).toBe('p');

    expect(events).toContain('begin:child@parent');
    expect(events).toContain('enter:child@parent');
    expect(events.indexOf('finish:child@child')).toBeLessThan(
      events.indexOf('finish:parent@parent'),
    );
    expect(oplog.map((entry) => entry.functionName)).toEqual(['child', 'parent']);
    expect(scope.current).toBe(root);
    expect(root.openChildren).toBe(0);
  });
});
