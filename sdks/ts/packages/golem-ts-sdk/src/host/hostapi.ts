// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import {
  PromiseId,
  getPromise,
  generateIdempotencyKey as rawGenerateIdempotencyKey,
  resolveComponentId as rawResolveComponentId,
  fork as rawFork,
  ForkResult as RawForkResult,
  getSelfMetadata as rawGetSelfMetadata,
  getAgentMetadata as rawGetAgentMetadata,
  AgentMetadata as RawAgentMetadata,
  AgentId as RawAgentId,
  GetAgents as RawGetAgents,
  AgentAnyFilter,
} from 'golem:api/host@1.5.0';
import { ComponentId as RawComponentId } from 'golem:core/types@1.5.0';
import { ParsedAgentId } from '../agentId';
import { awaitPollable } from '../internal/pollableUtils';
import * as wasiEnv from 'wasi:cli/environment@0.2.3';
import { Uuid } from '../uuid';
import { ComponentId, EnvironmentId } from '../ids';

// Re-export functions (pass-through — these don't return or accept UUID-containing types)
export {
  createPromise,
  getPromise,
  completePromise,
  getOplogIndex,
  setOplogIndex,
  oplogCommit,
  markBeginOperation,
  markEndOperation,
  getOplogPersistenceLevel,
  setOplogPersistenceLevel,
  getIdempotenceMode,
  setIdempotenceMode,
  updateAgent,
  forkAgent,
  revertAgent,
  resolveAgentId,
  resolveAgentIdStrict,
} from 'golem:api/host@1.5.0';

// Re-export classes (GetAgents is wrapped below)
export { GetPromiseResult } from 'golem:api/host@1.5.0';

// Re-export types (excluding those we shadow with rich classes or redefine)
export type {
  ValueAndType,
  PromiseId,
  OplogIndex,
  Pollable,
  ComponentRevision,
  PersistenceLevel,
  UpdateMode,
  FilterComparator,
  StringFilterComparator,
  AgentStatus,
  AgentNameFilter,
  AgentStatusFilter,
  AgentVersionFilter,
  AgentCreatedAtFilter,
  AgentEnvFilter,
  AgentConfigVarsFilter,
  AgentPropertyFilter,
  AgentAllFilter,
  AgentAnyFilter,
  RevertAgentTarget,
  Snapshot,
} from 'golem:api/host@1.5.0';

// Re-export rich types, shadowing raw WIT types
export { Uuid } from '../uuid';
export { ComponentId, AccountId, EnvironmentId } from '../ids';

/**
 * Represents a Golem agent, consisting of a component ID and the agent's string identifier.
 */
export type AgentId = {
  componentId: ComponentId;
  agentId: string;
};

/**
 * Metadata about an agent.
 */
export type AgentMetadata = {
  agentId: AgentId;
  args: string[];
  env: [string, string][];
  configVars: [string, string][];
  status: string;
  componentRevision: bigint;
  retryCount: bigint;
  environmentId: EnvironmentId;
};

function wrapAgentId(raw: RawAgentId): AgentId {
  return {
    componentId: ComponentId.from(raw.componentId),
    agentId: raw.agentId,
  };
}

function wrapAgentMetadata(raw: RawAgentMetadata): AgentMetadata {
  return {
    agentId: wrapAgentId(raw.agentId),
    args: raw.args,
    env: raw.env,
    configVars: raw.configVars,
    status: raw.status,
    componentRevision: raw.componentRevision,
    retryCount: raw.retryCount,
    environmentId: EnvironmentId.from(raw.environmentId),
  };
}

/**
 * Generates an idempotency key as a rich {@link Uuid}.
 */
export function generateIdempotencyKey(): Uuid {
  return Uuid.from(rawGenerateIdempotencyKey());
}

/**
 * Get the component-id for a given component reference.
 * Returns undefined when no component with the specified reference exists.
 */
export function resolveComponentId(componentReference: string): ComponentId | undefined {
  const raw = rawResolveComponentId(componentReference);
  return raw ? ComponentId.from(raw) : undefined;
}

/**
 * Get the current agent's metadata.
 */
export function getSelfMetadata(): AgentMetadata {
  return wrapAgentMetadata(rawGetSelfMetadata());
}

/**
 * Get agent metadata.
 */
export function getAgentMetadata(agentId: RawAgentId): AgentMetadata | undefined {
  const raw = rawGetAgentMetadata(agentId);
  return raw ? wrapAgentMetadata(raw) : undefined;
}

/**
 * Agent enumeration with enriched metadata.
 */
export class GetAgents {
  private readonly inner: RawGetAgents;

  constructor(componentId: RawComponentId, filter: AgentAnyFilter | undefined, precise: boolean) {
    this.inner = new RawGetAgents(componentId, filter, precise);
  }

  getNext(): AgentMetadata[] | undefined {
    const raw = this.inner.getNext();
    return raw ? raw.map(wrapAgentMetadata) : undefined;
  }
}

/**
 * Details about the fork result.
 */
export type ForkDetails = {
  forkedPhantomId: Uuid;
};

/**
 * Indicates which agent the code is running on after `fork`.
 */
export type ForkResult =
  | { tag: 'original'; val: ForkDetails }
  | { tag: 'forked'; val: ForkDetails };

/**
 * Forks the current agent. Returns enriched ForkResult with rich Uuid phantom IDs.
 */
export function fork(): ForkResult {
  const raw: RawForkResult = rawFork();
  return {
    tag: raw.tag,
    val: { forkedPhantomId: Uuid.from(raw.val.forkedPhantomId) },
  };
}

export async function awaitPromise(promiseId: PromiseId): Promise<Uint8Array> {
  const promise = getPromise(promiseId);
  await promise.subscribe().promise();
  return promise.get()!;
}

/**
 * Awaits a Golem promise with abort support. When the signal is aborted,
 * the returned promise rejects, releasing the caller from waiting.
 *
 * **Note:** Aborting only cancels the local wait — the promise may still
 * be completed on the server side.
 */
export async function awaitAbortablePromise(
  promiseId: PromiseId,
  signal: AbortSignal,
): Promise<Uint8Array> {
  const promise = getPromise(promiseId);
  await awaitPollable(promise.subscribe(), signal);
  return promise.get()!;
}

/**
 * Returns the raw string agent ID of the current agent.
 */
export function getRawSelfAgentId(): ParsedAgentId {
  const env = wasiEnv.getEnvironment();
  const agentId: [string, string] | undefined = env.find(([key]) => key === 'GOLEM_AGENT_ID');
  if (!agentId) {
    throw new Error('GOLEM_AGENT_ID is not set');
  }
  return new ParsedAgentId(agentId[1]);
}
