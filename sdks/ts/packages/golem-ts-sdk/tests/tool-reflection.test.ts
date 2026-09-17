// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { toolDefinition, getExtendedToolDefinition } from '../src/tool';
import { encodeTool } from '../src/internal/tool';
import { compileSchema } from '../src/schema/adapter';
import { ToolRemoteOutputError, ToolType } from '../src/toolReflection';
import type { ToolClientRuntime } from '../src/bridge/tool';
import { schemaShapesMatch, v } from '../src/internal/schema-model';

function fixture(result: { readonly malformed?: boolean } = {}) {
  const definition = toolDefinition('reflect-demo').body((body) =>
    body.positional('value', z.string()).option('maybe', z.string()).returns(z.string()),
  );
  const output = compileSchema(z.string());
  const start = vi.fn((_path, _input) => ({
    settledResult: Promise.resolve({
      status: 'fulfilled' as const,
      value: {
        result: result.malformed ? undefined : { graph: output.graph, value: output.toValue('ok') },
      },
    }),
    cancel: vi.fn(),
  }));
  const registered = {
    lookupName: 'reflect-demo',
    definition: encodeTool(getExtendedToolDefinition(definition)),
    implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
  };
  const tool = new ToolType(registered, { start } as ToolClientRuntime);
  return { tool, start };
}

describe('native tool reflection', () => {
  it('builds a canonical input schema and rejects invalid JSON before dispatch', async () => {
    const { tool, start } = fixture();
    const command = tool.client.command([]);
    expect(command.inputSchema?.toJsonSchema()).toBeDefined();
    expect(command.validateJson({ value: 'hello', maybe: null }).success).toBe(true);
    expect(command.validateJson({ value: 'hello' }).success).toBe(false);
    expect(start).not.toHaveBeenCalled();
    for (const maybe of [null, 'supplied']) {
      await expect(command.invokeJson({ value: 'hello', maybe })).resolves.toBe('ok');
      const sent = start.mock.calls.at(-1)![1];
      expect(schemaShapesMatch(command.inputSchema!.graph, sent.graph)).toBe(true);
      expect(command.inputSchema!.validateValue(sent.value).success).toBe(true);
    }
    expect(start).toHaveBeenCalledTimes(2);
  });

  it('rejects a missing declared remote result', async () => {
    const { tool } = fixture({ malformed: true });
    await expect(
      tool.client.command([]).invokeJson({ value: 'hello', maybe: null }),
    ).rejects.toBeInstanceOf(ToolRemoteOutputError);
  });

  it('reports a non-canonical declared result as malformed remote output', async () => {
    const definition = toolDefinition('numeric-reflection').body((body) =>
      body.positional('value', z.string()).returns(z.number()),
    );
    const output = compileSchema(z.number());
    const start = vi.fn(() => ({
      settledResult: Promise.resolve({
        status: 'fulfilled' as const,
        value: { result: { graph: output.graph, value: v.f64(Number.NaN) } },
      }),
      cancel: vi.fn(),
    }));
    const tool = new ToolType(
      {
        lookupName: 'numeric-reflection',
        definition: encodeTool(getExtendedToolDefinition(definition)),
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      { start } as ToolClientRuntime,
    );
    const command = tool.client.command([]);
    await expect(command.invokeJson({ value: 'hello' })).rejects.toBeInstanceOf(
      ToolRemoteOutputError,
    );
    await expect(command.startJson({ value: 'hello' }).result).rejects.toBeInstanceOf(
      ToolRemoteOutputError,
    );
  });
});
