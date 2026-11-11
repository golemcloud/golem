// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { makeAgentId, parseAgentId, WasmRpc } from 'golem:agent/host@2.0.0';
import { ParsedAgentId } from '../src/agentId';
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
