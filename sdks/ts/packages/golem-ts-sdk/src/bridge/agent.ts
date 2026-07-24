// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { makeAgentId, WasmRpc } from 'golem:agent/host@2.0.0';
import type {
  CancellationToken,
  CancelableScheduledInvocationReceipt,
  Datetime,
  InvocationMetadata,
  ScheduledInvocationReceipt,
} from 'golem:agent/host@2.0.0';
import { awaitPollable, disposeWitResource, throwIfAborted } from '../internal/pollableUtils';
import {
  schemaValueFromWit,
  schemaValueToWit,
  typedSchemaValueToWit,
  type SchemaValue,
  type TypedSchemaValue,
} from '../internal/schema-model';
import type { Uuid } from '../uuid';

export class RemoteCallError extends Error {
  readonly _tag = 'RemoteCallError';
}
export interface RemoteInvocationResult {
  metadata: InvocationMetadata;
  value?: SchemaValue;
}
export interface AgentConfigEntry {
  readonly path: readonly string[];
  readonly value: TypedSchemaValue;
}

export interface RemoteAgentHandle {
  readonly agentId: string;
  invokeAndAwait(
    method: string,
    params: SchemaValue,
    signal?: AbortSignal,
  ): Promise<SchemaValue | undefined>;
  invokeAndAwaitWithMetadata(
    method: string,
    params: SchemaValue,
    signal?: AbortSignal,
  ): Promise<RemoteInvocationResult>;
  invoke(method: string, params: SchemaValue): void;
  invokeWithMetadata(method: string, params: SchemaValue): InvocationMetadata;
  schedule(at: Datetime, method: string, params: SchemaValue): void;
  scheduleWithMetadata(
    at: Datetime,
    method: string,
    params: SchemaValue,
  ): ScheduledInvocationReceipt;
  scheduleCancelable(at: Datetime, method: string, params: SchemaValue): CancellationToken;
  scheduleCancelableWithMetadata(
    at: Datetime,
    method: string,
    params: SchemaValue,
  ): CancelableScheduledInvocationReceipt;
}

export function resolveRemoteAgent(
  agentTypeName: string,
  constructorValue: SchemaValue,
  phantomId?: Uuid,
  configEntries: readonly AgentConfigEntry[] = [],
): RemoteAgentHandle {
  const constructorTree = schemaValueToWit(constructorValue);
  const agentId = makeAgentId(agentTypeName, constructorTree, phantomId);
  const rpc = new WasmRpc(
    agentTypeName,
    constructorTree,
    phantomId,
    configEntries.map((entry) => ({
      path: [...entry.path],
      value: typedSchemaValueToWit(entry.value),
    })),
  );
  const awaitInvocation = async (
    method: string,
    params: SchemaValue,
    signal?: AbortSignal,
  ): Promise<RemoteInvocationResult> => {
    throwIfAborted(signal);
    const invocation = rpc.asyncInvokeAndAwait(method, schemaValueToWit(params));
    const future = invocation.future;
    const onAbort = () => {
      try {
        future.cancel();
      } catch {
        /* already completed */
      }
    };
    signal?.addEventListener('abort', onAbort, { once: true });
    try {
      try {
        try {
          await awaitPollable(future.subscribe(), signal);
        } catch (error) {
          if (!signal?.aborted) {
            try {
              future.cancel();
            } catch {
              /* already completed */
            }
          }
          try {
            future.get();
          } catch {
            // Preserve the polling failure after consuming the terminal result.
          }
          throw error;
        }
      } finally {
        signal?.removeEventListener('abort', onAbort);
      }
      const result = future.get();
      if (!result) {
        try {
          future.cancel();
        } catch {
          /* already completed */
        }
        try {
          future.get();
        } catch {
          // Preserve the missing result error after consuming the terminal result.
        }
        throw new RemoteCallError(`RPC to ${agentId}.${method} failed (no result)`);
      }
      if (result.tag === 'err')
        throw new RemoteCallError(
          `Remote agent ${agentId}.${method} errored: ${JSON.stringify(result.val, (_, value) => (typeof value === 'bigint' ? value.toString() : value))}`,
        );
      try {
        return {
          metadata: invocation.metadata,
          value: result.val === undefined ? undefined : schemaValueFromWit(result.val),
        };
      } catch (error) {
        if (error instanceof RemoteCallError) throw error;
        throw new RemoteCallError(
          `Remote agent ${agentId}.${method} returned an invalid schema value: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
    } finally {
      disposeWitResource(future);
    }
  };
  return {
    agentId,
    invokeAndAwait: async (method, params, signal) =>
      (await awaitInvocation(method, params, signal)).value,
    invokeAndAwaitWithMetadata: awaitInvocation,
    invoke(method, params) {
      rpc.invoke(method, schemaValueToWit(params));
    },
    invokeWithMetadata(method, params) {
      return rpc.invoke(method, schemaValueToWit(params));
    },
    schedule(at, method, params) {
      rpc.scheduleInvocation(at, method, schemaValueToWit(params));
    },
    scheduleWithMetadata(at, method, params) {
      return rpc.scheduleInvocation(at, method, schemaValueToWit(params));
    },
    scheduleCancelable(at, method, params) {
      return rpc.scheduleCancelableInvocation(at, method, schemaValueToWit(params))
        .cancellationToken;
    },
    scheduleCancelableWithMetadata(at, method, params) {
      return rpc.scheduleCancelableInvocation(at, method, schemaValueToWit(params));
    },
  };
}
