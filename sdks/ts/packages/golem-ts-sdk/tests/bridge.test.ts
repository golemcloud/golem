// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { WasmRpc } from 'golem:agent/host@2.0.0';
import { ToolRpc, type RpcError } from 'golem:tool/host@0.1.0';
import type { OutputStream } from 'wasi:io/streams@0.2.3';
import { describe, expect, it, vi } from 'vitest';
import { bridge } from '../src';

const schemaGraphFromObject = (value: unknown) => bridge.schemaGraphFromJson(JSON.stringify(value));

describe('public bridge runtime', () => {
  it('parses rich serde schema graphs without losing integer precision or metadata', () => {
    const graph = schemaGraphFromObject({
      defs: [
        {
          id: 'item',
          name: 'example.Item',
          body: {
            kind: 's64',
            value: {
              restrictions: {
                min: { kind: 'signed', value: '-9223372036854775808' },
                max: { kind: 'float-bits', value: '18446744073709551615' },
                unit: 'items',
              },
              metadata: {
                doc: 'count',
                aliases: ['n'],
                examples: ['1'],
                role: { tag: 'other', value: 'counter' },
              },
            },
          },
        },
      ],
      root: {
        kind: 'record',
        value: {
          fields: [
            {
              name: 'items',
              body: { kind: 'list', value: { element: { kind: 'ref', value: { id: 'item' } } } },
              metadata: { role: { tag: 'multimodal' } },
            },
            {
              name: 'amount',
              body: {
                kind: 'quantity',
                value: {
                  spec: {
                    baseUnit: 'USD',
                    allowedSuffixes: ['USD'],
                    min: { mantissa: '9007199254740993001', scale: 2, unit: 'USD' },
                  },
                },
              },
            },
          ],
        },
      },
    });

    expect(graph.root.body.tag).toBe('record');
    expect(graph.defs.get('item')).toMatchObject({
      name: 'example.Item',
      body: { metadata: { doc: 'count', role: { tag: 'other', val: 'counter' } } },
    });
    const numeric = graph.defs.get('item')!.body.body;
    expect(numeric.tag === 's64' && numeric.restrictions?.min?.val).toBe(-9223372036854775808n);
    expect(numeric.tag === 's64' && numeric.restrictions?.max?.val).toBe(18446744073709551615n);
    const fields = graph.root.body.tag === 'record' ? graph.root.body.fields : [];
    expect(fields[0].metadata.role).toEqual({ tag: 'multimodal' });
    const quantity = fields[1].body.body;
    expect(quantity.tag === 'quantity' && quantity.spec.min?.mantissa).toBe(9007199254740993001n);
  });

  it('parses stringified precision-sensitive bridge integers exactly', () => {
    const graph = bridge.schemaGraphFromJson(`{
      "defs": [
        {"id":"unsigned","body":{"kind":"u64","value":{"restrictions":{"max":{"kind":"unsigned","value":"18446744073709551615"}}}}}
      ],
      "root":{"kind":"string"}
    }`);

    const unsigned = graph.defs.get('unsigned')!.body.body;
    expect(unsigned.tag === 'u64' && unsigned.restrictions?.max?.val).toBe(18446744073709551615n);
  });

  it('rejects duplicate schema definition IDs', () => {
    const malformed = {
      defs: [
        { id: 'x', body: { kind: 'bool', value: {} } },
        { id: 'x', body: { kind: 'string', value: {} } },
      ],
      root: { kind: 'ref', value: { id: 'x' } },
    };

    expect(() => schemaGraphFromObject(malformed)).toThrow(/duplicate.*x/i);
  });

  it('accepts safe numeric forms for precision-sensitive bridge integers', () => {
    const root = schemaGraphFromObject({
      root: {
        kind: 'quantity',
        value: { spec: { baseUnit: 'x', min: { mantissa: 42, scale: 0, unit: 'x' } } },
      },
    }).root.body;
    expect(root.tag === 'quantity' && root.spec.min?.mantissa).toBe(42n);
  });

  it('rejects unsafe numeric forms for precision-sensitive bridge integers', () => {
    expect(() =>
      schemaGraphFromObject({
        root: {
          kind: 'u64',
          value: { restrictions: { max: { kind: 'unsigned', value: 18446744073709552000 } } },
        },
      }),
    ).toThrow('Expected safe integer');
    expect(() =>
      schemaGraphFromObject({
        root: {
          kind: 'quantity',
          value: {
            spec: { baseUnit: 'x', min: { mantissa: 9007199254740992, scale: 0, unit: 'x' } },
          },
        },
      }),
    ).toThrow('Expected safe integer');
  });

  it.each([
    [
      'signed bound below i64',
      {
        root: {
          kind: 's64',
          value: { restrictions: { min: { kind: 'signed', value: '-9223372036854775809' } } },
        },
      },
    ],
    [
      'negative u64 bound',
      {
        root: {
          kind: 'u64',
          value: { restrictions: { min: { kind: 'unsigned', value: '-1' } } },
        },
      },
    ],
    [
      'float bits above u64',
      {
        root: {
          kind: 'f64',
          value: {
            restrictions: { max: { kind: 'float-bits', value: '18446744073709551616' } },
          },
        },
      },
    ],
    [
      'fixed-list length above u32',
      {
        root: {
          kind: 'fixed-list',
          value: { element: { kind: 'u8' }, length: 4_294_967_296 },
        },
      },
    ],
    [
      'negative text minimum length',
      {
        root: { kind: 'text', value: { restrictions: { minLength: -1 } } },
      },
    ],
    [
      'binary maximum bytes above u32',
      {
        root: { kind: 'binary', value: { restrictions: { maxBytes: 4_294_967_296 } } },
      },
    ],
    [
      'quantity mantissa above i64',
      {
        root: {
          kind: 'quantity',
          value: {
            spec: {
              baseUnit: 'x',
              min: { mantissa: '9223372036854775808', scale: 0, unit: 'x' },
            },
          },
        },
      },
    ],
    [
      'quantity scale above i32',
      {
        root: {
          kind: 'quantity',
          value: {
            spec: {
              baseUnit: 'x',
              min: { mantissa: '0', scale: 2_147_483_648, unit: 'x' },
            },
          },
        },
      },
    ],
  ])('rejects bridge integers outside the source schema field domains: %s', (_case, graph) => {
    expect(() => schemaGraphFromObject(graph)).toThrow();
  });

  it.each([
    ['signed i64', 's64', { min: { kind: 'signed', value: '-9223372036854775808' } }],
    ['unsigned u64', 'u64', { max: { kind: 'unsigned', value: '18446744073709551615' } }],
    ['float bits u64', 'f64', { max: { kind: 'float-bits', value: '18446744073709551615' } }],
  ])('accepts the %s schema bound endpoint', (_case, kind, restrictions) => {
    expect(() => schemaGraphFromObject({ root: { kind, value: { restrictions } } })).not.toThrow();
  });

  it('accepts integer endpoints for u32, i64, and i32 schema fields', () => {
    for (const root of [
      { kind: 'fixed-list', value: { element: { kind: 'u8' }, length: 4_294_967_295 } },
      { kind: 'text', value: { restrictions: { minLength: 0, maxLength: 4_294_967_295 } } },
      { kind: 'binary', value: { restrictions: { minBytes: 0, maxBytes: 4_294_967_295 } } },
      {
        kind: 'quantity',
        value: {
          spec: {
            baseUnit: 'x',
            min: { mantissa: '-9223372036854775808', scale: -2_147_483_648, unit: 'x' },
            max: { mantissa: '9223372036854775807', scale: 2_147_483_647, unit: 'x' },
          },
        },
      },
    ]) {
      expect(() => schemaGraphFromObject({ root })).not.toThrow();
    }
  });

  it('normalizes negative-zero float bounds when parsing schema JSON', () => {
    const root = schemaGraphFromObject({
      root: {
        kind: 'f64',
        value: {
          restrictions: {
            min: { kind: 'float-bits', value: '9223372036854775808' },
          },
        },
      },
    }).root.body;

    expect(root.tag === 'f64' && root.restrictions?.min?.val).toBe(0n);
  });

  it('rejects malformed metadata', () => {
    const malformed = {
      root: { kind: 'string', value: { metadata: { role: { tag: 'other', value: 42 } } } },
    };
    expect(() => schemaGraphFromObject(malformed)).toThrow(
      'Expected string at metadata.role.value',
    );
  });

  it('validates optional quota-token resource names', () => {
    for (const spec of [{}, { resourceName: null }, { resourceName: 'cpu' }]) {
      const root = schemaGraphFromObject({
        root: { kind: 'quota-token', value: { spec } },
      }).root.body;
      expect(root.tag === 'quota-token' && root.spec.resourceName).toBe(
        spec.resourceName === 'cpu' ? 'cpu' : undefined,
      );
    }
    expect(() =>
      schemaGraphFromObject({
        root: { kind: 'quota-token', value: { spec: { resourceName: 42 } } },
      }),
    ).toThrow('Expected string');
  });

  it('parses every Rust-serde union discriminator and lowers the graph to WIT', () => {
    const rules = [
      { rule: 'prefix', value: { prefix: 'a' } },
      { rule: 'suffix', value: { suffix: 'z' } },
      { rule: 'contains', value: { substring: 'mid' } },
      { rule: 'regex', value: { regex: '^x$' } },
      { rule: 'field-equals', value: { fieldName: 'kind', literal: 'x' } },
      { rule: 'field-equals', value: { fieldName: 'kind' } },
      { rule: 'field-absent', value: { fieldName: 'missing' } },
    ];
    const stringBody = { kind: 'string' };
    const recordBody = {
      kind: 'record',
      value: { fields: [{ name: 'kind', body: stringBody }] },
    };
    const graph = schemaGraphFromObject({
      root: {
        kind: 'union',
        value: {
          spec: {
            branches: rules.map((discriminator, index) => ({
              tag: `case${index}`,
              body: index < 4 ? stringBody : recordBody,
              discriminator,
            })),
          },
        },
      },
    });
    const discriminators =
      graph.root.body.tag === 'union'
        ? graph.root.body.branches.map((branch) => branch.discriminator)
        : [];
    expect(discriminators).toEqual([
      { tag: 'prefix', val: 'a' },
      { tag: 'suffix', val: 'z' },
      { tag: 'contains', val: 'mid' },
      { tag: 'regex', val: '^x$' },
      { tag: 'field-equals', val: { fieldName: 'kind', literal: 'x' } },
      { tag: 'field-equals', val: { fieldName: 'kind', literal: undefined } },
      { tag: 'field-absent', val: 'missing' },
    ]);
    expect(bridge.schemaGraphFromWit(bridge.schemaGraphToWit(graph))).toEqual(graph);
    expect(() =>
      schemaGraphFromObject({
        root: {
          kind: 'union',
          value: {
            spec: {
              branches: [
                { tag: 'x', body: stringBody, discriminator: { rule: 'new-rule', value: {} } },
              ],
            },
          },
        },
      }),
    ).toThrow("Unknown discriminator rule 'new-rule'");
  });

  it('normalizes metadata roles and empty numeric restrictions', () => {
    const parse = (metadata: unknown) =>
      schemaGraphFromObject({ root: { kind: 'string', value: { metadata } } }).root.metadata.role;
    expect(parse({ role: { tag: 'future-role' } })).toEqual({ tag: 'other', val: 'future-role' });
    expect(parse({ role: { tag: 'multimodal' } })).toEqual({ tag: 'multimodal' });
    expect(parse({ role: { tag: 'unstructured-text' } })).toEqual({ tag: 'unstructured-text' });
    expect(parse({ role: { tag: 'unstructured-binary' } })).toEqual({ tag: 'unstructured-binary' });
    expect(parse({ role: { tag: 'other', value: 'custom' } })).toEqual({
      tag: 'other',
      val: 'custom',
    });
    for (const restrictions of [{}, { unit: '' }]) {
      const root = schemaGraphFromObject({
        root: { kind: 'u32', value: { restrictions } },
      }).root.body;
      expect(root.tag === 'u32' && root.restrictions).toBeUndefined();
    }
  });

  it.each([
    ['text', { restrictions: { languages: ['en'], minLength: 1, maxLength: 4, regex: '^x' } }],
    ['binary', { restrictions: { mimeTypes: ['image/png'], minBytes: 1, maxBytes: 8 } }],
    ['path', { spec: { direction: 'input', kind: 'file', allowedExtensions: ['txt'] } }],
    ['url', { restrictions: { allowedSchemes: ['https'], allowedHosts: ['golem.cloud'] } }],
    ['secret', { spec: {} }],
    ['secret', { spec: { inner: { kind: 'u8' }, category: 'key' } }],
    ['enum', { cases: ['a', 'b'] }],
    ['flags', { flags: ['a', 'b'] }],
    ['variant', { cases: [{ name: 'a' }, { name: 'b', payload: { kind: 'string' } }] }],
    ['tuple', { elements: [{ kind: 'string' }, { kind: 'u8' }] }],
    ['map', { key: { kind: 'string' }, value: { kind: 'u8' } }],
    ['option', { inner: { kind: 'string' } }],
    ['result', { spec: { ok: { kind: 'string' }, err: { kind: 'u8' } } }],
    ['fixed-list', { element: { kind: 'u8' }, length: 2 }],
    ['future', { inner: { kind: 'string' } }],
    ['stream', { inner: { kind: 'string' } }],
  ])('parses and WIT-round-trips the %s structural kind', (kind, value) => {
    const graph = schemaGraphFromObject({ root: { kind, value } });
    expect(bridge.schemaGraphFromWit(bridge.schemaGraphToWit(graph))).toEqual(graph);
  });

  it('provides lazy definition-independent tool transport and model conversion', async () => {
    const runtime = bridge.createToolClientRuntime('git');
    expect(ToolRpc).not.toHaveBeenCalled();
    const wire = bridge.typedSchemaValueToWit({
      graph: { defs: new Map(), root: bridge.t.string() },
      value: bridge.v.string('ok'),
    });
    vi.mocked(ToolRpc).mockImplementationOnce(
      () => ({ invokeAndAwait: vi.fn(() => ({ result: wire })) }) as never,
    );
    const result = await runtime.invokeAndAwait(['status'], {
      graph: { defs: new Map(), root: bridge.t.tuple([]) },
      value: bridge.v.tuple([]),
    });
    expect(result.result?.value).toEqual(bridge.v.string('ok'));
    expect(ToolRpc).toHaveBeenCalledWith('git');
  });

  it('disposes stdout exactly once when structured tool result decoding fails', async () => {
    const dispose = vi.fn();
    const stdout = { [Symbol.dispose]: dispose } as unknown as OutputStream;
    const runtime = bridge.createToolClientRuntime('broken-result', {
      invokeAndAwait: () => ({
        result: {
          graph: { typeNodes: [], defs: [], root: 0 },
          value: { valueNodes: [], root: 0 },
        },
        stdout,
      }),
    });

    await expect(
      runtime.invokeAndAwait([], {
        graph: { defs: new Map(), root: bridge.t.tuple([]) },
        value: bridge.v.tuple([]),
      }),
    ).rejects.toThrow();
    expect(dispose).toHaveBeenCalledOnce();
  });

  it('transfers stdout ownership when structured tool result decoding succeeds', async () => {
    const dispose = vi.fn();
    const stdout = { [Symbol.dispose]: dispose } as unknown as OutputStream;
    const typed = {
      graph: { defs: new Map(), root: bridge.t.string() },
      value: bridge.v.string('ok'),
    };
    const runtime = bridge.createToolClientRuntime('valid-result', {
      invokeAndAwait: () => ({ result: bridge.typedSchemaValueToWit(typed), stdout }),
    });

    await expect(
      runtime.invokeAndAwait([], {
        graph: { defs: new Map(), root: bridge.t.tuple([]) },
        value: bridge.v.tuple([]),
      }),
    ).resolves.toEqual({ result: typed, stdout });
    expect(dispose).not.toHaveBeenCalled();
  });

  it('splits declared custom errors from stable RPC errors', () => {
    const typed = {
      graph: { defs: new Map(), root: bridge.t.string() },
      value: bridge.v.string('bad'),
    };
    expect(
      bridge.splitToolRpcError(
        {
          tag: 'remote-tool-error',
          val: { tag: 'custom-error', val: bridge.typedSchemaValueToWit(typed) },
        },
        (payload) => payload.value,
      ),
    ).toEqual({ tag: 'tool', error: bridge.v.string('bad') });
    expect(bridge.splitToolRpcError({ tag: 'denied', val: 'no' }, () => 'unused')).toEqual({
      tag: 'rpc',
      error: { tag: 'denied', val: 'no' },
    });
  });

  it('guards and splits host-thrown RPC values', () => {
    const custom = {
      tag: 'remote-tool-error',
      val: {
        tag: 'custom-error',
        val: bridge.typedSchemaValueToWit({
          graph: { defs: new Map(), root: bridge.t.string() },
          value: bridge.v.string('bad'),
        }),
      },
    } satisfies RpcError;
    for (const error of [custom, { tag: 'denied', val: 'no' } satisfies RpcError]) {
      expect(bridge.isRpcError(error)).toBe(true);
      expect(bridge.splitToolRpcError(error, (payload) => payload.value).tag).toBe(
        error === custom ? 'tool' : 'rpc',
      );
    }
    for (const malformed of [
      null,
      {},
      { tag: 'denied' },
      { tag: 'denied', val: 1 },
      { tag: 'remote-tool-error', val: { tag: 'invalid-input' } },
    ]) {
      expect(bridge.isRpcError(malformed)).toBe(false);
    }
  });

  it('does not classify an array with attached properties as a host RPC error record', () => {
    const malformed = Object.assign([], { tag: 'denied', val: 'no' });

    expect(bridge.isRpcError(malformed)).toBe(false);
  });

  it('does not classify a sparse invalid-command-path carrier as a host RPC error', () => {
    const path = new Array<string>(1);
    const malformed = {
      tag: 'remote-tool-error',
      val: { tag: 'invalid-command-path', val: path },
    };

    expect(bridge.isRpcError(malformed)).toBe(false);
  });

  it('does not classify malformed custom-error typed payloads as host RPC errors', () => {
    const malformed = {
      tag: 'remote-tool-error',
      val: { tag: 'custom-error', val: { graph: null, value: null } },
    };

    expect(bridge.isRpcError(malformed)).toBe(false);
  });

  it('does not consume an owned custom-error payload while classifying it', () => {
    const raw = { [Symbol.dispose]: vi.fn() };
    const payload = {
      graph: bridge.schemaGraphToWit({
        defs: new Map(),
        root: bridge.t.secret(bridge.t.string()),
      }),
      value: {
        valueNodes: [{ tag: 'secret-value' as const, val: raw as never }],
        root: 0,
      },
    };
    const error = {
      tag: 'remote-tool-error',
      val: { tag: 'custom-error', val: payload },
    } as unknown as RpcError;

    expect(bridge.isRpcError(error)).toBe(true);
    expect(payload.value.valueNodes[0].val).toBe(raw);
    expect(bridge.splitToolRpcError(error, (decoded) => decoded.value.tag)).toEqual({
      tag: 'tool',
      error: 'secret',
    });
  });

  it('rejects custom-error payloads whose rich value records are malformed', () => {
    const payload = {
      graph: bridge.schemaGraphToWit({
        defs: new Map(),
        root: bridge.schemaType({ tag: 'binary', restrictions: {} }),
      }),
      value: {
        valueNodes: [{ tag: 'binary-value' as const, val: null as never }],
        root: 0,
      },
    };
    const error = {
      tag: 'remote-tool-error',
      val: { tag: 'custom-error', val: payload },
    } as unknown as RpcError;

    expect(() => bridge.typedSchemaValueFromWit(payload)).toThrow();
    expect(bridge.isRpcError(error)).toBe(false);
  });

  it('rejects out-of-range wire datetimes without consuming an owned sibling', () => {
    const raw = { [Symbol.dispose]: vi.fn() };
    const payload = {
      graph: bridge.schemaGraphToWit({
        defs: new Map(),
        root: bridge.t.tuple([bridge.t.secret(bridge.t.string()), bridge.t.datetime()]),
      }),
      value: {
        valueNodes: [
          { tag: 'tuple-value' as const, val: [1, 2] },
          { tag: 'secret-value' as const, val: raw as never },
          {
            tag: 'datetime-value' as const,
            val: { seconds: 0n, nanoseconds: 1_000_000_000 },
          },
        ],
        root: 0,
      },
    };
    const error = {
      tag: 'remote-tool-error',
      val: { tag: 'custom-error', val: payload },
    } as unknown as RpcError;

    expect.soft(bridge.isRpcError(error)).toBe(false);
    expect.soft(payload.value.valueNodes[1].val).toBe(raw);
    expect.soft(() => bridge.typedSchemaValueFromWit(payload)).toThrow(/datetime/i);
    expect.soft(payload.value.valueNodes[1].val).toBe(raw);
  });

  it('releases an owned quota token when typed decoding rejects the graph', () => {
    const raw = { [Symbol.dispose]: vi.fn() };
    const payload = {
      graph: { typeNodes: [], defs: [], root: 0 },
      value: {
        valueNodes: [{ tag: 'quota-token-handle' as const, val: raw as never }],
        root: 0,
      },
    };
    const error = {
      tag: 'remote-tool-error',
      val: { tag: 'custom-error', val: payload },
    } as unknown as RpcError;

    expect(bridge.isRpcError(error)).toBe(false);
    expect(payload.value.valueNodes[0].val).toBe(raw);
    expect(() => bridge.typedSchemaValueFromWit(payload)).toThrow(/type node index/i);
    expect(payload.value.valueNodes[0].val).toBeUndefined();
  });

  it('wraps malformed raw agent output with agent and method context', async () => {
    const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
    const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      asyncInvokeAndAwait: ReturnType<typeof vi.fn>;
    };
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'example', idempotencyKey: 'key' },
      future: {
        subscribe: vi.fn().mockReturnValue({ promise: vi.fn().mockResolvedValue(undefined) }),
        get: vi.fn().mockReturnValue({ tag: 'ok', val: { tag: 'not-a-schema-value' } }),
        cancel: vi.fn(),
      },
    });
    await expect(remote.invokeAndAwait('broken', bridge.v.tuple([]))).rejects.toMatchObject({
      _tag: 'RemoteCallError',
      message: expect.stringContaining('.broken returned an invalid schema value'),
    });
  });

  it('returns scheduled invocation metadata when requested', () => {
    const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
    const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      scheduleInvocation: ReturnType<typeof vi.fn>;
    };
    const receipt = { metadata: { agentId: 'example', idempotencyKey: 'scheduled' } };
    rpc.scheduleInvocation.mockReturnValue(receipt);
    const at = { seconds: 1n, nanoseconds: 0 };

    expect(remote.scheduleWithMetadata(at, 'run', bridge.v.tuple([]))).toBe(receipt);
    expect(rpc.scheduleInvocation).toHaveBeenCalledWith(at, 'run', expect.anything());
  });

  it('best-effort disposes the future after an awaited agent invocation', async () => {
    const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
    const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      asyncInvokeAndAwait: ReturnType<typeof vi.fn>;
    };
    const dispose = vi.fn();
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'example', idempotencyKey: 'key' },
      future: {
        subscribe: vi.fn().mockReturnValue({ promise: vi.fn().mockResolvedValue(undefined) }),
        get: vi.fn().mockReturnValue({ tag: 'ok', val: undefined }),
        cancel: vi.fn(),
        [Symbol.dispose]: dispose,
      },
    });

    await expect(remote.invokeAndAwait('ping', bridge.v.tuple([]))).resolves.toBeUndefined();
    expect(dispose).toHaveBeenCalledOnce();
  });

  it('consumes an agent future terminal cancellation result before disposing it', async () => {
    const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
    const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      asyncInvokeAndAwait: ReturnType<typeof vi.fn>;
    };
    const controller = new AbortController();
    const cancel = vi.fn();
    const get = vi.fn().mockImplementation(() => {
      throw new Error('terminal get failed');
    });
    const dispose = vi.fn();
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'example', idempotencyKey: 'key' },
      future: {
        subscribe: vi.fn().mockReturnValue({
          abortablePromise: (signal: AbortSignal) =>
            new Promise<void>((_resolve, reject) => {
              signal.addEventListener('abort', () => reject(signal.reason), { once: true });
            }),
        }),
        get,
        cancel,
        [Symbol.dispose]: dispose,
      },
    });

    const invocation = remote.invokeAndAwait('ping', bridge.v.tuple([]), controller.signal);
    controller.abort(new Error('cancelled by caller'));

    await expect(invocation).rejects.toThrow('cancelled by caller');
    expect(cancel).toHaveBeenCalledOnce();
    expect(get).toHaveBeenCalledOnce();
    expect(dispose).toHaveBeenCalledOnce();
    expect(cancel.mock.invocationCallOrder[0]).toBeLessThan(get.mock.invocationCallOrder[0]);
    expect(get.mock.invocationCallOrder[0]).toBeLessThan(dispose.mock.invocationCallOrder[0]);
  });

  it('cancels and consumes an agent future before disposal when polling fails', async () => {
    const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
    const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      asyncInvokeAndAwait: ReturnType<typeof vi.fn>;
    };
    const pollError = new Error('poll failed');
    const cancel = vi.fn().mockImplementation(() => {
      throw new Error('cancel failed');
    });
    const get = vi.fn().mockImplementation(() => {
      throw new Error('terminal get failed');
    });
    const dispose = vi.fn();
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'example', idempotencyKey: 'key' },
      future: {
        subscribe: vi.fn().mockReturnValue({ promise: vi.fn().mockRejectedValue(pollError) }),
        cancel,
        get,
        [Symbol.dispose]: dispose,
      },
    });

    await expect(remote.invokeAndAwait('ping', bridge.v.tuple([]))).rejects.toBe(pollError);
    expect(cancel).toHaveBeenCalledOnce();
    expect(get).toHaveBeenCalledOnce();
    expect(dispose).toHaveBeenCalledOnce();
    expect(cancel.mock.invocationCallOrder[0]).toBeLessThan(get.mock.invocationCallOrder[0]);
    expect(get.mock.invocationCallOrder[0]).toBeLessThan(dispose.mock.invocationCallOrder[0]);
  });

  it('cancels and terminally consumes an agent future when readiness yields no result', async () => {
    const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
    const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      asyncInvokeAndAwait: ReturnType<typeof vi.fn>;
    };
    const cancel = vi.fn();
    const get = vi
      .fn()
      .mockReturnValueOnce(undefined)
      .mockReturnValueOnce({ tag: 'err', val: { tag: 'protocol-error', val: 'cancelled' } });
    const dispose = vi.fn();
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'example', idempotencyKey: 'key' },
      future: {
        subscribe: vi.fn().mockReturnValue({ promise: vi.fn().mockResolvedValue(undefined) }),
        cancel,
        get,
        [Symbol.dispose]: dispose,
      },
    });

    await expect(remote.invokeAndAwait('ping', bridge.v.tuple([]))).rejects.toMatchObject({
      _tag: 'RemoteCallError',
      message: expect.stringContaining('failed (no result)'),
    });
    expect(cancel).toHaveBeenCalledOnce();
    expect(get).toHaveBeenCalledTimes(2);
    expect(dispose).toHaveBeenCalledOnce();
    expect(get.mock.invocationCallOrder[0]).toBeLessThan(cancel.mock.invocationCallOrder[0]);
    expect(cancel.mock.invocationCallOrder[0]).toBeLessThan(get.mock.invocationCallOrder[1]);
    expect(get.mock.invocationCallOrder[1]).toBeLessThan(dispose.mock.invocationCallOrder[0]);
  });

  it.each([1.5, Number.NaN])('rejects invalid datetime nanoseconds (%s)', (nanoseconds) => {
    expect(() => bridge.datetimeToISOString({ seconds: 0n, nanoseconds })).toThrow(/nanoseconds/i);
  });

  it('rejects datetimes outside the canonical four-digit-year domain', () => {
    expect(() => bridge.datetimeToISOString({ seconds: 253402300800n, nanoseconds: 0 })).toThrow();
    const endpoint = bridge.datetimeToISOString({ seconds: 253402214400n, nanoseconds: 0 });
    expect(() => bridge.datetimeFromISOString(endpoint)).not.toThrow();
    expect(bridge.datetimeToISOString({ seconds: -62167219200n, nanoseconds: 0 })).toBe(
      '0000-01-01T00:00:00Z',
    );
  });

  it.each(['2023-02-29T00:00:00Z', '2024-02-30T00:00:00Z', '2024-01-01', '2024-01-01T00:00:00'])(
    'rejects invalid or incomplete ISO instants (%s)',
    (value) => {
      expect(() => bridge.datetimeFromISOString(value)).toThrow();
    },
  );

  it('preserves valid ISO instant fractions', () => {
    const datetime = bridge.datetimeFromISOString('2024-02-29T12:34:56.123456789Z');
    expect(datetime).toEqual({ seconds: 1709210096n, nanoseconds: 123456789 });
    expect(bridge.datetimeToISOString(datetime)).toBe('2024-02-29T12:34:56.123456789Z');
  });
});
