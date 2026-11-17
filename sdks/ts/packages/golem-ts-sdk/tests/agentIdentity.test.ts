// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { makeAgentId, parseAgentId, WasmRpc } from 'golem:agent/host@2.0.0';
import { ParsedAgentId } from '../src/agentId';
import { defineAgent } from '../src/defineAgent';
import { method } from '../src/method';
import { z } from 'zod';
import { hostInvocationMetadata } from './invocationMetadataTestSupport';
import { getRawSelfAgentId } from '../src/host/hostapi';
import {
  resolveRemoteAgent,
  resolveRemoteAgentFallibly,
  RemoteCallError,
} from '../src/bridge/agent';
import { schemaValueToWit, v } from '../src/internal/schema-model';

describe('identity boundaries', () => {
  beforeEach(() => vi.clearAllMocks());

  it('retains separate semantic and wire caches while dynamic binding decodes freshly', () => {
    const original = v.string('original');
    const wire = schemaValueToWit(v.string('wire'));
    vi.mocked(parseAgentId).mockReturnValue([
      'HostType',
      { graph: { typeNodes: [], defs: [], root: 0 }, value: wire },
      undefined,
    ]);
    const identity = ParsedAgentId.create({ typeName: 'LocalType', constructorValue: original });
    const semantic = identity.parsed();
    expect(semantic).toEqual(['LocalType', original, undefined]);
    expect(semantic[1]).toBe(original);
    expect(parseAgentId).not.toHaveBeenCalled();
    const cachedWire = identity.parsedWire();
    expect(cachedWire[1]).toBe(wire);
    expect(identity.parsedWire()).toBe(cachedWire);
    expect(identity.parsed()).toBe(semantic);
    expect(parseAgentId).toHaveBeenCalledTimes(1);

    identity.dynamicClient();
    identity.dynamicClient();
    expect(parseAgentId).toHaveBeenCalledTimes(3);
    expect(WasmRpc.create).toHaveBeenLastCalledWith('HostType', wire, undefined, []);
    expect(identity.parsed()).toBe(semantic);
    expect(identity.parsedWire()).toBe(cachedWire);
  });

  it('binds the recorded self identity without creating a connection and rejects a different client type', () => {
    const drop = vi.fn();
    const invoke = vi.fn(() => hostInvocationMetadata('recorded-phantom-id'));
    const rpc = { [Symbol.dispose]: drop, invoke } as unknown as WasmRpc;
    const wire = schemaValueToWit(v.record([]));
    vi.mocked(WasmRpc.bindSelf).mockReturnValue(['recorded-phantom-id', rpc]);
    vi.mocked(parseAgentId).mockReturnValue([
      'Counter',
      { graph: { typeNodes: [], defs: [], root: 0 }, value: wire },
      { highBits: 17n, lowBits: 29n },
    ]);
    const counter = defineAgent({
      name: 'Counter',
      id: {},
      methods: { touch: method({ input: {}, returns: z.void() }) },
    });
    counter.client.bindSelf().touch.trigger();
    expect(invoke).toHaveBeenCalledWith('touch', schemaValueToWit(v.record([])), undefined);
    expect(parseAgentId).toHaveBeenCalledWith('recorded-phantom-id');
    expect(drop).not.toHaveBeenCalled();
    const other = defineAgent({ name: 'Other', id: {}, methods: {} });
    expect(() => other.client.bindSelf()).toThrow(RemoteCallError);
    expect(drop).toHaveBeenCalledTimes(1);
    vi.mocked(WasmRpc.bindSelf).mockImplementationOnce(() => {
      throw { tag: 'denied', val: 'ephemeral owner' };
    });
    expect(() => counter.client.bindSelf()).toThrow(RemoteCallError);
    expect(drop).toHaveBeenCalledTimes(1);
    expect(makeAgentId).not.toHaveBeenCalled();
    expect(WasmRpc.create).not.toHaveBeenCalled();
    expect(WasmRpc).not.toHaveBeenCalled();
  });

  it('caches wire decoding on an initially unparsed identity', () => {
    const wire = schemaValueToWit(v.string('decoded'));
    vi.mocked(parseAgentId).mockReturnValue([
      'Type',
      { graph: { typeNodes: [], defs: [], root: 0 }, value: wire },
      undefined,
    ]);
    const identity = new ParsedAgentId('encoded');
    expect(identity.parsed()).toEqual(['Type', v.string('decoded'), undefined]);
    expect(identity.parsed()).toBe(identity.parsed());
    expect(identity.parsedWire()[1]).toBe(wire);
    expect(parseAgentId).toHaveBeenCalledTimes(1);
  });

  it('keeps trapping and fallible connections distinct and skips ephemeral ID creation', () => {
    resolveRemoteAgent('Ephemeral', v.record([]), undefined, [], 'ephemeral');
    resolveRemoteAgentFallibly('Ephemeral', v.record([]), undefined, [], 'ephemeral');
    expect(makeAgentId).not.toHaveBeenCalled();
    expect(WasmRpc).toHaveBeenCalledTimes(1);
    expect(WasmRpc.create).toHaveBeenCalledTimes(1);
    const cause = { tag: 'denied' as const, val: 'no access' };
    vi.mocked(WasmRpc.create).mockImplementationOnce(() => {
      throw cause;
    });
    try {
      resolveRemoteAgentFallibly('Durable', v.record([]));
      expect.fail('expected connection failure');
    } catch (error) {
      expect(error).toBeInstanceOf(RemoteCallError);
      expect((error as RemoteCallError).cause).toEqual({ tag: 'denied', details: 'no access' });
    }
  });
});

describe('self agent ID', () => {
  it('names the card grant when GOLEM_AGENT_ID is not readable', () => {
    const previous = (globalThis as { currentAgentId?: string }).currentAgentId;
    (globalThis as { currentAgentId?: string }).currentAgentId = undefined;
    try {
      expect(() => getRawSelfAgentId()).toThrow(
        new Error(
          "GOLEM_AGENT_ID is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : GOLEM_AGENT_ID",
        ),
      );
    } finally {
      (globalThis as { currentAgentId?: string }).currentAgentId = previous;
    }
  });
});
