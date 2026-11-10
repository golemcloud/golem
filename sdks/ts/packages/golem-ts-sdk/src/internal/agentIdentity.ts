// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { makeAgentId, parseAgentId, type WasmRpc } from 'golem:agent/host@2.0.0';
import type { SchemaValueTree, Uuid as RawUuid } from 'golem:core/types@2.0.0';
import { Uuid } from '../uuid';
import {
  schemaValueFromWit,
  schemaValueToWit,
  typedSchemaValueToWit,
  type SchemaValue,
  type TypedSchemaValue,
} from './schema-model';

export function createAgentIdString(
  typeName: string,
  constructorValue: SchemaValue,
  phantomId?: RawUuid,
): string {
  const normalized = phantomId ? Uuid.from(phantomId) : undefined;
  return makeAgentId(typeName, schemaValueToWit(constructorValue), normalized);
}

export function decodeAgentIdWire(value: string): [string, SchemaValueTree, Uuid | undefined] {
  const [typeName, parameters, phantomId] = parseAgentId(value);
  return [typeName, parameters.value, phantomId ? Uuid.from(phantomId) : undefined];
}

/** Fresh decoding is distinct from the public facade's semantic and wire caches. */
export function decodeAgentId(value: string): [string, SchemaValue, Uuid | undefined] {
  const [typeName, constructorValue, phantomId] = parseAgentId(value);
  return [
    typeName,
    schemaValueFromWit(constructorValue.value),
    phantomId === undefined ? undefined : Uuid.from(phantomId),
  ];
}

export type RemoteAgentCreation = (
  agentTypeName: string,
  constructorTree: SchemaValueTree,
  phantomId: Uuid | undefined,
  config: Array<{ path: string[]; value: ReturnType<typeof typedSchemaValueToWit> }>,
  agentId: string,
) => WasmRpc;

export function connectRemoteAgent(
  agentTypeName: string,
  constructorValue: SchemaValue,
  phantomId: Uuid | undefined,
  configEntries: readonly { readonly path: readonly string[]; readonly value: TypedSchemaValue }[],
  mode: 'durable' | 'ephemeral',
  create: RemoteAgentCreation,
): { agentId: string; rpc: WasmRpc } {
  const constructorTree = schemaValueToWit(constructorValue);
  const agentId =
    mode === 'ephemeral' ? agentTypeName : makeAgentId(agentTypeName, constructorTree, phantomId);
  const rpc = create(
    agentTypeName,
    constructorTree,
    phantomId,
    configEntries.map((entry) => ({
      path: [...entry.path],
      value: typedSchemaValueToWit(entry.value),
    })),
    agentId,
  );
  return { agentId, rpc };
}
