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

import { ToolRpc, type RpcError } from 'golem:tool/host@0.1.0';
import { type as arkType } from 'arktype';
import { describe, expect, it, vi } from 'vitest';
import * as z3 from 'zod3';
import { z } from 'zod/v4';
import {
  client,
  ToolCallError,
  toolDefinition,
  type ToolClientInvocationResult,
  type ToolClientTransport,
} from '../src/fluent/tool';
import { compileSchema } from '../src/fluent/schema/adapter';
import {
  deepEqual,
  t,
  typedSchemaValueFromWit,
  typedSchemaValueToWit,
  v,
} from '../src/internal/schema-model';
import { Bytes, s } from '../src/fluent/schema/markers';

interface RecordedInvocation {
  readonly commandPath: readonly string[];
  readonly input: Parameters<ToolClientTransport['invokeAndAwait']>[1];
  readonly stdin: ReadableStream<Uint8Array> | undefined;
}

class FakeTransport implements ToolClientTransport {
  readonly invocations: RecordedInvocation[] = [];

  constructor(
    private readonly respond: (
      invocation: RecordedInvocation,
    ) => ToolClientInvocationResult | Promise<ToolClientInvocationResult>,
  ) {}

  invokeAndAwait(
    commandPath: readonly string[],
    input: Parameters<ToolClientTransport['invokeAndAwait']>[1],
    stdin: ReadableStream<Uint8Array> | undefined,
  ): ToolClientInvocationResult | Promise<ToolClientInvocationResult> {
    const invocation = { commandPath: [...commandPath], input, stdin };
    this.invocations.push(invocation);
    return this.respond(invocation);
  }
}

function wireValue(schema: Parameters<typeof compileSchema>[0], value: unknown) {
  const codec = compileSchema(schema);
  return typedSchemaValueToWit({ graph: codec.graph, value: codec.toValue(value) });
}

function rejectionOf(promise: Promise<unknown>): Promise<unknown> {
  return promise.then(
    () => Symbol('resolved'),
    (error) => error,
  );
}

