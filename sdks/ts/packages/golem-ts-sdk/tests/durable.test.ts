// `durable()` — persist a side effect's typed result on the live run, replay it
// (without re-running the body) during recovery. The host `golem:durability`
// interface is mocked with an in-memory oplog and live/replayed begin results so
// we can exercise both lifecycle branches and the fallible Result path.
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

beforeEach(() => {
  oplog = [];
  replayCursor = 0;
  live = true;
  pending = undefined;
  unfinishedDrops = 0;
});

afterEach(() => {
  vi.doUnmock('golem:durability/durability@1.6.0');
  vi.doUnmock('golem:api/host@1.5.0');
  vi.resetModules();
});

async function load() {
  vi.resetModules();
  vi.doMock('golem:durability/durability@1.6.0', () => {
    class MockLiveCustomDurableInvocation {
      private consumed = false;

      static finish(
        invocation: MockLiveCustomDurableInvocation,
        response: unknown,
        _forcedCommit: boolean,
      ) {
        invocation.consumed = true;
        oplog.push({ ...pending!, response });
      }

      [Symbol.dispose]() {
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
    };
  });
  vi.doMock('golem:api/host@1.5.0', () => ({
    getIdempotenceMode: () => false,
    setIdempotenceMode: () => {},
    markBeginOperation: () => 0n,
    markEndOperation: () => {},
    trap: () => {
      throw new Error('trap');
    },
  }));
  await import('../src/schema/zod'); // side-effect: registers the Zod walker
  const durableMod = await import('../src/host/durable');
  const resultMod = await import('../src/host/result');
  return {
    durable: durableMod.durable,
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

  it('supports async bodies', async () => {
    const { durable, FunctionType } = await load();
    const out = await durable(baseSpec(FunctionType), { symbol: 'X' }, async () => ({
      symbol: 'X',
      price: 7,
    }));
    expect(out).toEqual({ symbol: 'X', price: 7 });
    expect(oplog).toHaveLength(1);
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

  it('drops an unfinished invocation when an async body rejects', async () => {
    const { durable, FunctionType } = await load();

    await expect(
      durable(baseSpec(FunctionType), { symbol: 'X' }, async () => {
        throw new Error('failed');
      }),
    ).rejects.toThrow('failed');
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
