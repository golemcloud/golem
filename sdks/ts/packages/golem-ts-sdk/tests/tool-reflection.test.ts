// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { c, toolDefinition, getExtendedToolDefinition } from '../src/tool';
import { encodeTool } from '../src/internal/tool';
import { compileSchema } from '../src/schema/adapter';
import { ToolRemoteOutputError, ToolType } from '../src/toolReflection';
import type { ToolClientRuntime } from '../src/bridge/tool';
import {
  emptyMetadata,
  field,
  schemaShapesMatch,
  schemaType,
  t,
  v,
} from '../src/internal/schema-model';
import { toCanonicalJsonSchema } from '../src/schema/render';

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
  it('treats a default-true negatable flag as present when set to false', () => {
    const definition = toolDefinition('negatable-reflection').body((body) =>
      body
        .flag('enabled', { default: true, negatable: true })
        .constraint(c.requiresAll([c.present('enabled')])),
    );
    const command = new ToolType(
      {
        lookupName: 'negatable-reflection',
        definition: encodeTool(getExtendedToolDefinition(definition)),
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      { start: vi.fn() } as unknown as ToolClientRuntime,
    ).client.command([]);
    expect(command.validateJson({ enabled: false }).success).toBe(true);
    expect(command.validateJson({ enabled: true }).success).toBe(false);
  });

  it('matches ValueIs through an optional positional carrier', () => {
    const definition = toolDefinition('nested-value-is').body((body) =>
      body
        .positional('maybe', z.string(), { required: false })
        .constraint(c.requiresAll([c.valueIs('maybe', 'needle')])),
    );
    const command = new ToolType(
      {
        lookupName: 'nested-value-is',
        definition: encodeTool(getExtendedToolDefinition(definition)),
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      { start: vi.fn() } as unknown as ToolClientRuntime,
    ).client.command([]);
    expect(command.validateJson({ maybe: 'needle' }).success).toBe(true);
    expect(command.validateJson({ maybe: null }).success).toBe(false);
    expect(command.validateJson({ maybe: 'other' }).success).toBe(false);
  });

  it('renders discriminator conditions without replacing the branch schema', () => {
    const root = schemaType({
      tag: 'union',
      branches: [
        {
          tag: 'named',
          body: t.record([field('kind', t.string()), field('payload', t.string())]),
          discriminator: { tag: 'field-equals', val: { fieldName: 'kind', literal: 'named' } },
          metadata: emptyMetadata(),
        },
        {
          tag: 'pattern',
          body: t.string(),
          discriminator: { tag: 'regex', val: '^[a-z]+$' },
          metadata: emptyMetadata(),
        },
      ],
    });
    expect(toCanonicalJsonSchema({ defs: new Map(), root }, root, false)).toMatchObject({
      oneOf: [
        {
          allOf: [{ required: ['kind', 'payload'] }, { properties: { kind: { const: 'named' } } }],
        },
        { allOf: [{ type: 'string' }, { pattern: '^[a-z]+$' }] },
      ],
    });
  });

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
    await expect(command.startJson({ value: 'hello' }).collect()).rejects.toBeInstanceOf(
      ToolRemoteOutputError,
    );
  });
});
