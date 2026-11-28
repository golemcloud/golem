// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1

import { getAgentTypeByAgentId, type Datetime } from 'golem:agent/host@2.0.0';
import type { InvocationMetadata } from './invocationId';
import {
  getRemoteAgentById,
  RemoteCallError,
  type Invocation,
  type RemoteAgentHandle,
} from './bridge/agent';
import { type SchemaValue } from './internal/schema-model';
import { AgentId } from './agentId';
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
  /**
   * Triggers the method without awaiting its result, and resolves to the invocation's handle
   * once the target has durably accepted it. Never sends a scope card.
   */
  triggerValue(input: SchemaValue): Promise<Invocation<SchemaValue | undefined>>;
  /**
   * Schedules the method for `at`, and resolves to the invocation's handle once the target has
   * durably accepted it. Never sends a scope card.
   */
  scheduleValue(at: Datetime, input: SchemaValue): Promise<Invocation<SchemaValue | undefined>>;
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
    const registered = getAgentTypeByAgentId(AgentId.toWit(agentId));
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

  triggerValue(input: SchemaValue): Promise<Invocation<SchemaValue | undefined>> {
    return this.remote.trigger(this.name, input);
  }

  scheduleValue(at: Datetime, input: SchemaValue): Promise<Invocation<SchemaValue | undefined>> {
    return this.remote.schedule(at, this.name, input);
  }
}
