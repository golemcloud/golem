// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { WasmRpc } from 'golem:agent/host@2.0.0';
import { createStdin, ToolRpc, type ByteStreamFailure, type RpcError } from 'golem:tool/host@0.1.0';
import { describe, expect, it, vi } from 'vitest';
import { bridge } from '../src';
import { GuestSchemaValueStreamHandle, validateSchemaGraph } from '../src/internal/schema-model';

const graph = (root: bridge.SchemaType): bridge.SchemaGraph => ({ defs: new Map(), root });
const streamFailures = [
  { tag: 'cancelled' },
  { tag: 'abandoned' },
  { tag: 'resource-exhausted' },
  { tag: 'failed', val: 'source failed' },
] satisfies ByteStreamFailure[];

describe('public bridge runtime', () => {
  it('validates both the graph and value of typed schema values', () => {
    const stringGraph = graph(bridge.t.string());
    const otherStringGraph = graph(
      bridge.schemaType({ tag: 'string' }, { ...bridge.emptyMetadata(), doc: 'different schema' }),
    );

    expect(
      bridge.typedSchemaValueConforms(stringGraph, {
        graph: stringGraph,
        value: bridge.v.string('ok'),
      }),
    ).toBe(true);
    expect(
      bridge.typedSchemaValueConforms(stringGraph, {
        graph: otherStringGraph,
        value: bridge.v.string('structurally compatible with the expected value'),
      }),
    ).toBe(false);
    expect(
      bridge.typedSchemaValueConforms(stringGraph, {
        graph: stringGraph,
        value: bridge.v.bool(true),
      }),
    ).toBe(false);
  });

  it('uses Unicode code-point semantics when validating typed values', () => {
    const textGraph = graph(bridge.schemaType({ tag: 'text', restrictions: { regex: '^.$' } }));

    expect(
      bridge.typedSchemaValueConforms(textGraph, {
        graph: textGraph,
        value: bridge.v.text('😀'),
      }),
    ).toBe(true);
  });

  it('rejects legacy regex escapes outside Unicode-mode ECMAScript', () => {
    const textGraph = graph(bridge.schemaType({ tag: 'text', restrictions: { regex: '^\\1$' } }));

    expect(validateSchemaGraph(textGraph)).toMatchObject([{ code: 'invalid-text-regex' }]);
  });

  it('WIT-round-trips every union discriminator', () => {
    const stringBody = bridge.t.string();
    const recordBody = bridge.t.record([bridge.field('kind', stringBody)]);
    const discriminators: bridge.DiscriminatorRule[] = [
      { tag: 'prefix', val: 'a' },
      { tag: 'suffix', val: 'z' },
      { tag: 'contains', val: 'mid' },
      { tag: 'regex', val: '^x$' },
      { tag: 'field-equals', val: { fieldName: 'kind', literal: 'x' } },
      { tag: 'field-equals', val: { fieldName: 'kind', literal: undefined } },
      { tag: 'field-absent', val: 'missing' },
    ];
    const unionGraph = graph(
      bridge.schemaType({
        tag: 'union',
        branches: discriminators.map((discriminator, index) => ({
          tag: `case${index}`,
          body: index < 4 ? stringBody : recordBody,
          discriminator,
          metadata: bridge.emptyMetadata(),
        })),
      }),
    );

    expect(bridge.schemaGraphFromWit(bridge.schemaGraphToWit(unionGraph))).toEqual(unionGraph);
  });

  it.each<[string, bridge.SchemaType]>([
    [
      'text',
      bridge.schemaType({
        tag: 'text',
        restrictions: { languages: ['en'], minLength: 1, maxLength: 4, regex: '^x' },
      }),
    ],
    [
      'binary',
      bridge.schemaType({
        tag: 'binary',
        restrictions: { mimeTypes: ['image/png'], minBytes: 1, maxBytes: 8 },
      }),
    ],
    ['path', bridge.t.path({ direction: 'input', kind: 'file', allowedExtensions: ['txt'] })],
    ['url', bridge.t.url({ allowedSchemes: ['https'], allowedHosts: ['golem.cloud'] })],
    ['secret', bridge.t.secret(bridge.t.u8(), { category: 'key' })],
    ['enum', bridge.t.enum(['a', 'b'])],
    ['flags', bridge.t.flags(['a', 'b'])],
    [
      'variant',
      bridge.t.variant([bridge.variantCase('a'), bridge.variantCase('b', bridge.t.string())]),
    ],
    ['tuple', bridge.t.tuple([bridge.t.string(), bridge.t.u8()])],
    ['map', bridge.t.map(bridge.t.string(), bridge.t.u8())],
    ['option', bridge.t.option(bridge.t.string())],
    ['result', bridge.t.result(bridge.t.string(), bridge.t.u8())],
    ['fixed-list', bridge.t.fixedList(bridge.t.u8(), 2)],
    ['future', bridge.schemaType({ tag: 'future', element: bridge.t.string() })],
    ['stream', bridge.schemaType({ tag: 'stream', element: bridge.t.string() })],
  ])('WIT-round-trips the %s structural kind', (_kind, root) => {
    const nativeGraph = graph(root);
    expect(bridge.schemaGraphFromWit(bridge.schemaGraphToWit(nativeGraph))).toEqual(nativeGraph);
  });

  it('provides lazy definition-independent tool transport and model conversion', async () => {
    const runtime = bridge.createToolClientRuntime('git');
    expect(ToolRpc).not.toHaveBeenCalled();
    const wire = bridge.typedSchemaValueToWit({
      graph: { defs: new Map(), root: bridge.t.string() },
      value: bridge.v.string('ok'),
    });
    vi.mocked(ToolRpc).mockImplementationOnce(
      () =>
        ({
          asyncInvokeAndAwait: vi.fn(() => ({
            get: () => Promise.resolve({ result: wire }),
            cancel: vi.fn(),
          })),
        }) as never,
    );
    const invocation = runtime.start(
      ['status'],
      {
        graph: { defs: new Map(), root: bridge.t.tuple([]) },
        value: bridge.v.tuple([]),
      },
      undefined,
      false,
    );
    const result = await bridge.resultFromSettledToolResult(invocation.settledResult);
    expect(result.result?.value).toEqual(bridge.v.string('ok'));
    expect(ToolRpc).toHaveBeenCalledWith('git');
  });

  it('stops a blocked stdin source when the host closes consumption', async () => {
    let closeConsumption!: () => void;
    const consumptionClosed = new Promise<void>((resolve) => {
      closeConsumption = resolve;
    });
    const writer = {
      write: vi.fn().mockResolvedValue(undefined),
      finish: vi.fn().mockResolvedValue(undefined),
      fail: vi.fn().mockResolvedValue(undefined),
    };
    vi.mocked(createStdin).mockReturnValue([
      writer,
      {},
      { wait: vi.fn(() => consumptionClosed) },
    ] as never);
    vi.mocked(ToolRpc).mockImplementationOnce(
      () =>
        ({
          asyncInvokeAndAwait: vi.fn(() => ({
            get: () => new Promise<never>(() => {}),
            cancel: vi.fn(),
          })),
        }) as never,
    );
    const cancelSource = vi.fn();
    let markReadStarted!: () => void;
    const readStarted = new Promise<void>((resolve) => {
      markReadStarted = resolve;
    });
    const source = new ReadableStream<Uint8Array>({
      pull() {
        markReadStarted();
        return new Promise(() => undefined);
      },
      cancel: cancelSource,
    });

    bridge.createToolClientTransport('blocked-stdin').start([], {} as never, source, false);
    await readStarted;
    closeConsumption();

    await vi.waitFor(() => expect(cancelSource).toHaveBeenCalledOnce(), { timeout: 100 });
  });

  it('starts the stdin pump before result completion and skips empty source chunks', async () => {
    const callOrder: string[] = [];
    const getResult = vi.fn(() => {
      callOrder.push('result');
      return new Promise<never>(() => {});
    });
    const writer = {
      write: vi.fn().mockResolvedValue(undefined),
      finish: vi.fn().mockResolvedValue(undefined),
      fail: vi.fn().mockResolvedValue(undefined),
    };
    vi.mocked(createStdin).mockReturnValue([
      writer,
      {},
      { wait: vi.fn(() => new Promise(() => undefined)) },
    ] as never);
    vi.mocked(ToolRpc).mockImplementationOnce(
      () =>
        ({
          asyncInvokeAndAwait: vi.fn(() => ({
            get: getResult,
            cancel: vi.fn(),
          })),
        }) as never,
    );
    const source = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new Uint8Array());
        controller.enqueue(Uint8Array.of(1, 2));
        controller.close();
      },
    });
    const getReader = source.getReader.bind(source);
    vi.spyOn(source, 'getReader').mockImplementationOnce(() => {
      callOrder.push('stdin');
      return getReader();
    });

    bridge.createToolClientTransport('empty-stdin-chunk').start([], {} as never, source, false);

    expect(callOrder).toEqual(['stdin', 'result']);
    await vi.waitFor(() => expect(writer.finish).toHaveBeenCalledOnce());
    expect(writer.write).toHaveBeenCalledOnce();
    expect(writer.write).toHaveBeenCalledWith(Uint8Array.of(1, 2));
    expect(writer.fail).not.toHaveBeenCalled();
  });

  it.each(streamFailures)('preserves a typed $tag stdin source failure', async (failure) => {
    const writer = {
      write: vi.fn().mockResolvedValue(undefined),
      finish: vi.fn().mockResolvedValue(undefined),
      fail: vi.fn().mockResolvedValue(undefined),
    };
    vi.mocked(createStdin).mockReturnValue([
      writer,
      {},
      { wait: vi.fn(() => new Promise(() => undefined)) },
    ] as never);
    vi.mocked(ToolRpc).mockImplementationOnce(
      () =>
        ({
          asyncInvokeAndAwait: vi.fn(() => ({
            get: () => new Promise<never>(() => {}),
            cancel: vi.fn(),
          })),
        }) as never,
    );
    const source = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.error(new bridge.ToolStreamError(failure));
      },
    });

    bridge.createToolClientTransport('failed-stdin').start([], {} as never, source, false);

    await vi.waitFor(() => expect(writer.fail).toHaveBeenCalledWith(failure));
  });

  it('maps an unknown stdin source exception to generic failure', async () => {
    const writer = {
      write: vi.fn().mockResolvedValue(undefined),
      finish: vi.fn().mockResolvedValue(undefined),
      fail: vi.fn().mockResolvedValue(undefined),
    };
    vi.mocked(createStdin).mockReturnValue([
      writer,
      {},
      { wait: vi.fn(() => new Promise(() => undefined)) },
    ] as never);
    vi.mocked(ToolRpc).mockImplementationOnce(
      () =>
        ({
          asyncInvokeAndAwait: vi.fn(() => ({
            get: () => new Promise<never>(() => {}),
            cancel: vi.fn(),
          })),
        }) as never,
    );
    const source = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.error(new Error('unknown source failure'));
      },
    });

    bridge.createToolClientTransport('failed-stdin').start([], {} as never, source, false);

    await vi.waitFor(() =>
      expect(writer.fail).toHaveBeenCalledWith({
        tag: 'failed',
        val: 'unknown source failure',
      }),
    );
  });

  it('forwards cancellation to a transport invocation whose cancel method uses its receiver', () => {
    const rawInvocation = {
      cancelled: false,
      settledResult: new Promise<never>(() => {}),
      cancel() {
        this.cancelled = true;
      },
    };
    const runtime = bridge.createToolClientRuntime('receiver-bound-cancel', {
      start: () => rawInvocation,
    });

    runtime
      .start(
        [],
        {
          graph: { defs: new Map(), root: bridge.t.tuple([]) },
          value: bridge.v.tuple([]),
        },
        undefined,
        false,
      )
      .cancel();

    expect(rawInvocation.cancelled).toBe(true);
  });

  it('keeps stdout independently consumable when structured result decoding fails', async () => {
    const close = vi.fn().mockResolvedValue({ done: true, value: undefined });
    const next = vi
      .fn()
      .mockResolvedValueOnce({
        done: false,
        value: { tag: 'ok', val: Uint8Array.of(1, 2) },
      })
      .mockResolvedValue({ done: true, value: undefined });
    const stdout = {
      [Symbol.asyncIterator]: () => ({
        next,
        return: close,
      }),
    };
    const runtime = bridge.createToolClientRuntime('broken-result', {
      start: () => ({
        settledResult: Promise.resolve({
          status: 'fulfilled',
          value: {
            result: {
              graph: { typeNodes: [], defs: [], root: 0 },
              value: { valueNodes: [], root: 0 },
            },
          },
        }),
        stdout,
        cancel: vi.fn(),
      }),
    });

    const invocation = runtime.start(
      [],
      {
        graph: { defs: new Map(), root: bridge.t.tuple([]) },
        value: bridge.v.tuple([]),
      },
      undefined,
      true,
    );
    await expect(bridge.resultFromSettledToolResult(invocation.settledResult)).rejects.toThrow();
    const chunks = [];
    for await (const item of invocation.stdout!) chunks.push(item);
    expect(chunks).toEqual([{ tag: 'ok', val: Uint8Array.of(1, 2) }]);
    expect(close).not.toHaveBeenCalled();
  });

  it('transfers stdout ownership when structured tool result decoding succeeds', async () => {
    const close = vi.fn().mockResolvedValue({ done: true, value: undefined });
    const stdout = {
      [Symbol.asyncIterator]: () => ({
        next: vi.fn().mockResolvedValue({ done: true, value: undefined }),
        return: close,
      }),
    };
    const typed = {
      graph: { defs: new Map(), root: bridge.t.string() },
      value: bridge.v.string('ok'),
    };
    const runtime = bridge.createToolClientRuntime('valid-result', {
      start: () => ({
        settledResult: Promise.resolve({
          status: 'fulfilled',
          value: { result: bridge.typedSchemaValueToWit(typed) },
        }),
        stdout,
        cancel: vi.fn(),
      }),
    });

    const invocation = runtime.start(
      [],
      {
        graph: { defs: new Map(), root: bridge.t.tuple([]) },
        value: bridge.v.tuple([]),
      },
      undefined,
      true,
    );
    await expect(bridge.resultFromSettledToolResult(invocation.settledResult)).resolves.toEqual({
      result: typed,
    });
    expect(invocation.stdout).toBe(stdout);
    expect(close).not.toHaveBeenCalled();
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

  it('keeps bridge capability conversions opaque and affine', () => {
    const assertOpaque = (handle: unknown) => {
      expect((handle as { take?: unknown }).take).toBeUndefined();
      expect((handle as { withHandle?: unknown }).withHandle).toBeUndefined();
      expect((handle as { isPresent?: unknown }).isPresent).toBeUndefined();
    };

    const secretRaw = { id: 'secret' } as never;
    const secret = bridge.secretHandleToSchemaValue(secretRaw);
    assertOpaque(secret.handle);
    expect(() => bridge.secretHandleToSchemaValue(secretRaw)).toThrow(/already owned/);
    expect(() =>
      bridge.schemaValueFromWit({
        valueNodes: [{ tag: 'secret-value', val: secretRaw }],
        root: 0,
      }),
    ).toThrow(/already owned/);
    expect(bridge.secretHandleFromSchemaValue(secret)).toBe(secretRaw);
    expect(() => bridge.secretHandleFromSchemaValue(secret)).toThrow(/already consumed/);
    expect(() => bridge.secretHandleToSchemaValue(secretRaw)).not.toThrow();

    const cardRaw = { id: 'permission-card' } as never;
    const card = bridge.permissionCardHandleToSchemaValue(cardRaw);
    assertOpaque(card.handle);
    expect(() => bridge.permissionCardHandleToSchemaValue(cardRaw)).toThrow(/already owned/);
    expect(() =>
      bridge.schemaValueFromWit({
        valueNodes: [{ tag: 'permission-card-handle', val: cardRaw }],
        root: 0,
      }),
    ).toThrow(/already owned/);
    expect(bridge.permissionCardHandleFromSchemaValue(card)).toBe(cardRaw);
    expect(() => bridge.permissionCardHandleFromSchemaValue(card)).toThrow(/already consumed/);

    const quotaRaw = { id: 'quota-token' } as never;
    const quotaValue = bridge.schemaValueFromWit({
      valueNodes: [{ tag: 'quota-token-handle', val: quotaRaw }],
      root: 0,
    });
    expect(quotaValue.tag).toBe('quota-token');
    if (quotaValue.tag !== 'quota-token') throw new Error('expected quota-token');
    assertOpaque(quotaValue.handle);
    const quotaAliasTree = {
      valueNodes: [{ tag: 'quota-token-handle' as const, val: quotaRaw }],
      root: 0,
    };
    expect(() => bridge.schemaValueFromWit(quotaAliasTree)).toThrow(/already owned/);

    let fakeKey: unknown;
    const fakeToken = {
      _toSchemaValue: (key: unknown) => {
        fakeKey = key;
        return quotaValue;
      },
    } as never;
    expect(() => bridge.quotaTokenToSchemaValue(fakeToken)).toThrow(/invalid quota token/);
    expect(fakeKey).toBeUndefined();

    let intercepted = false;
    const quotaClass = bridge.QuotaToken as unknown as Record<string, unknown>;
    const quotaPrototype = bridge.QuotaToken.prototype as unknown as Record<string, unknown>;
    quotaClass._fromSchemaValue = () => {
      intercepted = true;
    };
    quotaPrototype._toSchemaValue = () => {
      intercepted = true;
    };
    try {
      const token = bridge.quotaTokenFromSchemaValue(quotaValue);
      const alias = bridge.quotaTokenFromSchemaValue(quotaValue);
      const encoded = bridge.quotaTokenToSchemaValue(token);
      expect(intercepted).toBe(false);
      const wire = bridge.schemaValueToWit(encoded);
      expect(() =>
        bridge.schemaValueFromWit({
          valueNodes: [{ tag: 'quota-token-handle', val: quotaRaw }],
          root: 0,
        }),
      ).toThrow(/already owned/);
      expect(bridge.schemaValueFromWit(wire).tag).toBe('quota-token');
      expect(() => bridge.schemaValueToWit(bridge.quotaTokenToSchemaValue(alias))).toThrow(
        /already transferred/,
      );
    } finally {
      delete quotaClass._fromSchemaValue;
      delete quotaPrototype._toSchemaValue;
    }
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
        get: vi.fn().mockResolvedValue({ tag: 'not-a-schema-value' }),
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
    expect(rpc.scheduleInvocation).toHaveBeenCalledWith(at, 'run', expect.anything(), undefined);
  });

  it('returns unit output from an awaited agent invocation', async () => {
    const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
    const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      asyncInvokeAndAwait: ReturnType<typeof vi.fn>;
    };
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'example', idempotencyKey: 'key' },
      future: {
        get: vi.fn().mockResolvedValue(undefined),
        cancel: vi.fn(),
      },
    });

    await expect(remote.invokeAndAwait('ping', bridge.v.tuple([]))).resolves.toBeUndefined();
  });

  it('passes a recursively nested native stream to an awaited agent invocation', async () => {
    const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
    const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      asyncInvokeAndAwait: ReturnType<typeof vi.fn>;
    };
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'example', idempotencyKey: 'key' },
      future: {
        get: vi.fn().mockResolvedValue(undefined),
        cancel: vi.fn(),
      },
    });
    const source = (async function* () {})();
    const params = bridge.v.record([
      bridge.v.option(
        bridge.v.list([
          bridge.v.stream(new GuestSchemaValueStreamHandle({ kind: 'native', value: source })),
        ]),
      ),
    ]);

    await remote.invokeAndAwait('consume', params);

    const tree = rpc.asyncInvokeAndAwait.mock.calls[0][1];
    const streamNode = tree.valueNodes.find((node: { tag: string }) => node.tag === 'stream-value');
    expect(streamNode).toMatchObject({ tag: 'stream-value' });
    expect(streamNode.val).toMatchObject({ reader: source });
  });

  it.each(['invoke', 'schedule'] as const)(
    'rejects native streams at the non-awaited %s boundary',
    (boundary) => {
      const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
      const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
        invoke: ReturnType<typeof vi.fn>;
        scheduleInvocation: ReturnType<typeof vi.fn>;
      };
      const source = (async function* () {})();
      const params = bridge.v.option(
        bridge.v.stream(new GuestSchemaValueStreamHandle({ kind: 'native', value: source })),
      );

      expect(() => {
        if (boundary === 'invoke') remote.invoke('consume', params);
        else remote.schedule({ seconds: 1n, nanoseconds: 0 }, 'consume', params);
      }).toThrow('native schema value streams require asynchronous encoding');
      expect(rpc.invoke).not.toHaveBeenCalled();
      expect(rpc.scheduleInvocation).not.toHaveBeenCalled();
    },
  );

  it('cancels an agent future when the caller aborts', async () => {
    const remote = bridge.resolveRemoteAgent('Example', bridge.v.tuple([]));
    const rpc = vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      asyncInvokeAndAwait: ReturnType<typeof vi.fn>;
    };
    const controller = new AbortController();
    const cancel = vi.fn();
    const get = vi.fn().mockReturnValue(new Promise<never>(() => {}));
    rpc.asyncInvokeAndAwait.mockReturnValue({
      metadata: { agentId: 'example', idempotencyKey: 'key' },
      future: {
        get,
        cancel,
      },
    });

    const invocation = remote.invokeAndAwait('ping', bridge.v.tuple([]), controller.signal);
    await vi.waitFor(() => expect(get).toHaveBeenCalledOnce());
    controller.abort(new Error('cancelled by caller'));

    await expect(invocation).rejects.toThrow('cancelled by caller');
    expect(cancel).toHaveBeenCalledOnce();
    expect(get).toHaveBeenCalledOnce();
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