describe('fluent tool runtime client', () => {
  it('assembles root bodies, dispatchers, callable intersections, nested paths, and grafted subtrees', async () => {
    const subtree = toolDefinition('remote')
      .global('remote-global', z.string(), { required: true })
      .body((body) => body.positional('value', z.number()).returns(z.boolean()))
      .command('nested', (nested) => nested.body((body) => body.returns(z.string())));
    const definition = toolDefinition('git')
      .global('git-dir', z.string(), { required: true })
      .body((body) => body.returns(z.void()))
      .command('pure', (pure) =>
        pure.command('leaf', (leaf) => leaf.body((body) => body.returns(z.string()))),
      )
      .command('stash', (stash) =>
        stash
          .body((body) => body.returns(z.string()))
          .command('pop', (pop) => pop.body((body) => body.returns(z.string()))),
      )
      .command('remote', subtree);
    const transport = new FakeTransport(({ commandPath }) => {
      const value =
        commandPath.join('/') === 'remote'
          ? wireValue(z.boolean(), true)
          : wireValue(z.string(), commandPath.join('/') || 'root');
      return commandPath.length === 0 ? {} : { result: value };
    });
    const runtime = client(definition, { transport });

    expect(typeof runtime.git).toBe('function');
    expect(typeof runtime.pure).toBe('object');
    expect(typeof runtime.pure.leaf).toBe('function');
    expect(typeof runtime.stash).toBe('function');
    expect(typeof runtime.stash.pop).toBe('function');
    expect(typeof runtime.remote).toBe('function');
    expect(typeof runtime.remote.nested).toBe('function');

    await expect(runtime.git({ gitDir: '.git' })).resolves.toBeUndefined();
    await expect(runtime.pure.leaf({ gitDir: '.git' })).resolves.toBe('pure/leaf');
    await expect(runtime.stash({ gitDir: '.git' })).resolves.toBe('stash');
    await expect(runtime.stash.pop({ gitDir: '.git' })).resolves.toBe('stash/pop');
    await expect(
      runtime.remote({ gitDir: '.git', remoteGlobal: 'origin', value: 42 }),
    ).resolves.toBe(true);
    await expect(runtime.remote.nested({ gitDir: '.git', remoteGlobal: 'origin' })).resolves.toBe(
      'remote/nested',
    );

    expect(transport.invocations.map(({ commandPath }) => commandPath)).toEqual([
      [],
      ['pure', 'leaf'],
      ['stash'],
      ['stash', 'pop'],
      ['remote'],
      ['remote', 'nested'],
    ]);
    const subtreeInput = typedSchemaValueFromWit(transport.invocations[4].input);
    expect(subtreeInput.value).toEqual(v.record([v.string('.git'), v.string('origin'), v.f64(42)]));
  });

  it('maps camel-case fields into canonical codec order, including optional and repeatable inputs', async () => {
    const definition = toolDefinition('search')
      .global('root-global', z.string(), { required: true })
      .body((body) =>
        body
          .option('max-count', z.number(), { optionalScalar: true })
          .option('patterns', z.string(), { repeatable: 'repeated' })
          .flag('dry-run')
          .returns(z.void()),
      );
    const transport = new FakeTransport(() => ({}));
    const runtime = client(definition, { transport });

    await runtime.search({
      rootGlobal: 'global',
      patterns: ['TODO', 'FIXME'],
      dryRun: false,
    });

    const decoded = typedSchemaValueFromWit(transport.invocations[0].input);
    expect(decoded.value).toEqual(
      v.record([
        v.string('global'),
        v.option(undefined),
        v.list([v.string('TODO'), v.string('FIXME')]),
        v.bool(false),
      ]),
    );
  });

  it('orders inherited globals before fixed positionals, tails, options, and flags', async () => {
    const definition = toolDefinition('ordered')
      .global('root-global', z.string(), { required: true })
      .command('group', (group) =>
        group
          .global('group-global', z.number(), { required: true })
          .command('leaf', (leaf) =>
            leaf
              .global('leaf-global', z.boolean(), { kind: 'flag' })
              .body((body) =>
                body
                  .positional('fixed-value', z.string())
                  .tail('remaining-values', z.string())
                  .option('local-option', z.number(), { optionalScalar: true })
                  .flag('local-flag')
                  .returns(z.void()),
              ),
          ),
      );
    const transport = new FakeTransport(() => ({}));

    await client(definition, { transport }).group.leaf({
      rootGlobal: 'root',
      groupGlobal: 2,
      leafGlobal: true,
      fixedValue: 'fixed',
      remainingValues: ['one', 'two'],
      localFlag: false,
    });

    expect(transport.invocations[0].commandPath).toEqual(['group', 'leaf']);
    expect(typedSchemaValueFromWit(transport.invocations[0].input).value).toEqual(
      v.record([
        v.string('root'),
        v.f64(2),
        v.bool(true),
        v.string('fixed'),
        v.list([v.string('one'), v.string('two')]),
        v.option(undefined),
        v.bool(false),
      ]),
    );
  });

  it('treats an omitted optional argument named constructor as absent', async () => {
    const definition = toolDefinition('prototype-name').body((body) =>
      body.option('constructor', z.string(), { optionalScalar: true }).returns(z.void()),
    );
    const transport = new FakeTransport(() => ({}));

    await expect(client(definition, { transport })['prototype-name']({})).resolves.toBeUndefined();
    expect(typedSchemaValueFromWit(transport.invocations[0].input).value).toEqual(
      v.record([v.option(undefined)]),
    );
  });

  it('forwards optional and required stdin through the transport seam', async () => {
    const required = toolDefinition('required').body((body) =>
      body.stdin({ required: true }).returns(z.void()),
    );
    const optional = toolDefinition('optional').body((body) =>
      body.stdin({ required: false }).returns(z.void()),
    );
    const transport = new FakeTransport(() => ({}));
    const requiredClient = client(required, { transport });
    const optionalClient = client(optional, { transport });
    const stdin = new ReadableStream<Uint8Array>();

    await requiredClient.required({ stdin });
    await optionalClient.optional({});

    expect(transport.invocations.map(({ stdin: recorded }) => recorded)).toEqual([
      stdin,
      undefined,
    ]);
    const missing = await rejectionOf(requiredClient.required({} as never));
    expect(missing).toBeInstanceOf(ToolCallError);
    expect(missing).toMatchObject({
      cause: {
        tag: 'rpc',
        error: { tag: 'protocol-error', val: expect.stringContaining('stdin') },
      },
    });
    const invalid = await rejectionOf(requiredClient.required({ stdin: 'invalid' } as never));
    expect(invalid).toMatchObject({
      cause: {
        tag: 'rpc',
        error: { tag: 'protocol-error', val: expect.stringContaining('ReadableStream') },
      },
    });
  });

  it('projects structured, unit, required stdout, and optional stdout results', async () => {
    const stdout = new ReadableStream<Uint8Array>();
    const structured = toolDefinition('structured').body((body) => body.returns(z.string()));
    const unit = toolDefinition('unit').body((body) => body.returns(z.void()));
    const structuredStdout = toolDefinition('structured-stdout').body((body) =>
      body.stdout({ required: true }).returns(z.string()),
    );
    const unitStdout = toolDefinition('unit-stdout').body((body) =>
      body.stdout({ required: true }).returns(z.void()),
    );
    const optionalStdout = toolDefinition('optional-stdout').body((body) =>
      body.stdout({ required: false }).returns(z.string()),
    );

    await expect(
      client(structured, {
        transport: new FakeTransport(() => ({ result: wireValue(z.string(), 'value') })),
      }).structured({}),
    ).resolves.toBe('value');
    await expect(
      client(unit, { transport: new FakeTransport(() => ({})) }).unit({}),
    ).resolves.toBeUndefined();
    await expect(
      client(structuredStdout, {
        transport: new FakeTransport(() => ({
          result: wireValue(z.string(), 'value'),
          stdout,
        })),
      })['structured-stdout']({}),
    ).resolves.toEqual({ result: 'value', stdout });
    await expect(
      client(unitStdout, { transport: new FakeTransport(() => ({ stdout })) })['unit-stdout']({}),
    ).resolves.toBe(stdout);
    await expect(
      client(optionalStdout, {
        transport: new FakeTransport(() => ({ result: wireValue(z.string(), 'value') })),
      })['optional-stdout']({}),
    ).resolves.toEqual({ result: 'value' });
  });

  it('combines stdin with structured results and stdout through the transport seam', async () => {
    const definition = toolDefinition('transform').body((body) =>
      body.stdin({ required: true }).stdout({ required: true }).returns(z.string()),
    );
    const stdin = new ReadableStream<Uint8Array>();
    const stdout = new ReadableStream<Uint8Array>();
    const transport = new FakeTransport(() => ({
      result: wireValue(z.string(), 'transformed'),
      stdout,
    }));

    await expect(client(definition, { transport }).transform({ stdin })).resolves.toEqual({
      result: 'transformed',
      stdout,
    });
    expect(transport.invocations[0].stdin).toBe(stdin);
  });

  it.each([
    {
      name: 'missing structured result',
      definition: toolDefinition('missing-result').body((body) => body.returns(z.string())),
      response: {},
    },
    {
      name: 'unexpected unit result',
      definition: toolDefinition('unexpected-result').body((body) => body.returns(z.void())),
      response: { result: wireValue(z.string(), 'unexpected') },
    },
    {
      name: 'missing required stdout',
      definition: toolDefinition('missing-stdout').body((body) =>
        body.stdout({ required: true }).returns(z.void()),
      ),
      response: {},
    },
    {
      name: 'unexpected stdout',
      definition: toolDefinition('unexpected-stdout').body((body) => body.returns(z.void())),
      response: { stdout: new ReadableStream<Uint8Array>() },
    },
  ])('rejects $name with a stable protocol error', async ({ definition, response }) => {
    const runtime = client(definition, { transport: new FakeTransport(() => response) }) as Record<
      string,
      (args: {}) => Promise<unknown>
    >;
    const failure = await rejectionOf(runtime[definition.name]({}));

    expect(failure).toBeInstanceOf(ToolCallError);
    expect(failure).toMatchObject({ cause: { tag: 'rpc', error: { tag: 'protocol-error' } } });
  });

  it('cancels stdout when another response validation fails', async () => {
    let cancelled = false;
    const stdout = new ReadableStream<Uint8Array>({
      cancel: () => {
        cancelled = true;
      },
    });
    const definition = toolDefinition('invalid-streamed-result').body((body) =>
      body.stdout({ required: true }).returns(z.string()),
    );
    const failure = await rejectionOf(
      client(definition, {
        transport: new FakeTransport(() => ({
          result: wireValue(z.boolean(), true),
          stdout,
        })),
      })['invalid-streamed-result']({}),
    );

    expect(failure).toMatchObject({ cause: { tag: 'rpc', error: { tag: 'protocol-error' } } });
    await vi.waitFor(() => expect(cancelled).toBe(true));
  });

  it.each<RpcError>([
    { tag: 'protocol-error', val: 'protocol' },
    { tag: 'denied', val: 'denied' },
    { tag: 'not-found', val: 'not found' },
    { tag: 'remote-internal-error', val: 'internal' },
    { tag: 'remote-tool-error', val: { tag: 'invalid-input', val: 'bad input' } },
  ])('preserves the $tag RPC failure in ToolCallError.cause', async (rpcError) => {
    const definition = toolDefinition('failure').body((body) => body.returns(z.void()));
    const transport = new FakeTransport(() => {
      throw rpcError;
    });
    const failure = await rejectionOf(client(definition, { transport }).failure({}));

    expect(failure).toBeInstanceOf(ToolCallError);
    expect(failure).toMatchObject({ cause: { tag: 'rpc', error: rpcError } });
  });

  it('classifies a malformed remote-tool-error as a protocol error', async () => {
    const definition = toolDefinition('malformed-rpc').body((body) => body.returns(z.void()));
    const transport = new FakeTransport(() => {
      throw { tag: 'remote-tool-error', val: { tag: 'invalid-input' } };
    });
    const failure = await rejectionOf(client(definition, { transport })['malformed-rpc']({}));

    expect(failure).toBeInstanceOf(ToolCallError);
    expect(failure).toMatchObject({
      cause: { tag: 'rpc', error: { tag: 'protocol-error' } },
    });
  });

  it('decodes declared custom errors with and without payloads', async () => {
    const withPayload = toolDefinition('with-payload').body((body) =>
      body.returns(z.void()).error('failed', {
        kind: 'runtime',
        exitCode: 1,
        payload: z.object({ reason: z.string() }),
      }),
    );
    const payloadless = toolDefinition('payloadless').body((body) =>
      body.returns(z.void()).error('not-found', { kind: 'runtime', exitCode: 1 }),
    );
    const payloadFailure = await rejectionOf(
      client(withPayload, {
        transport: new FakeTransport(() => {
          throw {
            tag: 'remote-tool-error',
            val: {
              tag: 'custom-error',
              val: wireValue(z.object({ reason: z.string() }), { reason: 'nope' }),
            },
          } satisfies RpcError;
        }),
      })['with-payload']({}),
    );
    const payloadlessFailure = await rejectionOf(
      client(payloadless, {
        transport: new FakeTransport(() => {
          throw {
            tag: 'remote-tool-error',
            val: {
              tag: 'custom-error',
              val: typedSchemaValueToWit({
                graph: { defs: new Map(), root: t.tuple([]) },
                value: v.tuple([]),
              }),
            },
          } satisfies RpcError;
        }),
      }).payloadless({}),
    );

    expect(payloadFailure).toMatchObject({
      cause: {
        tag: 'tool',
        error: { tag: 'err', name: 'failed', hasPayload: true, payload: { reason: 'nope' } },
      },
    });
    expect(payloadlessFailure).toMatchObject({
      cause: {
        tag: 'tool',
        error: { tag: 'err', name: 'not-found', hasPayload: false },
      },
    });
  });

  it('decodes same-shaped custom errors as the first declared case', async () => {
    const withPayload = toolDefinition('with-payload').body((body) =>
      body
        .returns(z.void())
        .error('first', { kind: 'runtime', exitCode: 1, payload: z.string() })
        .error('second', { kind: 'runtime', exitCode: 2, payload: z.string() }),
    );
    const payloadless = toolDefinition('payloadless').body((body) =>
      body
        .returns(z.void())
        .error('first', { kind: 'runtime', exitCode: 1 })
        .error('second', { kind: 'runtime', exitCode: 2 }),
    );
    const payloadFailure = await rejectionOf(
      client(withPayload, {
        transport: new FakeTransport(() => {
          throw {
            tag: 'remote-tool-error',
            val: { tag: 'custom-error', val: wireValue(z.string(), 'failure') },
          } satisfies RpcError;
        }),
      })['with-payload']({}),
    );
    const payloadlessFailure = await rejectionOf(
      client(payloadless, {
        transport: new FakeTransport(() => {
          throw {
            tag: 'remote-tool-error',
            val: {
              tag: 'custom-error',
              val: typedSchemaValueToWit({
                graph: { defs: new Map(), root: t.tuple([]) },
                value: v.tuple([]),
              }),
            },
          } satisfies RpcError;
        }),
      }).payloadless({}),
    );

    expect(payloadFailure).toMatchObject({
      cause: {
        tag: 'tool',
        error: { tag: 'err', name: 'first', hasPayload: true, payload: 'failure' },
      },
    });
    expect(payloadlessFailure).toMatchObject({
      cause: {
        tag: 'tool',
        error: { tag: 'err', name: 'first', hasPayload: false },
      },
    });
  });

  it('decodes an owned quota-token result exactly once', async () => {
    const schema = s.quotaToken();
    const codec = compileSchema(schema);
    const definition = toolDefinition('quota-result').body((body) => body.returns(schema));
    const raw = { [Symbol.dispose]: vi.fn() } as never;

    await expect(
      client(definition, {
        transport: new FakeTransport(() => ({
          result: typedSchemaValueToWit({
            graph: codec.graph,
            value: codec.toValue(raw),
          }),
        })),
      })['quota-result']({}),
    ).resolves.toBe(raw);
  });

  it('accepts allowed MIME metadata when projecting a binary result to Uint8Array', async () => {
    const schema = Bytes({ mimeTypes: ['application/octet-stream'] });
    const codec = compileSchema(schema);
    const bytes = new Uint8Array([1, 2, 3]);
    const definition = toolDefinition('binary-result').body((body) => body.returns(schema));

    await expect(
      client(definition, {
        transport: new FakeTransport(() => ({
          result: typedSchemaValueToWit({
            graph: codec.graph,
            value: { tag: 'binary', bytes, mimeType: 'application/octet-stream' },
          }),
        })),
      })['binary-result']({}),
    ).resolves.toEqual(bytes);
  });

  it('maps input encoding and result decoding failures to stable protocol errors', async () => {
    const definition = toolDefinition('codec').body((body) =>
      body.positional('input', z.string()).returns(z.number()),
    );
    const invalidInput = await rejectionOf(
      client(definition, { transport: new FakeTransport(() => ({})) }).codec({
        input: 42 as never,
      }),
    );
    const invalidResult = await rejectionOf(
      client(definition, {
        transport: new FakeTransport(() => ({ result: wireValue(z.string(), 'wrong') })),
      }).codec({ input: 'valid' }),
    );

    expect(invalidInput).toMatchObject({
      cause: { tag: 'rpc', error: { tag: 'protocol-error' } },
    });
    expect(invalidResult).toMatchObject({
      cause: {
        tag: 'rpc',
        error: { tag: 'protocol-error', val: expect.stringContaining('schema') },
      },
    });
  });

  it('rejects a result value that does not conform to its matching schema graph', async () => {
    const codec = compileSchema(z.object({ value: z.string() }));
    const definition = toolDefinition('invalid-value').body((body) =>
      body.returns(z.object({ value: z.string() })),
    );
    const failure = await rejectionOf(
      client(definition, {
        transport: new FakeTransport(() => ({
          result: typedSchemaValueToWit({
            graph: codec.graph,
            value: v.record([v.bool(true)]),
          }),
        })),
      })['invalid-value']({}),
    );

    expect(failure).toMatchObject({
      cause: {
        tag: 'rpc',
        error: { tag: 'protocol-error', val: expect.stringContaining('does not conform') },
      },
    });
  });

  it('encodes and decodes Zod 3, Zod 4, and ArkType definitions', async () => {
    const zod3Definition = toolDefinition('zod-three').body((body) =>
      body
        .positional('input', z3.object({ message: z3.string() }))
        .returns(z3.object({ length: z3.number() })),
    );
    const zod4Definition = toolDefinition('zod-four').body((body) =>
      body
        .positional('input', z.object({ message: z.string() }))
        .returns(z.object({ length: z.number() })),
    );
    const arkDefinition = toolDefinition('ark').body((body) =>
      body
        .positional('input', arkType({ message: 'string' }))
        .returns(arkType({ length: 'number' })),
    );

    const zod3Transport = new FakeTransport(() => ({
      result: wireValue(z3.object({ length: z3.number() }), { length: 3 }),
    }));
    const zod4Transport = new FakeTransport(() => ({
      result: wireValue(z.object({ length: z.number() }), { length: 4 }),
    }));
    const arkTransport = new FakeTransport(() => ({
      result: wireValue(arkType({ length: 'number' }), { length: 5 }),
    }));

    await expect(
      client(zod3Definition, { transport: zod3Transport })['zod-three']({
        input: { message: 'old' },
      }),
    ).resolves.toEqual({ length: 3 });
    await expect(
      client(zod4Definition, { transport: zod4Transport })['zod-four']({
        input: { message: 'new' },
      }),
    ).resolves.toEqual({ length: 4 });
    await expect(
      client(arkDefinition, { transport: arkTransport }).ark({ input: { message: 'ark' } }),
    ).resolves.toEqual({ length: 5 });

    expect(typedSchemaValueFromWit(zod3Transport.invocations[0].input).value).toEqual(
      v.record([v.record([v.string('old')])]),
    );
    expect(typedSchemaValueFromWit(zod4Transport.invocations[0].input).value).toEqual(
      v.record([v.record([v.string('new')])]),
    );
    expect(typedSchemaValueFromWit(arkTransport.invocations[0].input).value).toEqual(
      v.record([v.record([v.string('ark')])]),
    );
  });

  it('lazily creates and reuses the default ToolRpc for streamless calls', async () => {
    const resultCodec = compileSchema(z.string());
    const invokeAndAwait = vi.fn(() => ({
      result: typedSchemaValueToWit({
        graph: resultCodec.graph,
        value: resultCodec.toValue('ok'),
      }),
    }));
    vi.mocked(ToolRpc)
      .mockClear()
      .mockImplementation(() => ({ invokeAndAwait }) as unknown as ToolRpc);
    const definition = toolDefinition('default').body((body) => body.returns(z.string()));
    const runtime = client(definition);

    expect(ToolRpc).not.toHaveBeenCalled();
    await expect(runtime.default({})).resolves.toBe('ok');
    await expect(runtime.default({})).resolves.toBe('ok');

    expect(ToolRpc).toHaveBeenCalledOnce();
    expect(ToolRpc).toHaveBeenCalledWith('default');
    expect(invokeAndAwait).toHaveBeenCalledTimes(2);
    expect(invokeAndAwait.mock.calls.every(([path]) => deepEqual(path, []))).toBe(true);
  });

  it('fails unsupported default host stream adaptation through stable protocol errors', async () => {
    const stdinDefinition = toolDefinition('stdin-host').body((body) =>
      body.stdin({ required: true }).returns(z.void()),
    );
    const stdinFailure = await rejectionOf(
      client(stdinDefinition)['stdin-host']({ stdin: new ReadableStream<Uint8Array>() }),
    );
    expect(stdinFailure).toMatchObject({
      cause: {
        tag: 'rpc',
        error: { tag: 'protocol-error', val: expect.stringContaining('wasi:io input-stream') },
      },
    });

    const dispose = vi.fn();
    vi.mocked(ToolRpc).mockImplementation(
      () =>
        ({
          invokeAndAwait: () => ({ stdout: { [Symbol.dispose]: dispose } }),
        }) as unknown as ToolRpc,
    );
    const stdoutDefinition = toolDefinition('stdout-host').body((body) =>
      body.stdout({ required: true }).returns(z.void()),
    );
    const stdoutFailure = await rejectionOf(client(stdoutDefinition)['stdout-host']({}));

    expect(stdoutFailure).toMatchObject({
      cause: {
        tag: 'rpc',
        error: { tag: 'protocol-error', val: expect.stringContaining('write-only') },
      },
    });
    expect(dispose).toHaveBeenCalledOnce();
  });
});
