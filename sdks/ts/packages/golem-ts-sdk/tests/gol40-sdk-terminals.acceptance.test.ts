// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { KeyValue, ok, s, tool, toolDefinition } from '../src';
import { startedToolInvocation } from '../src/bridge/tool';
import { ToolRegistry } from '../src/internal/registry/toolRegistry';
import { typedSchemaValueFromWit, typedSchemaValueToWit } from '../src/internal/schema-model';

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
  };
}

describe('GOL-40 TypeScript SDK terminal acceptance', () => {
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
