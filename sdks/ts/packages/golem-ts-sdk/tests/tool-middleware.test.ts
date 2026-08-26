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

import type {
  InvocationResult,
  Tool,
  ToolError,
  TypedSchemaValue,
  UnderlyingTool,
} from 'golem:tool/common@0.1.0';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod/v4';
import { compileSchema } from '../src/schema/adapter';
import { sdkPrincipalFromHost } from '../src/principal';
import {
  command,
  decodeDeclaredToolError,
  err,
  getExtendedToolDefinition,
  ToolInvokeError,
  toolDefinition,
} from '../src/tool';
import { ToolMiddlewareRegistry } from '../src/internal/registry/toolMiddlewareRegistry';
import {
  invokeMonomorphicToolMiddleware,
  type MonomorphicToolMiddlewareInvocation,
} from '../src/internal/tool/middlewareRuntime';
import { typedSchemaValueFromWit, typedSchemaValueToWit, v } from '../src/internal/schema-model';

type RawUnderlyingTool = Pick<UnderlyingTool, 'invoke'>;

beforeEach(() => {
  ToolMiddlewareRegistry.clearForTests();
});

function wireValue(schema: Parameters<typeof compileSchema>[0], value: unknown): TypedSchemaValue {
  const codec = compileSchema(schema);
  return typedSchemaValueToWit({ graph: codec.graph, value: codec.toValue(value) });
}

function commandInput(
  definition: Parameters<typeof getExtendedToolDefinition>[0],
  commandPath: readonly string[],
  value: Record<string, unknown>,
): TypedSchemaValue {
  const tool = getExtendedToolDefinition(definition);
  const command = tool.commandByPath(commandPath);
  if (!command) throw new Error(`missing test command ${commandPath.join(' ')}`);
  return typedSchemaValueToWit(tool.canonicalInputModel(command).encodeTyped(value));
}

function decodeValue(
  schema: Parameters<typeof compileSchema>[0],
  value: TypedSchemaValue,
): unknown {
  const codec = compileSchema(schema);
  return codec.fromValue(typedSchemaValueFromWit(value).value);
}

function monomorphicSource(name: string) {
  const source = ToolMiddlewareRegistry.getSource(name);
  if (!source || source.kind !== 'monomorphic') {
    throw new Error(`missing monomorphic middleware ${name}`);
  }
  return source;
}

function invoke(
  middlewareName: string,
  options: Omit<MonomorphicToolMiddlewareInvocation, 'toolName' | 'toolMetadata'>,
  raw: RawUnderlyingTool,
): Promise<InvocationResult> {
  return invokeMonomorphicToolMiddleware(
    monomorphicSource(middlewareName),
    {
      toolName: 'runtime-name-is-not-a-codec-input',
      toolMetadata: { intentionally: 'malformed and ignored' } as unknown as Tool,
      ...options,
    },
    raw,
  );
}

function rejectionOf(promise: Promise<unknown>): Promise<unknown> {
  return promise.then(
    () => Symbol('resolved'),
    (error) => error,
  );
}

function controllableStream(...values: number[]): {
  stream: AsyncIterable<number>;
  close: ReturnType<typeof vi.fn>;
} {
  let index = 0;
  const close = vi.fn(async () => ({ done: true, value: undefined }) as IteratorResult<number>);
  const iterator: AsyncIterator<number> = {
    next: async () =>
      index < values.length
        ? { done: false, value: values[index++] }
        : { done: true, value: undefined },
    return: close,
  };
  return { stream: { [Symbol.asyncIterator]: () => iterator }, close };
}

const anonymous = sdkPrincipalFromHost({ tag: 'anonymous' });

