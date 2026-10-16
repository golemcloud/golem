// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { c, toolDefinition, getExtendedToolDefinition } from '../src/tool';
import { encodeTool } from '../src/internal/tool';
import { compileSchema } from '../src/schema/adapter';
import { s } from '../src/schema/markers';
import { ToolCallError } from '../src/toolClient';
import { ToolRemoteOutputError, ToolType } from '../src/toolReflection';
import { createToolClientRuntime, type ToolClientRuntime } from '../src/bridge/tool';
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
    const source = new Uint8Array([1, 255]);
    const definition = toolDefinition('binary-snapshot').body((body) =>
      body.option('payload', s.binary(), { default: source }),
    );
    const wire = encodeTool(getExtendedToolDefinition(definition));
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

  it('invokes with one authored option carrier for omitted and supplied inputs', async () => {
    const definition = toolDefinition('single-option').body((body) =>
      body
        .positional('position', z.string().optional(), { required: false })
        .option('choice', z.string().optional()),
    );
    const authored = getExtendedToolDefinition(definition);
    const start = vi.fn((_path, _input) => ({
      settledResult: Promise.resolve({
        status: 'fulfilled' as const,
        value: { result: undefined },
      }),
      cancel: vi.fn(),
    }));
    const command = new ToolType(
      {
        lookupName: 'single-option',
        definition: encodeTool(authored),
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      { start } as ToolClientRuntime,
    ).client.command([]);
    for (const argument of command.arguments) {
      expect(argument.schema.root.body.tag).toBe('option');
      if (argument.schema.root.body.tag === 'option') {
        expect(argument.schema.root.body.element.body.tag).not.toBe('option');
      }
    }
    const expectedGraph = authored.canonicalInputModel(authored.root).codec.graph;
    for (const [position, choice] of [
      [null, null],
      ['p', null],
      [null, 'c'],
      ['p', 'c'],
    ] as const) {
      expect(command.validateJson({ position, choice }).success).toBe(true);
      await expect(command.invokeJson({ position, choice })).resolves.toBeUndefined();
      const sent = start.mock.calls.at(-1)![1];
      expect(schemaShapesMatch(expectedGraph, sent.graph)).toBe(true);
      expect(command.inputSchema!.validateValue(sent.value).success).toBe(true);
    }
    expect(start).toHaveBeenCalledTimes(4);
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

  it('builds a canonical input schema and accepts omitted options before dispatch', async () => {
    const { tool, start } = fixture();
    const command = tool.client.command([]);
    expect(command.inputSchema?.toJsonSchema()).toBeDefined();
    expect(command.validateJson({ value: 'hello', maybe: null }).success).toBe(true);
    expect(command.validateJson({ value: 'hello' }).success).toBe(true);
    expect(start).not.toHaveBeenCalled();
    for (const input of [
      { value: 'hello' },
      { value: 'hello', maybe: null },
      { value: 'hello', maybe: 'supplied' },
    ]) {
      await expect(command.invokeJson(input)).resolves.toBe('ok');
      const sent = start.mock.calls.at(-1)![1];
      expect(schemaShapesMatch(command.inputSchema!.graph, sent.graph)).toBe(true);
      expect(command.inputSchema!.validateValue(sent.value).success).toBe(true);
    }
    expect(start).toHaveBeenCalledTimes(3);
  });

  it('fills omitted declared defaults when packing canonical JSON', async () => {
    const definition = toolDefinition('bash-like').body((body) =>
      body
        .positional('script', z.string())
        .positional('mode', z.string(), { default: 'run' })
        .option('cwd', z.string(), { default: '' })
        .option('timeout', s.u32(), { default: 600 })
        .option('shell', z.string())
        .option('env', z.string(), { repeatable: 'repeated', default: ['A=1'] })
        .flag('login')
        .flag('verbose', { kind: 'count-flag' })
        .returns(z.string()),
    );
    const output = compileSchema(z.string());
    const start = vi.fn((_path, _input) => ({
      settledResult: Promise.resolve({
        status: 'fulfilled' as const,
        value: { result: { graph: output.graph, value: output.toValue('ok') } },
      }),
      cancel: vi.fn(),
    }));
    const command = new ToolType(
      {
        lookupName: 'bash-like',
        definition: encodeTool(getExtendedToolDefinition(definition)),
        implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
      },
      { start } as unknown as ToolClientRuntime,
    ).client.command([]);
    const names = command.arguments.map((argument) => argument.name);
    const fields = (value: ReturnType<typeof command.packJson>) => {
      if (value.tag !== 'record') throw new Error('expected a record');
      return Object.fromEntries(names.map((name, index) => [name, value.fields[index]]));
    };

    expect(fields(command.packJson({ script: 'echo ok', env: [] }))).toEqual({
      script: v.string('echo ok'),
      mode: v.string('run'),
      cwd: v.string(''),
      timeout: v.u32(600),
      shell: { tag: 'option' },
      env: v.list([]),
      login: v.bool(false),
      verbose: v.u32(0),
    });
    expect(
      fields(
        command.packJson({
          script: 'echo ok',
          mode: 'check',
          cwd: '/tmp',
          timeout: 120000,
          shell: 'zsh',
          env: ['B=2'],
          login: true,
          verbose: 2,
        }),
      ),
    ).toEqual({
      script: v.string('echo ok'),
      mode: v.string('check'),
      cwd: v.string('/tmp'),
      timeout: v.u32(120000),
      shell: { tag: 'option', value: v.string('zsh') },
      env: v.list([v.string('B=2')]),
      login: v.bool(true),
      verbose: v.u32(2),
    });
    expect(command.inputSchema!.packJson({ script: 'echo ok', env: [] })).toEqual(
      command.packJson({ script: 'echo ok', env: [] }),
    );

    expect(command.validateJson({ env: [] })).toEqual({
      success: false,
      issues: [{ path: ['script'], message: "missing field 'script'" }],
    });
    expect(() => command.packJson({ env: [] })).toThrow("missing field 'script'");
    expect(command.validateJson({ script: 'echo ok' })).toEqual({
      success: false,
      issues: [{ path: ['env'], message: "missing field 'env'" }],
    });

    expect(command.inputSchema!.toJsonSchema({ includeDraftMarker: false })).toMatchObject({
      required: ['script', 'env'],
      properties: {
        mode: { type: 'string', default: 'run' },
        cwd: { type: 'string', default: '' },
        timeout: { default: 600 },
        login: { type: 'boolean', default: false },
        verbose: { default: 0 },
      },
    });
    const rendered = command.inputSchema!.toJsonSchema() as {
      properties: Record<string, Record<string, unknown>>;
    };
    expect(rendered.properties.shell).not.toHaveProperty('default');
    expect(rendered.properties.env).not.toHaveProperty('default');

    const root = command.inputSchema!.root;
    if (root.body.tag !== 'record') throw new Error('expected a record input');
    expect(
      Object.fromEntries(root.body.fields.map((field) => [field.name, field.body.body.tag])),
    ).toMatchObject({ mode: 'string', cwd: 'string', timeout: 'u32', login: 'bool' });

    await expect(command.invokeJson({ script: 'echo ok', env: [] })).resolves.toBe('ok');
    const sent = start.mock.calls.at(-1)![1];
    expect(schemaShapesMatch(command.inputSchema!.graph, sent.graph)).toBe(true);
    expect(fields(sent.value).timeout).toEqual(v.u32(600));
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
    const collected = await command.startJson({ value: 'hello' }).collect();
    expect(collected.result.status).toBe('rejected');
    if (collected.result.status === 'rejected') {
      expect(collected.result.reason).toBeInstanceOf(ToolRemoteOutputError);
    }
    expect(collected.stdout).toEqual({ status: 'fulfilled', value: undefined });
    expect(collected.stderr).toEqual({ status: 'fulfilled', value: undefined });
  });
});
