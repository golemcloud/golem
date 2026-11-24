// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { getAgentTypeByAgentId, type Datetime } from 'golem:agent/host@2.0.0';
import type { CancelableScheduledInvocationReceipt, InvocationMetadata } from './invocationId';
import { getRemoteAgentById, RemoteCallError, type RemoteAgentHandle } from './bridge/agent';
import { type SchemaValue } from './internal/schema-model';
import type { AgentId } from './agentId';
import type { ScopeCard } from './host/permissions';

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
  /** Invoke and await. A `scopeCard` is borrowed for the call. */
  invokeValue(
    input: SchemaValue,
    signal?: AbortSignal,
    scopeCard?: ScopeCard,
  ): Promise<DynamicInvocation<SchemaValue>>;
  triggerValue(input: SchemaValue): InvocationMetadata;
  scheduleValue(at: Datetime, input: SchemaValue): CancelableScheduledInvocationReceipt;
}

/** Invokes an existing durable agent with schema values and no typed client definition. */
export class DynamicAgentClient implements DynamicAgentClientSurface {
  private constructor(
    readonly agentId: AgentId,
    private readonly remote: RemoteAgentHandle,
  ) {}

  /**
   * Connects to the existing agent with this ID; its type comes from the ID's type tag. Never
   * creates the agent: an ID whose tag names no deployed agent type, or an unknown agent, throws
   * a `not-found` {@link RemoteCallError}.
   */
  static getById(agentId: AgentId): DynamicAgentClient {
    const registered = getAgentTypeByAgentId(agentId);
    if (registered === undefined) {
      throw new RemoteCallError(`Failed to connect to remote agent ${agentId.toString()}`, {
        tag: 'not-found',
        details: `no deployed agent type has the type tag of agent ${agentId.toString()}`,
      });
    }
    return new DynamicAgentClient(
      agentId,
      getRemoteAgentById(registered.agentType.typeName, agentId),
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

  invokeValue(
    input: SchemaValue,
    signal?: AbortSignal,
    scopeCard?: ScopeCard,
  ): Promise<DynamicInvocation<SchemaValue>> {
    return this.remote.invokeAndAwaitWithMetadata(this.name, input, signal, scopeCard);
  }

  triggerValue(input: SchemaValue): InvocationMetadata {
    return this.remote.invokeWithMetadata(this.name, input);
  }

  scheduleValue(at: Datetime, input: SchemaValue): CancelableScheduledInvocationReceipt {
    return this.remote.scheduleCancelableWithMetadata(at, this.name, input);
  }
}
