// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import {
  parseAgentId,
  type CancelableScheduledInvocationReceipt,
  type Datetime,
  type InvocationMetadata,
} from 'golem:agent/host@2.0.0';
import { resolveRemoteAgentFallibly, type RemoteAgentHandle } from './bridge/agent';
import { schemaValueFromWit, type SchemaValue } from './internal/schema-model';
import type { AgentId } from './agentId';
import { Uuid } from './uuid';

export interface DynamicInvocation<T> {
  readonly metadata: InvocationMetadata;
  readonly value?: T;
}

export interface DynamicAgentClientSurface {
  readonly agentId: AgentId;
  method(name: string): DynamicAgentMethodSurface;
}

export interface DynamicAgentMethodSurface {
  readonly name: string;
  invokeValue(input: SchemaValue, signal?: AbortSignal): Promise<DynamicInvocation<SchemaValue>>;
  triggerValue(input: SchemaValue): InvocationMetadata;
  scheduleValue(at: Datetime, input: SchemaValue): CancelableScheduledInvocationReceipt;
}

export class DynamicAgentClient implements DynamicAgentClientSurface {
  readonly agentId: AgentId;
  private readonly remote: RemoteAgentHandle;

  constructor(agentId: AgentId) {
    this.agentId = agentId;
    const [typeName, constructorValue, phantomId] = parseAgentId(agentId.agentId);
    this.remote = resolveRemoteAgentFallibly(
      typeName,
      schemaValueFromWit(constructorValue.value),
      phantomId === undefined ? undefined : Uuid.from(phantomId),
    );
  }

  method(name: string): DynamicAgentMethod {
    return new DynamicAgentMethod(name, this.remote);
  }
}

export class DynamicAgentMethod implements DynamicAgentMethodSurface {
  constructor(
    public readonly name: string,
    private readonly remote: RemoteAgentHandle,
  ) {}

  invokeValue(input: SchemaValue, signal?: AbortSignal): Promise<DynamicInvocation<SchemaValue>> {
    return this.remote.invokeAndAwaitWithMetadata(this.name, input, signal);
  }

  triggerValue(input: SchemaValue): InvocationMetadata {
    return this.remote.invokeWithMetadata(this.name, input);
  }

  scheduleValue(at: Datetime, input: SchemaValue): CancelableScheduledInvocationReceipt {
    return this.remote.scheduleCancelableWithMetadata(at, this.name, input);
  }
}
