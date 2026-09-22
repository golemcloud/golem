// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { c, toolDefinition, getExtendedToolDefinition } from '../src/tool';
import { encodeTool } from '../src/internal/tool';
import { compileSchema } from '../src/schema/adapter';
import { ToolCallError } from '../src/toolClient';
import { ToolRemoteOutputError, ToolType } from '../src/toolReflection';
import { createToolClientRuntime, type ToolClientRuntime } from '../src/bridge/tool';
import {
  emptyMetadata,
  field,
  schemaShapesMatch,
  schemaValueToWit,
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

  it('applies schema restrictions through command-level packing and validation', () => {
    const definition = toolDefinition('restricted-reflection').body((body) =>
      body.positional('count', z.number().min(10)),
    );
    const command = new ToolType(
      {
        lookupName: 'restricted-reflection',
        definition: encodeTool(getExtendedToolDefinition(definition)),
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      { start: vi.fn() } as unknown as ToolClientRuntime,
    ).client.command([]);
    expect(command.validateJson({ count: 9 }).success).toBe(false);
    expect(() => command.packJson({ count: 9 })).toThrow(/invalid tool input/i);
    expect(command.validateJson({ count: 10 }).success).toBe(true);
  });

  it('owns a deeply immutable discovery snapshot', () => {
    const definition = toolDefinition('immutable-reflection').body((body) =>
      body
        .flag('enabled', { default: true, negatable: true })
        .constraint(c.requiresAll([c.present('enabled')])),
    );
    const wire = encodeTool(getExtendedToolDefinition(definition));
    const command = new ToolType(
      {
        lookupName: 'immutable-reflection',
        definition: wire,
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      { start: vi.fn() } as unknown as ToolClientRuntime,
    ).client.command([]);
    const constraints = wire.commands.nodes[0].body!.constraints as unknown as unknown[];
    constraints[0] = { tag: 'requires-all', val: [{ tag: 'present', val: 'missing' }] };
    expect(command.validateJson({ enabled: false }).success).toBe(true);
    expect(Object.isFrozen(command.constraints[0])).toBe(true);
  });

  it('owns binary defaults without exposing their mutable bytes', () => {
    const definition = toolDefinition('binary-snapshot').body((body) =>
      body.option('payload', z.string()),
    );
    const wire = encodeTool(getExtendedToolDefinition(definition));
    const source = new Uint8Array([1, 255]);
    wire.commands.nodes[0].body!.options[0].default_ = schemaValueToWit(v.binary(source));
    const command = new ToolType(
      {
        lookupName: 'binary-snapshot',
        definition: wire,
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      { start: vi.fn() } as unknown as ToolClientRuntime,
    ).client.command([]);
    source[0] = 8;
    const first = command.arguments[0].default;
    expect(first).toMatchObject({ tag: 'binary', bytes: new Uint8Array([1, 255]) });
    if (first?.tag !== 'binary') throw new Error('expected binary default');
    first.bytes[1] = 9;
    expect(command.arguments[0].default).toMatchObject({
      tag: 'binary',
      bytes: new Uint8Array([1, 255]),
    });
  });

  it('does not nest already optional positional and scalar option schemas', () => {
    const definition = toolDefinition('single-option').body((body) =>
      body
        .positional('position', z.string().optional(), { required: false })
        .option('choice', z.string().optional()),
    );
    const command = new ToolType(
      {
        lookupName: 'single-option',
        definition: encodeTool(getExtendedToolDefinition(definition)),
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      { start: vi.fn() } as unknown as ToolClientRuntime,
    ).client.command([]);
    for (const argument of command.arguments) {
      expect(argument.schema.root.body.tag).toBe('option');
      if (argument.schema.root.body.tag === 'option') {
        expect(argument.schema.root.body.element.body.tag).not.toBe('option');
      }
    }
    expect(command.validateJson({ position: null, choice: null }).success).toBe(true);
    expect(command.validateJson({ position: 'p', choice: 'c' }).success).toBe(true);
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

  it('preserves a structured reflected RPC creation error', async () => {
    const definition = toolDefinition('creation-error').body((body) =>
      body.positional('value', z.string()).returns(z.string()),
    );
    const denied = { tag: 'denied', val: 'not granted' } as const;
    const runtime = createToolClientRuntime('creation-error', {
      start() {
        throw denied;
      },
    });
    const tool = new ToolType(
      {
        lookupName: 'creation-error',
        definition: encodeTool(getExtendedToolDefinition(definition)),
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      runtime,
    );

    await expect(tool.client.command([]).invokeJson({ value: 'hello' })).rejects.toEqual(
      new ToolCallError({ tag: 'rpc', error: denied }),
    );
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
