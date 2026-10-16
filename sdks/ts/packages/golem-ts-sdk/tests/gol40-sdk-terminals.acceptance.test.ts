// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { createOutput, ToolRpc } from 'golem:tool/host@0.1.0';
import type { InvocationResult, TypedSchemaValue } from 'golem:tool/host@0.1.0';
import { KeyValue, err, ok, s, tool, toolDefinition } from '../src';
import {
  createToolClientRuntime,
  createToolClientTransport,
  mapSettledToolResult,
  splitToolRpcError,
  startedToolInvocation,
  type ToolRpcError,
} from '../src/bridge/tool';
import { secretHandleFromSchemaValue } from '../src/bridge/schema';
import { ToolRegistry } from '../src/internal/registry/toolRegistry';
import {
  GuestSchemaValueStreamHandle,
  field,
  schemaGraphToWit,
  t,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
} from '../src/internal/schema-model';
import {
  deferredStartedToolInvocation,
  type SettledToolResult,
} from '../src/internal/tool/startedToolInvocation';
import { encodeTool } from '../src/internal/tool';
import { getExtendedToolDefinition } from '../src/tool';
import { ToolType } from '../src/toolReflection';
import { relinquishSchemaValueCapabilities } from '../src/schema/codec';

const ArtifactReport = z.object({
  artifactId: s.u64(),
  digest: z.string(),
  labels: KeyValue(z.string()),
  warnings: z.array(z.string()),
});

beforeEach(() => ToolRegistry.clearForTests());

function outputWriter() {
  const chunks: number[] = [];
  return {
    chunks,
    write: vi.fn(async (chunk: Uint8Array) => {
      chunks.push(...chunk);
    }),
    finish: vi.fn(async () => undefined),
    fail: vi.fn(async () => undefined),
    [Symbol.dispose]: vi.fn(),
  };
}

function gate<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

const flush = async () => {
  for (let i = 0; i < 30; i++) await Promise.resolve();
};
const emptyInput = () =>
  typedSchemaValueToWit({
    graph: { defs: new Map(), root: t.record([]) },
    value: { tag: 'record', fields: [] },
  });