describe('monomorphic tool middleware registration', () => {
  it('normalizes metadata and compiles every presented leaf, including grafted descendants', () => {
    const graft = toolDefinition('remote')
      .body((body) => body.returns(z.string()))
      .command('leaf', (leaf) => leaf.body((body) => body.returns(z.string())));
    const presented = toolDefinition('complete')
      .body((body) => body.returns(z.string()))
      .command('branch', (branch) =>
        branch
          .body((body) => body.returns(z.string()))
          .command('leaf', (leaf) => leaf.body((body) => body.returns(z.string()))),
      )
      .command('remote', graft);

    expect(
      presented.middleware({
        name: 'complete-policy',
        aliases: ['complete-alias'],
        doc: {
          summary: 'Complete policy',
          description: 'Covers every presented leaf.',
          examples: [{ title: 'Example', body: 'complete' }],
        },
        implementation: {
          complete: async () => 'root',
          branch: command(async () => 'branch', { leaf: async () => 'branch-leaf' }),
          remote: command(async () => 'remote', { leaf: async () => 'remote-leaf' }),
        },
      }),
    ).toEqual({ name: 'complete-policy' });

    const source = monomorphicSource('complete-policy');
    expect(source.aliases).toEqual(['complete-alias']);
    expect(source.doc).toEqual({
      summary: 'Complete policy',
      description: 'Covers every presented leaf.',
      examples: [{ title: 'Example', body: 'complete' }],
    });
    expect(source.expected).toBe(source.presented);
    expect(source.runtime.subtreeForwards).toEqual([]);
    expect(source.runtime.bindings.map(({ commandPath }) => commandPath)).toEqual([
      [],
      ['branch'],
      ['branch', 'leaf'],
      ['remote'],
      ['remote', 'leaf'],
    ]);
  });

  it.each([
    {
      name: 'Invalid',
      aliases: undefined,
      doc: undefined,
      message: 'invalid tool middleware name',
    },
    {
      name: 'valid',
      aliases: ['valid'],
      doc: undefined,
      message: 'duplicate tool middleware name or alias',
    },
    {
      name: 'valid',
      aliases: ['Invalid'],
      doc: undefined,
      message: 'invalid tool middleware alias',
    },
    {
      name: 'valid',
      aliases: undefined,
      doc: { summary: 42 },
      message: 'doc summary must be a string',
    },
  ])('defers invalid metadata without inserting $name', ({ name, aliases, doc, message }) => {
    const definition = toolDefinition('presented').body((body) => body.returns(z.void()));
    definition.middleware({
      name,
      aliases,
      doc: doc as never,
      implementation: { presented: async () => undefined },
    });

    expect(ToolMiddlewareRegistry.getSource(name)).toBeUndefined();
    expect(ToolMiddlewareRegistry.getRegistrationErrors()).toEqual([
      { name, messages: [expect.stringContaining(message)] },
    ]);
  });

  it('rejects an incomplete implementation atomically', () => {
    const definition = toolDefinition('presented').command('leaf', (leaf) =>
      leaf.body((body) => body.returns(z.void())),
    );
    definition.middleware({
      name: 'missing-handler',
      implementation: {} as never,
    });

    expect(ToolMiddlewareRegistry.getSource('missing-handler')).toBeUndefined();
    expect(ToolMiddlewareRegistry.getRegistrationErrors()).toEqual([
      {
        name: 'missing-handler',
        messages: [expect.stringContaining('missing implementation for tool command')],
      },
    ]);
  });

  it('rejects a supplied non-definition wraps value instead of treating it as omitted', () => {
    const definition = toolDefinition('presented').body((body) => body.returns(z.void()));
    definition.middleware({
      name: 'invalid-wraps',
      wraps: null as never,
      implementation: { presented: async () => undefined },
    });

    expect(ToolMiddlewareRegistry.getSource('invalid-wraps')).toBeUndefined();
    expect(ToolMiddlewareRegistry.getRegistrationErrors()).toEqual([
      {
        name: 'invalid-wraps',
        messages: [expect.stringContaining('wraps must be a tool definition')],
      },
    ]);
  });
});

