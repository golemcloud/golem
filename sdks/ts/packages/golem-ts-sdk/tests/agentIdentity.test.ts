// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { getAgentTypeByAgentId, WasmRpc } from 'golem:agent/host@2.0.0';
import { AgentId } from '../src/agentId';
import { DynamicAgentClient } from '../src/dynamicClient';
import { defineAgent } from '../src/defineAgent';
import { method } from '../src/method';
import { z } from 'zod';
import { hostInvocationMetadata } from './invocationMetadataTestSupport';
import { getSelfAgentId } from '../src/host/hostapi';
import { MOCK_REMOTE_AGENT_ID } from './testSetup';
import {
  createRemoteAgent,
  getRemoteAgentById,
  RemoteCallError,
  upsertRemoteAgent,
} from '../src/bridge/agent';
import { schemaValueToWit, typedSchemaValueToWit, v } from '../src/internal/schema-model';
import { compileSchema } from '../src/schema/adapter';
import { selectSelfAgent, SELF_AGENT_ID } from './selfAgentTestSupport';

const targetId = AgentId.parse('00000007-0000-8000-8000-000000000004');

describe('identity boundaries', () => {
  beforeEach(() => vi.clearAllMocks());

  it('binds the recorded self identity without creating a connection and rejects a different client type', () => {
    const drop = vi.fn();
    const invoke = vi.fn(() => hostInvocationMetadata());
    const rpc = { [Symbol.dispose]: drop, invoke } as unknown as WasmRpc;
    selectSelfAgent('Counter');
    vi.mocked(WasmRpc.bindSelf).mockReturnValue([AgentId.parse(SELF_AGENT_ID), rpc]);
    const counter = defineAgent({
      name: 'Counter',
      id: {},
      methods: { touch: method({ input: {}, returns: z.void() }) },
    });
    counter.client.bindSelf().touch.trigger();
    expect(invoke).toHaveBeenCalledWith('touch', schemaValueToWit(v.record([])), undefined);
    expect(drop).not.toHaveBeenCalled();
    const other = defineAgent({ name: 'Other', id: {}, methods: {} });
    expect(() => other.client.bindSelf()).toThrow(RemoteCallError);
    expect(drop).toHaveBeenCalledTimes(1);
    vi.mocked(WasmRpc.bindSelf).mockImplementationOnce(() => {
      throw { tag: 'denied', val: 'ephemeral owner' };
    });
    expect(() => counter.client.bindSelf()).toThrow(RemoteCallError);
    expect(drop).toHaveBeenCalledTimes(1);
    expect(WasmRpc.upsert).not.toHaveBeenCalled();
    expect(WasmRpc.create).not.toHaveBeenCalled();
    expect(WasmRpc.getById).not.toHaveBeenCalled();
  });

  it('upserts through the host upsert only', () => {
    const remote = upsertRemoteAgent('Durable', v.record([v.string('a')]));
    expect(WasmRpc.upsert).toHaveBeenCalledWith(
      'Durable',
      schemaValueToWit(v.record([v.string('a')])),
    );
    expect(WasmRpc.create).not.toHaveBeenCalled();
    expect(WasmRpc.getById).not.toHaveBeenCalled();
    expect(remote.target).toBe('Durable');
  });

  it('maps keyed, config and env of a create to the host create options', async () => {
    const configGraph = compileSchema(z.string()).graph;
    const remote = await createRemoteAgent('Durable', v.record([]), {
      keyed: true,
      config: [{ path: ['model'], value: { graph: configGraph, value: v.string('m') } }],
      env: [['REGION', 'eu']],
    });
    expect(WasmRpc.create).toHaveBeenCalledWith('Durable', schemaValueToWit(v.record([])), {
      keyed: true,
      agentConfig: [
        {
          path: ['model'],
          value: typedSchemaValueToWit({ graph: configGraph, value: v.string('m') }),
        },
      ],
      env: [['REGION', 'eu']],
    });
    expect(remote.target).toBe(`Durable[${AgentId.from(MOCK_REMOTE_AGENT_ID)}]`);
    await createRemoteAgent('Durable', v.record([]));
    expect(vi.mocked(WasmRpc.create).mock.calls.at(-1)![2]).toEqual({
      keyed: false,
      agentConfig: [],
      env: [],
    });
  });

  it('reports a conflict of a keyed create as a typed RemoteCallError', async () => {
    vi.mocked(WasmRpc.create).mockRejectedValueOnce({ tag: 'conflict', val: 'key is current' });
    const error = await createRemoteAgent('Durable', v.record([]), { keyed: true }).catch(
      (error: unknown) => error,
    );
    expect(error).toBeInstanceOf(RemoteCallError);
    expect((error as RemoteCallError).cause).toEqual({
      tag: 'conflict',
      details: 'key is current',
    });
    expect((error as RemoteCallError).message).toContain('conflict: key is current');
  });

  it('gets by ID through the host get-by-id and never creates', async () => {
    await getRemoteAgentById('Durable', targetId);
    expect(WasmRpc.getById).toHaveBeenCalledWith('Durable', targetId);
    expect(WasmRpc.upsert).not.toHaveBeenCalled();
    expect(WasmRpc.create).not.toHaveBeenCalled();

    vi.mocked(WasmRpc.getById).mockImplementationOnce(() => {
      throw { tag: 'not-found', val: 'no agent' };
    });
    const missing = getRemoteAgentById('Durable', targetId);
    await expect(missing).rejects.toBeInstanceOf(RemoteCallError);
    await expect(missing).rejects.toMatchObject({
      cause: { tag: 'not-found', details: 'no agent' },
    });
  });

  it('reads the agent ID of a handle from the host agent-id method', async () => {
    const remote = upsertRemoteAgent('Durable', v.record([]));
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value;
    const agentId = await remote.agentId();
    expect(rpc.agentId).toHaveBeenCalledTimes(1);
    expect(agentId).toBeInstanceOf(AgentId);
    expect(agentId.equals(MOCK_REMOTE_AGENT_ID)).toBe(true);
  });

  it('connects a dynamic client by ID to the agent type that the ID tag names', async () => {
    vi.mocked(getAgentTypeByAgentId).mockReturnValueOnce({
      agentType: { typeName: 'Dynamic', mode: 'durable' },
    } as never);
    const client = await DynamicAgentClient.getById(targetId);
    expect(client.agentId).toBe(targetId);
    expect(WasmRpc.getById).toHaveBeenCalledWith('Dynamic', targetId);

    vi.mocked(getAgentTypeByAgentId).mockReturnValueOnce(undefined);
    await expect(DynamicAgentClient.getById(targetId)).rejects.toMatchObject({
      name: 'RemoteCallError',
      cause: { tag: 'not-found' },
    });
  });
});

describe('self agent ID', () => {
  it('names the card grant when GOLEM_AGENT_ID is not readable', () => {
    const previous = (globalThis as { currentAgentId?: string }).currentAgentId;
    (globalThis as { currentAgentId?: string }).currentAgentId = undefined;
    try {
      expect(() => getSelfAgentId()).toThrow(
        new Error(
          "GOLEM_AGENT_ID is not readable: the agent's permission card must allow env(?agent) @ ?agent : read : GOLEM_AGENT_ID",
        ),
      );
    } finally {
      (globalThis as { currentAgentId?: string }).currentAgentId = previous;
    }
  });
});