describe('GOL-40 TypeScript SDK terminal acceptance', () => {
  it.each(['success', 'custom-error', 'decoded-error', 'forwarded-rpc'])(
    'releases failed mapping capabilities but preserves %s delivery',
    async (kind) => {
      const secret = { [Symbol.dispose]: vi.fn() };
      const payload = {
        graph: schemaGraphToWit({ defs: new Map(), root: t.secret(t.string()) }),
        value: { root: 0, valueNodes: [{ tag: 'secret-value', val: secret }] },
      } as unknown as TypedSchemaValue;
      const future = {
        get: vi.fn(async () => {
          if (kind !== 'success')
            throw {
              tag: 'remote-tool-error',
              val: { tag: 'custom-error', val: { name: 'denied', payload } },
            };
          return { result: payload };
        }),
        cancel: vi.fn(),
        [Symbol.dispose]: vi.fn(),
      };
      vi.mocked(ToolRpc).mockImplementationOnce(
        () => ({ asyncInvokeAndAwait: () => future }) as unknown as ToolRpc,
      );
      const raw = createToolClientTransport('mapping-owner').start(
        [],
        emptyInput(),
        undefined,
        false,
        false,
      );
      const failure = new Error('decode failure');
      const mapped = mapSettledToolResult(
        raw.settledResult,
        () => {
          throw failure;
        },
        (error) => {
          if (kind === 'decoded-error')
            throw splitToolRpcError(error as ToolRpcError, (_name, typed) => ({
              tag: 'denied',
              value: secretHandleFromSchemaValue(typed.value),
            }));
          if (kind === 'forwarded-rpc') throw { tag: 'rpc', error };
          throw failure;
        },
      );
      const invocation = startedToolInvocation(undefined, undefined, mapped, () => raw.cancel());
      if (kind === 'decoded-error')
        await expect(invocation.result).rejects.toMatchObject({
          tag: 'tool',
          error: { tag: 'denied', value: secret },
        });
      else if (kind === 'forwarded-rpc')
        await expect(invocation.result).rejects.toMatchObject({
          tag: 'rpc',
          error: { tag: 'remote-tool-error' },
        });
      else await expect(invocation.result).rejects.toBe(failure);
      invocation.detachResult();
      await flush();
      expect(secret[Symbol.dispose].mock.calls.length).toBe(
        kind === 'decoded-error' || kind === 'forwarded-rpc' ? 0 : 1,
      );
      expect(future.cancel).not.toHaveBeenCalled();
    },
  );

  it.each(['settled-success', 'settled-error', 'same-turn-success', 'same-turn-error'])(
    'suppresses observer-less %s when detached before public delivery',
    async (kind) => {
      const terminal = gate<SettledToolResult<string>>();
      const outcome: SettledToolResult<string> = kind.endsWith('success')
        ? { status: 'fulfilled', value: 'undelivered' }
        : { status: 'rejected', reason: new Error('undelivered error') };
      if (kind.startsWith('settled')) terminal.resolve(outcome);
      const invocation = startedToolInvocation(undefined, undefined, terminal.promise, () => {});
      const result = invocation.result;
      if (kind.startsWith('same-turn')) terminal.resolve(outcome);
      invocation.detachResult();
      await expect(result).rejects.toThrow('observer is detached');
    },
  );

  it.each(['success', 'custom-error'])(
    'retains the unclaimed %s until collect returns, not until internal result settlement',
    async (kind) => {
      const terminal = gate<InvocationResult>();
      const outputEnd = gate<void>();
      const secret = { [Symbol.dispose]: vi.fn() };
      const payload = {
        graph: schemaGraphToWit({ defs: new Map(), root: t.secret(t.string()) }),
        value: { root: 0, valueNodes: [{ tag: 'secret-value', val: secret }] },
      } as unknown as TypedSchemaValue;
      const future = {
        get: vi.fn(() => terminal.promise),
        cancel: vi.fn(),
        [Symbol.dispose]: vi.fn(),
      };
      vi.mocked(ToolRpc).mockImplementationOnce(
        () => ({ asyncInvokeAndAwait: () => future }) as unknown as ToolRpc,
      );
      vi.mocked(createOutput).mockReturnValueOnce([
        {},
        {
          async *[Symbol.asyncIterator]() {
            yield { tag: 'ok', val: Uint8Array.of(4, 0, 255) };
            await outputEnd.promise;
            yield { tag: 'ok', val: Uint8Array.of(9) };
          },
        },
      ] as never);
      const raw = createToolClientRuntime('collect-owner').start(
        [],
        typedSchemaValueFromWit(emptyInput()),
        undefined,
        true,
        false,
      );
      const invocation = startedToolInvocation(
        raw.stdout,
        raw.stderr,
        mapSettledToolResult(raw.settledResult, (value) => value.result),
        () => raw.cancel(),
      );
      const collected = invocation.collect();
      if (kind === 'success') terminal.resolve({ result: payload });
      else
        terminal.reject({
          tag: 'remote-tool-error',
          val: { tag: 'custom-error', val: { name: 'denied', payload } },
        });
      await flush();
      expect(secret[Symbol.dispose]).not.toHaveBeenCalled();
      invocation.detachResult();
      await flush();
      expect(secret[Symbol.dispose]).toHaveBeenCalledOnce();
      outputEnd.resolve();
      const outcome = await collected;
      expect(outcome.result).toMatchObject({
        status: 'rejected',
        reason: expect.objectContaining({ message: 'tool result observer is detached' }),
      });
      expect(outcome.stdout).toEqual({ status: 'fulfilled', value: Uint8Array.of(4, 0, 255, 9) });
      expect(future.cancel).not.toHaveBeenCalled();
    },
  );

  it.each([
    ['invokeValue', 'stdout', false],
    ['invokeValue', 'stderr', false],
    ['invokeValue', 'both', false],
    ['invokeValue', 'neither', false],
    ['collect', 'both', false],
    ['invokeValue', 'both', true],
  ])(
    'keeps reflected %s result ownership independent from %s failures (result failure: %s)',
    async (method, failure, resultFails) => {
      const secret = { [Symbol.dispose]: vi.fn() };
      const resultError = new Error('result failure');
      const future = {
        get: vi.fn(async () => {
          if (resultFails) throw resultError;
          return {
            result: {
              graph: schemaGraphToWit({ defs: new Map(), root: t.secret(t.string()) }),
              value: { root: 0, valueNodes: [{ tag: 'secret-value', val: secret }] },
            },
          };
        }),
        cancel: vi.fn(),
        [Symbol.dispose]: vi.fn(),
      };
      vi.mocked(ToolRpc).mockImplementationOnce(
        () => ({ asyncInvokeAndAwait: () => future }) as unknown as ToolRpc,
      );
      for (const channel of ['stdout', 'stderr']) {
        vi.mocked(createOutput).mockReturnValueOnce([
          {},
          {
            async *[Symbol.asyncIterator]() {
              if (failure === channel || failure === 'both')
                yield { tag: 'err', val: { tag: 'failed', val: `${channel} failed` } };
              else yield { tag: 'ok', val: Uint8Array.of(4, 0, 255, 9) };
            },
          },
        ] as never);
      }
      const definition = toolDefinition('hidden-result').body((body) =>
        body.stdout({ required: false }).stderr({ required: false }).returns(s.secret(z.string())),
      );
      const command = new ToolType(
        {
          lookupName: 'hidden-result',
          definition: encodeTool(getExtendedToolDefinition(definition)),
          implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
        },
        createToolClientRuntime('hidden-result'),
      ).client.command([]);
      const input = command.packJson({});
      if (method === 'collect') {
        const invocation = command.startValue(input);
        const collected = await invocation.collect();
        expect(collected.result.status).toBe('fulfilled');
        expect(collected.stdout.status).toBe('rejected');
        expect(collected.stderr.status).toBe('rejected');
        invocation.detachResult();
      } else if (resultFails) {
        await expect(command.invokeValue(input)).rejects.toBe(resultError);
      } else if (failure === 'neither') {
        await expect(command.invokeValue(input)).resolves.toMatchObject({ tag: 'secret' });
      } else {
        await expect(command.invokeValue(input)).rejects.toThrow(
          failure === 'stderr' ? 'stderr failed' : 'stdout failed',
        );
      }
      await flush();
      expect(secret[Symbol.dispose]).toHaveBeenCalledTimes(
        method === 'invokeValue' && failure !== 'neither' && !resultFails ? 1 : 0,
      );
      expect(future.cancel).not.toHaveBeenCalled();
    },
  );

  it.each([
    ['success', 'startJson'],
    ['custom-error', 'startJson'],
    ['success', 'invokeJson'],
    ['custom-error', 'invokeJson'],
  ])(
    'releases an undisclosed reflected %s stream when %s conversion fails',
    async (kind, method) => {
      const definition = toolDefinition('json-stream-result').body((body) =>
        body.returns(s.stream(z.string())).error('denied', {
          kind: 'runtime',
          exitCode: 1,
          payload: s.stream(z.string()),
        }),
      );
      const returned = vi.fn(async () => ({ done: true as const, value: undefined }));
      const stream = {
        reader: {
          [Symbol.asyncIterator]: () => ({
            next: async () => ({ done: true as const, value: undefined }),
            return: returned,
          }),
        },
        [Symbol.dispose]: vi.fn(),
      };
      const payload = {
        graph: schemaGraphToWit({ defs: new Map(), root: t.stream(t.string()) }),
        value: { root: 0, valueNodes: [{ tag: 'stream-value', val: stream }] },
      } as unknown as TypedSchemaValue;
      const future = {
        get: vi.fn(async () => {
          if (kind === 'custom-error')
            throw {
              tag: 'remote-tool-error',
              val: { tag: 'custom-error', val: { name: 'denied', payload } },
            };
          return { result: payload };
        }),
        cancel: vi.fn(),
        [Symbol.dispose]: vi.fn(),
      };
      vi.mocked(ToolRpc).mockImplementationOnce(
        () => ({ asyncInvokeAndAwait: () => future }) as unknown as ToolRpc,
      );
      const command = new ToolType(
        {
          lookupName: 'json-stream-result',
          definition: encodeTool(getExtendedToolDefinition(definition)),
          implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
        },
        createToolClientRuntime('json-stream-result'),
      ).client.command([]);
      const invocation = method === 'startJson' ? command.startJson({}) : undefined;
      await expect(invocation ? invocation.result : command.invokeJson({})).rejects.toBeInstanceOf(
        Error,
      );
      await flush();
      expect(returned).toHaveBeenCalledOnce();
      expect(stream[Symbol.dispose]).toHaveBeenCalledOnce();
      expect(future.cancel).not.toHaveBeenCalled();
      invocation?.detachResult();
      await flush();
      expect(returned).toHaveBeenCalledOnce();
      expect(stream[Symbol.dispose]).toHaveBeenCalledOnce();
    },
  );

  it.each([
    'success',
    'declared',
    'malformed',
    'undeclared',
    'crash',
    'explicit',
    'cancelled-success',
    'cancelled-declared',
  ] as const)(
    'keeps partial bytes, terminals, structured %s outcome and cleanup independent',
    async (kind) => {
      const crash = new Error('handler crash');
      const explicit = { tag: 'consumer-closed' };
      const cancelled = { tag: 'closed', val: { tag: 'failed', val: { tag: 'cancelled' } } };
      toolDefinition('terminal-matrix')
        .body((body) =>
          body
            .stdout({ required: true })
            .stderr({ required: true })
            .returns(z.string())
            .error('denied', { kind: 'runtime', exitCode: 1, payload: z.string() }),
        )
        .implement({
          'terminal-matrix': async (_, context) => {
            const stdout = context.stdout.getWriter();
            await stdout.write(Uint8Array.of(0, 255, 17));
            await context.stderr.getWriter().write(Uint8Array.of(8));
            if (kind === 'crash') throw crash;
            if (kind === 'explicit') await stdout.close();
            if (kind === 'declared' || kind === 'cancelled-declared')
              return err('denied', 'reason');
            if (kind === 'undeclared') return err('other', 'reason') as never;
            if (kind === 'malformed') return { tag: 'ok' } as never;
            return ok('selected');
          },
        });
      const stdout = outputWriter();
      const stderr = outputWriter();
      stdout.finish.mockRejectedValue(kind.startsWith('cancelled-') ? cancelled : explicit);
      stderr.finish.mockRejectedValue({ tag: 'concurrent-operation' });
      stdout[Symbol.dispose].mockImplementation(() => {
        throw new Error('cleanup failure');
      });
      const invocation = tool.invoke(
        'terminal-matrix',
        [],
        emptyInput(),
        undefined,
        stdout,
        stderr,
        { tag: 'anonymous' },
      );
      if (kind === 'success' || kind === 'cancelled-success') {
        const terminal = await invocation;
        expect(typedSchemaValueFromWit(terminal.result!).value).toEqual({
          tag: 'string',
          value: 'selected',
        });
      } else if (kind === 'crash') await expect(invocation).rejects.toBe(crash);
      else if (kind === 'explicit') await expect(invocation).rejects.toBe(explicit);
      else
        await expect(invocation).rejects.toMatchObject({
          tag:
            kind === 'declared' || kind === 'cancelled-declared'
              ? 'custom-error'
              : 'invalid-result',
        });
      expect(stdout.chunks).toEqual([0, 255, 17]);
      expect(stderr.chunks).toEqual([8]);
      expect(stdout[Symbol.dispose]).toHaveBeenCalledOnce();
      expect(stderr[Symbol.dispose]).toHaveBeenCalledOnce();
      if (kind === 'crash') {
        expect(stdout.finish).not.toHaveBeenCalled();
        expect(stderr.finish).not.toHaveBeenCalled();
        expect(stdout.fail).toHaveBeenCalledOnce();
        expect(stderr.fail).toHaveBeenCalledOnce();
      } else if (kind === 'explicit') {
        expect(stdout.finish).toHaveBeenCalledOnce();
        expect(stdout.fail).not.toHaveBeenCalled();
        expect(stderr.fail).toHaveBeenCalledOnce();
      } else {
        expect(stdout.finish).toHaveBeenCalledOnce();
        expect(stderr.finish).toHaveBeenCalledOnce();
        expect(stdout.fail).not.toHaveBeenCalled();
        expect(stderr.fail).not.toHaveBeenCalled();
      }
    },
  );

  it('propagates a queued output failure while its sibling is backpressured', async () => {
    const failure = new Error('write failed');
    const blocked = gate<void>();
    const stdout = outputWriter();
    const stderr = outputWriter();
    stdout.write.mockRejectedValue(failure);
    stderr.write.mockImplementation(() => blocked.promise);
    stderr.fail.mockImplementation(async () => {
      blocked.resolve();
    });
    toolDefinition('blocked-sibling')
      .body((body) =>
        body.stdout({ required: true }).stderr({ required: true }).returns(z.string()),
      )
      .implement({
        'blocked-sibling': async (_, context) => {
          void context.stdout
            .getWriter()
            .write(Uint8Array.of(1, 255))
            .catch(() => {});
          void context.stderr
            .getWriter()
            .write(Uint8Array.of(0))
            .catch(() => {});
          return ok('done');
        },
      });
    const outcome = tool
      .invoke('blocked-sibling', [], emptyInput(), undefined, stdout, stderr, {
        tag: 'anonymous',
      })
      .then(
        (value) => ({ status: 'fulfilled', value }),
        (reason) => ({ status: 'rejected', reason }),
      );
    try {
      const observed = await Promise.race([
        outcome,
        new Promise((resolve) => setTimeout(() => resolve('blocked'), 50)),
      ]);
      expect(observed).toEqual({ status: 'rejected', reason: failure });
      expect(stderr.fail).toHaveBeenCalledOnce();
      expect(stdout[Symbol.dispose]).toHaveBeenCalledOnce();
      expect(stderr[Symbol.dispose]).toHaveBeenCalledOnce();
    } finally {
      blocked.resolve();
      await outcome;
    }
  });

  it('permanently closes stdout without closing stderr, result, or invocation', async () => {
    const closed = vi.fn();
    const output = (bytes: number[]) => ({
      [Symbol.asyncIterator]() {
        let emitted = false;
        return {
          async next() {
            if (emitted) return { done: true as const, value: undefined };
            emitted = true;
            return {
              done: false as const,
              value: { tag: 'ok' as const, val: Uint8Array.from(bytes) },
            };
          },
          async return() {
            closed();
            return { done: true as const, value: undefined };
          },
        };
      },
    });
    const cancel = vi.fn();
    const invocation = startedToolInvocation(
      output([1, 2, 255]),
      output([9, 0]),
      Promise.resolve({ status: 'fulfilled', value: 'done' }),
      cancel,
    );
    const reader = invocation.stdout!.getReader();
    expect((await reader.read()).value).toEqual(Uint8Array.of(1, 2, 255));
    await reader.cancel();
    expect(await reader.read()).toEqual({ done: true, value: undefined });
    const stderr = invocation.stderr!.getReader();
    expect((await stderr.read()).value).toEqual(Uint8Array.of(9, 0));
    expect(await invocation.result).toBe('done');
    expect(closed).toHaveBeenCalledOnce();
    expect(cancel).not.toHaveBeenCalled();
    invocation.cancel();
    expect(cancel).toHaveBeenCalledOnce();
  });

  it.each(['unobserved', 'during-get', 'after-terminal', 'custom-error'] as const)(
    'detaches generated-wrapper ownership %s and releases late capabilities',
    async (phase) => {
      const terminal = gate<InvocationResult>();
      const future = {
        get: vi.fn(() => terminal.promise),
        cancel: vi.fn(),
        [Symbol.dispose]: vi.fn(),
      };
      vi.mocked(ToolRpc).mockImplementationOnce(
        () => ({ asyncInvokeAndAwait: () => future }) as unknown as ToolRpc,
      );
      const runtime = createToolClientRuntime('observer-owner');
      const raw = runtime.start([], typedSchemaValueFromWit(emptyInput()), undefined, false, false);
      // This is the existing generated Promise client's mapping and cancel-closure seam.
      const mapped = mapSettledToolResult(raw.settledResult, (value) => value.result);
      const invocation = startedToolInvocation(raw.stdout, raw.stderr, mapped, () => raw.cancel());
      const secret = { [Symbol.dispose]: vi.fn() };
      const permission = { [Symbol.dispose]: vi.fn() };
      const streamReturn = vi.fn(async () => ({ done: true as const, value: undefined }));
      const stream = {
        reader: {
          [Symbol.asyncIterator]: () => ({
            next: async () => ({ done: true as const, value: undefined }),
            return: streamReturn,
          }),
        },
        [Symbol.dispose]: vi.fn(),
      };
      const payload = {
        graph: schemaGraphToWit({
          defs: new Map(),
          root: t.record([
            field('secret', t.secret(t.string())),
            field('permission', t.permissionCard({ polymorphic: false })),
            field('stream', t.stream()),
          ]),
        }),
        value: {
          root: 0,
          valueNodes: [
            { tag: 'record-value', val: [1, 2, 3] },
            { tag: 'secret-value', val: secret },
            { tag: 'permission-card-handle', val: permission },
            { tag: 'stream-value', val: stream },
          ],
        },
      } as unknown as TypedSchemaValue;
      {
        await flush();
        const rejected =
          phase === 'during-get' || phase === 'custom-error'
            ? expect(invocation.result).rejects.toThrow('observer is detached')
            : undefined;
        if (phase === 'after-terminal') {
          terminal.resolve({ result: payload });
          // The mapped terminal exists, but the caller has not claimed it.
          await flush();
        }
        invocation.detachResult();
        invocation.cancel();
        if (phase !== 'after-terminal') {
          expect(future.cancel).toHaveBeenCalledOnce();
          expect(future[Symbol.dispose]).not.toHaveBeenCalled();
          if (phase === 'custom-error')
            terminal.reject({
              tag: 'remote-tool-error',
              val: { tag: 'custom-error', val: { name: 'denied', payload } },
            });
          else terminal.resolve({ result: payload });
        }
        await rejected;
        await flush();
        expect(secret[Symbol.dispose]).toHaveBeenCalledOnce();
        expect(permission[Symbol.dispose]).toHaveBeenCalledOnce();
        expect(streamReturn).toHaveBeenCalledOnce();
        expect(stream[Symbol.dispose]).toHaveBeenCalledOnce();
        const calls = future.cancel.mock.calls.length;
        invocation.cancel();
        expect(future.cancel.mock.calls.length).toBe(calls);
      }
      expect(future[Symbol.dispose]).toHaveBeenCalledOnce();
      await expect(invocation.result).rejects.toThrow('observer is detached');
    },
  );

  it('keeps deferred admission rejection settled before observation and lets detach suppress delivery', async () => {
    const failure = new Error('admission failed');
    const unhandled: unknown[] = [];
    const record = (reason: unknown) => unhandled.push(reason);
    process.on('unhandledRejection', record);
    try {
      const invocation = deferredStartedToolInvocation<string>(
        Promise.reject(failure),
        false,
        false,
      );
      await new Promise((resolve) => setTimeout(resolve, 0));
      await expect(invocation.result).rejects.toBe(failure);
      expect(unhandled).toEqual([]);

      const detached = deferredStartedToolInvocation<string>(Promise.reject(failure), false, false);
      await flush();
      const result = detached.result;
      detached.detachResult();
      await expect(result).rejects.toThrow('observer is detached');
    } finally {
      process.off('unhandledRejection', record);
    }
  });

  it('forwards detach across deferred admission without cancelling the admitted invocation', async () => {
    const admission = gate<ReturnType<typeof startedToolInvocation<string>>>();
    const future = { get: vi.fn(async () => ({})), cancel: vi.fn(), [Symbol.dispose]: vi.fn() };
    vi.mocked(ToolRpc).mockImplementationOnce(
      () => ({ asyncInvokeAndAwait: () => future }) as unknown as ToolRpc,
    );
    const deferred = deferredStartedToolInvocation(admission.promise, false, false);
    const pending = expect(deferred.result).rejects.toThrow('observer is detached');
    deferred.detachResult();
    await pending;
    const raw = createToolClientTransport('deferred').start(
      [],
      emptyInput(),
      undefined,
      false,
      false,
    );
    admission.resolve(
      startedToolInvocation(
        undefined,
        undefined,
        mapSettledToolResult(raw.settledResult, () => 'done'),
        () => raw.cancel(),
      ),
    );
    await flush();
    await expect(deferred.result).rejects.toThrow('observer is detached');
    expect(future.cancel).not.toHaveBeenCalled();
    expect(future[Symbol.dispose]).toHaveBeenCalledOnce();
  });

  it('relinquishes an untransferred native stream capability', async () => {
    const returned = vi.fn(async () => ({ done: true as const, value: undefined }));
    const handle = new GuestSchemaValueStreamHandle({
      kind: 'native',
      value: {
        [Symbol.asyncIterator]: () => ({
          next: async () => ({ done: true as const, value: undefined }),
          return: returned,
        }),
      },
    });
    relinquishSchemaValueCapabilities({ tag: 'record', fields: [{ tag: 'stream', handle }] });
    await flush();
    expect(returned).toHaveBeenCalledOnce();
    expect(handle.peek()).toBeUndefined();
  });

  it('produces the independent rich-tool provider lifecycle tuple', async () => {
    let stdinClosed = 0;
    const stdinBytes: number[] = [];
    const stdin = {
      [Symbol.asyncIterator]() {
        const chunks = [Uint8Array.of(0, 97, 115, 109)];
        return {
          next: async () =>
            chunks.length
              ? { done: false as const, value: { tag: 'ok' as const, val: chunks.shift()! } }
              : { done: true as const, value: undefined },
          return: async () => {
            stdinClosed += 1;
            return { done: true as const, value: undefined };
          },
        };
      },
    };
    const definition = toolDefinition('gol40-terminal-provider-acceptance').body((body) =>
      body
        .stdin({ required: true })
        .stdout({ required: true })
        .stderr({ required: true })
        .returns(ArtifactReport),
    );
    definition.implement({
      'gol40-terminal-provider-acceptance': async (_, context) => {
        const stdinReader = context.stdin.getReader();
        while (true) {
          const chunk = await stdinReader.read();
          if (chunk.done) break;
          stdinBytes.push(...chunk.value);
        }
        await context.stdout.getWriter().write(new TextEncoder().encode('compiled 2 inputs\n'));
        await context.stderr.getWriter().write(Uint8Array.of(0, 1, 2));
        return ok({
          artifactId: 18446744073709551614n,
          digest: 'deadbeef',
          labels: new Map([
            ['tier', 'gold'],
            ['team', 'runtime'],
          ]),
          warnings: ['unsigned metadata'],
        });
      },
    });
    const registered = ToolRegistry.get('gol40-terminal-provider-acceptance');
    const node = registered?.extended.commandByPath([]);
    if (!registered || !node) throw new Error('terminal acceptance tool was not registered');
    const input = typedSchemaValueToWit(
      registered.extended.canonicalInputModel(node).encodeTyped({}),
    );
    const stdout = outputWriter();
    const stderr = outputWriter();

    let ownerOutcome = 'pending';
    const terminal = await tool
      .invoke('gol40-terminal-provider-acceptance', [], input, stdin, stdout, stderr, {
        tag: 'anonymous',
      })
      .then((result) => {
        ownerOutcome = 'completed';
        return result;
      });
    if (terminal.result === undefined) throw new Error('provider omitted its declared result');
    const report = registered.extended.root.body!.result!.codec.fromValue(
      typedSchemaValueFromWit(terminal.result).value,
    );

    expect({
      stdin: { bytes: stdinBytes, terminal: 'ended', cleanup: stdinClosed },
      stdout: {
        bytes: stdout.chunks,
        terminal: stdout.finish.mock.calls.length ? 'ended' : 'open',
      },
      stderr: {
        bytes: stderr.chunks,
        terminal: stderr.finish.mock.calls.length ? 'ended' : 'open',
      },
      structured: { kind: 'success', value: report },
      ownerOutcome,
    }).toEqual({
      stdin: { bytes: [0, 97, 115, 109], terminal: 'ended', cleanup: 1 },
      stdout: {
        bytes: [...new TextEncoder().encode('compiled 2 inputs\n')],
        terminal: 'ended',
      },
      stderr: { bytes: [0, 1, 2], terminal: 'ended' },
      structured: {
        kind: 'success',
        value: {
          artifactId: 18446744073709551614n,
          digest: 'deadbeef',
          labels: new Map([
            ['tier', 'gold'],
            ['team', 'runtime'],
          ]),
          warnings: ['unsigned metadata'],
        },
      },
      ownerOutcome: 'completed',
    });
    expect([stdout.fail.mock.calls.length, stderr.fail.mock.calls.length]).toEqual([0, 0]);
  });

  it('keeps observer drop, reader cancellation, and invocation cancellation distinct', async () => {
    const lifecycle = () => {
      let sourceClosed = 0;
      let invocationCancelled = 0;
      const source = {
        [Symbol.asyncIterator]() {
          return {
            next: async () => ({
              done: false as const,
              value: { tag: 'ok' as const, val: Uint8Array.of(1) },
            }),
            return: async () => {
              sourceClosed += 1;
              return { done: true as const, value: undefined };
            },
          };
        },
      };
      const invocation = startedToolInvocation(
        source,
        undefined,
        Promise.resolve({ status: 'fulfilled', value: 'done' }),
        () => {
          invocationCancelled += 1;
        },
      );
      return {
        invocation,
        observation: () => ({ sourceClosed, invocationCancelled }),
      };
    };

    const observerDropObservation = (() => lifecycle().observation)();
    await Promise.resolve();

    const readerCancellation = lifecycle();
    const reader = readerCancellation.invocation.stdout!.getReader();
    await reader.read();
    await reader.cancel();

    const invocationCancellation = lifecycle();
    invocationCancellation.invocation.cancel();

    expect({
      observerDrop: observerDropObservation(),
      readerCancellation: readerCancellation.observation(),
      invocationCancellation: invocationCancellation.observation(),
    }).toEqual({
      observerDrop: { sourceClosed: 0, invocationCancelled: 0 },
      readerCancellation: { sourceClosed: 1, invocationCancelled: 0 },
      invocationCancellation: { sourceClosed: 0, invocationCancelled: 1 },
    });
  });
});