describe('monomorphic tool middleware dispatch', () => {
  it('rejects, short-circuits, forwards, retries, and transforms transparently', async () => {
    const definition = toolDefinition('echo').body((body) =>
      body.positional('value', z.string()).returns(z.string()),
    );
    definition.middleware({
      name: 'echo-policy',
      implementation: {
        echo: async ({ value }, { underlying }) => {
          if (value === 'reject') {
            throw new ToolInvokeError({
              tag: 'constraint-violation',
              val: 'rejected by middleware',
            });
          }
          if (value === 'short') return 'short-circuited';
          if (value === 'retry') {
            try {
              return await underlying.echo({ value });
            } catch {
              return underlying.echo({ value });
            }
          }
          const result = await underlying.echo({ value });
          return value === 'transform' ? `outer(${result})` : result;
        },
      },
    });

    let calls = 0;
    const raw = {
      invoke: vi.fn(async (path, input): Promise<InvocationResult> => {
        expect(path).toEqual([]);
        const decoded = typedSchemaValueFromWit(input);
        const command = getExtendedToolDefinition(definition).commandByPath([])!;
        const fields = getExtendedToolDefinition(definition)
          .canonicalInputModel(command)
          .decode(decoded.value);
        calls++;
        if (fields.value === 'retry' && calls === 1) {
          throw { tag: 'constraint-violation', val: 'transient' } satisfies ToolError;
        }
        return { result: wireValue(z.string(), `inner:${fields.value}`) };
      }),
    } as RawUnderlyingTool;
    const run = (value: string) =>
      invoke(
        'echo-policy',
        {
          commandPath: [],
          input: commandInput(definition, [], { value }),
          stdin: undefined,
          principal: anonymous,
        },
        raw,
      );

    const rejected = (await rejectionOf(run('reject'))) as ToolInvokeError;
    expect(rejected).toMatchObject({
      cause: { tag: 'constraint-violation', val: 'rejected by middleware' },
    });
    expect(calls).toBe(0);

    await expect(
      run('short').then((result) => decodeValue(z.string(), result.result!)),
    ).resolves.toBe('short-circuited');
    expect(calls).toBe(0);

    await expect(
      run('forward').then((result) => decodeValue(z.string(), result.result!)),
    ).resolves.toBe('inner:forward');
    expect(calls).toBe(1);

    calls = 0;
    await expect(
      run('retry').then((result) => decodeValue(z.string(), result.result!)),
    ).resolves.toBe('inner:retry');
    expect(calls).toBe(2);

    calls = 0;
    await expect(
      run('transform').then((result) => decodeValue(z.string(), result.result!)),
    ).resolves.toBe('outer(inner:transform)');
    expect(calls).toBe(1);
  });

  it('adapts input, results, and only the typed custom-error branch', async () => {
    const unsignedNumber = z.number().int().nonnegative();
    const presented = toolDefinition('convert').body((body) =>
      body
        .positional('value', unsignedNumber)
        .returns(z.string())
        .error('rejected', { kind: 'runtime', exitCode: 1, payload: z.string() }),
    );
    const expected = toolDefinition('backend').body((body) =>
      body
        .positional('encoded', z.string())
        .returns(unsignedNumber)
        .error('failed', { kind: 'runtime', exitCode: 1, payload: z.string() }),
    );
    presented.middleware({
      name: 'adapter-policy',
      wraps: expected,
      implementation: {
        convert: async ({ value }, { underlying }) => {
          try {
            return `public:${await underlying.backend({ encoded: `backend:${value}` })}`;
          } catch (error) {
            if (!(error instanceof ToolInvokeError)) throw error;
            throw error.mapTool((backendError) => err('rejected', backendError.payload));
          }
        },
      },
    });

    let failure: ToolError | undefined;
    const raw = {
      invoke: vi.fn(async (path, input): Promise<InvocationResult> => {
        expect(path).toEqual([]);
        const command = getExtendedToolDefinition(expected).commandByPath([])!;
        const values = getExtendedToolDefinition(expected)
          .canonicalInputModel(command)
          .decode(typedSchemaValueFromWit(input).value);
        expect(values).toEqual({ encoded: 'backend:42' });
        if (failure) throw failure;
        return { result: wireValue(unsignedNumber, 7) };
      }),
    } as RawUnderlyingTool;
    const run = () =>
      invoke(
        'adapter-policy',
        {
          commandPath: [],
          input: commandInput(presented, [], { value: 42 }),
          stdin: undefined,
          principal: anonymous,
        },
        raw,
      );

    await expect(run().then((result) => decodeValue(z.string(), result.result!))).resolves.toBe(
      'public:7',
    );
    expect(monomorphicSource('adapter-policy').presented.toolName).toBe('convert');
    expect(monomorphicSource('adapter-policy').expected.toolName).toBe('backend');

    failure = { tag: 'custom-error', val: wireValue(z.string(), 'denied') };
    const custom = (await rejectionOf(run())) as ToolInvokeError<TypedSchemaValue>;
    expect(custom.cause.tag).toBe('tool');
    if (custom.cause.tag !== 'tool') throw new Error('custom error was not encoded');
    const presentedBody = getExtendedToolDefinition(presented).root.body!;
    expect(decodeDeclaredToolError(presentedBody, custom.cause.error, 'convert')).toEqual(
      err('rejected', 'denied'),
    );

    failure = { tag: 'invalid-tool-name', val: 'missing-backend' };
    await expect(run()).rejects.toMatchObject({
      cause: { tag: 'invalid-tool-name', val: 'missing-backend' },
    });
  });

  it('resolves aliases, inherited globals, body-plus-children, and grafts without tool registration', async () => {
    const graft = toolDefinition('remote')
      .aliases('r')
      .global('profile', z.string(), { aliases: ['config'], required: true })
      .command('leaf', (leaf) =>
        leaf.aliases('l').body((body) => body.positional('name', z.string()).returns(z.string())),
      );
    const presented = toolDefinition('nested')
      .global('config', z.string(), { required: true })
      .command('branch', (branch) =>
        branch
          .aliases('b')
          .body((body) => body.returns(z.string()))
          .command('leaf', (leaf) =>
            leaf
              .aliases('l')
              .body((body) => body.positional('name', z.string()).returns(z.string())),
          ),
      )
      .command('remote', graft);
    const observations: unknown[] = [];
    presented.middleware({
      name: 'nested-policy',
      implementation: {
        branch: command(async ({ config }) => `branch:${config}`, {
          leaf: async (args) => {
            observations.push(args);
            return `branch-leaf:${args.config}:${args.name}`;
          },
        }),
        remote: command({
          leaf: async (args) => {
            observations.push(args);
            return `remote-leaf:${args.config}:${args.name}`;
          },
        }),
      },
    });
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;
    const run = (path: readonly string[], input: Record<string, unknown>) =>
      invoke(
        'nested-policy',
        {
          commandPath: path,
          input: commandInput(presented, path, input),
          stdin: undefined,
          principal: anonymous,
        },
        raw,
      ).then((result) => decodeValue(z.string(), result.result!));

    await expect(run(['b'], { config: 'cfg' })).resolves.toBe('branch:cfg');
    await expect(run(['b', 'l'], { config: 'cfg', name: 'one' })).resolves.toBe(
      'branch-leaf:cfg:one',
    );
    await expect(run(['r', 'l'], { config: 'cfg', name: 'two' })).resolves.toBe(
      'remote-leaf:cfg:two',
    );
    expect(observations).toEqual([
      { config: 'cfg', name: 'one' },
      { config: 'cfg', name: 'two' },
    ]);
    expect(raw.invoke).not.toHaveBeenCalled();
  });

  it('maps malformed outer input and middleware results to the correct protocol errors', async () => {
    const definition = toolDefinition('checked').body((body) =>
      body
        .positional('value', z.string())
        .returns(z.string())
        .error('failed', { kind: 'runtime', exitCode: 1, payload: z.string() }),
    );
    let mode: 'wrong-result' | 'wrong-error' = 'wrong-result';
    let called = false;
    definition.middleware({
      name: 'checked-policy',
      implementation: {
        checked: async () => {
          called = true;
          if (mode === 'wrong-error') {
            throw ToolInvokeError.tool(err('undeclared', 42));
          }
          return 42 as never;
        },
      },
    });
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;
    const invokeWithInput = (input: TypedSchemaValue) =>
      invoke(
        'checked-policy',
        { commandPath: [], input, stdin: undefined, principal: anonymous },
        raw,
      );

    await expect(invokeWithInput(wireValue(z.boolean(), true))).rejects.toMatchObject({
      cause: { tag: 'invalid-input' },
    });
    expect(called).toBe(false);

    const validInput = typedSchemaValueFromWit(commandInput(definition, [], { value: 'valid' }));
    for (const malformedValue of [
      v.record([]),
      v.record([v.string('valid'), v.string('extra')]),
      v.record([v.bool(true)]),
    ]) {
      await expect(
        invokeWithInput(typedSchemaValueToWit({ graph: validInput.graph, value: malformedValue })),
      ).rejects.toMatchObject({ cause: { tag: 'invalid-input' } });
    }
    expect(called).toBe(false);

    await expect(
      invokeWithInput(commandInput(definition, [], { value: 'valid' })),
    ).rejects.toMatchObject({ cause: { tag: 'invalid-result' } });

    mode = 'wrong-error';
    await expect(
      invokeWithInput(commandInput(definition, [], { value: 'valid' })),
    ).rejects.toMatchObject({
      cause: { tag: 'invalid-result', val: expect.stringContaining('undeclared error') },
    });

    await expect(
      invoke(
        'checked-policy',
        {
          commandPath: ['missing'],
          input: commandInput(definition, [], { value: 'valid' }),
          stdin: undefined,
          principal: anonymous,
        },
        raw,
      ),
    ).rejects.toMatchObject({
      cause: { tag: 'invalid-command-path', val: ['missing'] },
    });
  });

  it('projects required stdin and structured stdout through the caller-side contract', async () => {
    const definition = toolDefinition('streaming').body((body) =>
      body.stdin({ required: true }).stdout({ required: true }).returns(z.string()),
    );
    definition.middleware({
      name: 'stream-policy',
      implementation: {
        streaming: async (_args, { underlying, stdin }) => underlying.streaming({ stdin }),
      },
    });
    const stdin = controllableStream(1, 2, 3);
    const stdout = controllableStream(4, 5, 6);
    const raw = {
      invoke: vi.fn(async (_path, _input, receivedStdin) => {
        expect(receivedStdin).toBe(stdin.stream);
        return { result: wireValue(z.string(), 'streamed'), stdout: stdout.stream };
      }),
    } as RawUnderlyingTool;

    const result = await invoke(
      'stream-policy',
      {
        commandPath: [],
        input: commandInput(definition, [], {}),
        stdin: stdin.stream,
        principal: anonymous,
      },
      raw,
    );

    expect(decodeValue(z.string(), result.result!)).toBe('streamed');
    expect(result.stdout).toBeDefined();
    expect(stdin.close).not.toHaveBeenCalled();
    expect(stdout.close).not.toHaveBeenCalled();
    await result.stdout?.[Symbol.asyncIterator]().return?.();
    expect(stdout.close).toHaveBeenCalledOnce();
  });

  it('enforces absent/required/optional stream slots and closes rejected streams', async () => {
    const absent = toolDefinition('absent').body((body) => body.returns(z.void()));
    let absentResult: unknown;
    absent.middleware({
      name: 'absent-policy',
      implementation: { absent: async () => absentResult as never },
    });
    const required = toolDefinition('required').body((body) =>
      body.stdin({ required: true }).returns(z.void()),
    );
    required.middleware({
      name: 'required-policy',
      implementation: { required: async () => undefined },
    });
    const optional = toolDefinition('optional').body((body) =>
      body.stdout({ required: false }).returns(z.void()),
    );
    let optionalResult: unknown;
    optional.middleware({
      name: 'optional-policy',
      implementation: { optional: async () => optionalResult as never },
    });
    const raw = { invoke: vi.fn(async () => ({})) } as RawUnderlyingTool;

    const unexpected = controllableStream(1);
    await expect(
      invoke(
        'absent-policy',
        {
          commandPath: [],
          input: commandInput(absent, [], {}),
          stdin: unexpected.stream,
          principal: anonymous,
        },
        raw,
      ),
    ).rejects.toMatchObject({ cause: { tag: 'invalid-input' } });
    expect(unexpected.close).toHaveBeenCalledOnce();

    const undeclaredStdout = controllableStream(2);
    absentResult = undeclaredStdout.stream;
    await expect(
      invoke(
        'absent-policy',
        {
          commandPath: [],
          input: commandInput(absent, [], {}),
          stdin: undefined,
          principal: anonymous,
        },
        raw,
      ),
    ).rejects.toMatchObject({ cause: { tag: 'invalid-result' } });
    expect(undeclaredStdout.close).toHaveBeenCalledOnce();

    await expect(
      invoke(
        'required-policy',
        {
          commandPath: [],
          input: commandInput(required, [], {}),
          stdin: undefined,
          principal: anonymous,
        },
        raw,
      ),
    ).rejects.toMatchObject({ cause: { tag: 'invalid-input' } });

    await expect(
      invoke(
        'optional-policy',
        {
          commandPath: [],
          input: commandInput(optional, [], {}),
          stdin: undefined,
          principal: anonymous,
        },
        raw,
      ),
    ).resolves.toEqual({ result: undefined, stdout: undefined });

    optionalResult = 'not-a-stream';
    await expect(
      invoke(
        'optional-policy',
        {
          commandPath: [],
          input: commandInput(optional, [], {}),
          stdin: undefined,
          principal: anonymous,
        },
        raw,
      ),
    ).rejects.toMatchObject({ cause: { tag: 'invalid-result' } });
  });
});
