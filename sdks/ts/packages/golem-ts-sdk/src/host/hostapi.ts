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
  AgentPropertyFilter,
  FilterComparator,
  StringFilterComparator,
  AgentStatus,
  RevertAgentTarget,
  OplogIndex,
} from 'golem:api/host@1.5.0';
import { ComponentId as RawComponentId } from 'golem:core/types@2.0.0';
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
  trap,
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
  config: [string, string][];
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
    config: raw.config,
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

// ---------------------------------------------------------------------------
// Filter DSL for `getAgents`
// ---------------------------------------------------------------------------

type FilterNode =
  | { readonly kind: 'leaf'; readonly node: AgentPropertyFilter }
  | { readonly kind: 'all'; readonly children: readonly FilterNode[] }
  | { readonly kind: 'any'; readonly children: readonly FilterNode[] };

/**
 * Immutable filter for {@link getAgents}. Build leaves with the static
 * constructors and compose them with `.and(...)` (intersection) / `.or(...)`
 * (union); {@link toRaw} compiles to the WIT `agent-any-filter`.
 *
 * @example
 * Filter.status('equal', 'idle').and(Filter.name('starts-with', 'worker-'))
 */
export class Filter {
  private constructor(private readonly node: FilterNode) {}

  /** Match by agent name. */
  static name(comparator: StringFilterComparator, value: string): Filter {
    return Filter.leaf({ tag: 'name', val: { comparator, value } });
  }
  /** Match by agent status. */
  static status(comparator: FilterComparator, value: AgentStatus): Filter {
    return Filter.leaf({ tag: 'status', val: { comparator, value } });
  }
  /** Match by component version. */
  static version(comparator: FilterComparator, value: bigint): Filter {
    return Filter.leaf({ tag: 'version', val: { comparator, value } });
  }
  /** Match by creation time (epoch nanos). */
  static createdAt(comparator: FilterComparator, value: bigint): Filter {
    return Filter.leaf({ tag: 'created-at', val: { comparator, value } });
  }
  /** Match by env-var key/value. */
  static env(name: string, comparator: StringFilterComparator, value: string): Filter {
    return Filter.leaf({ tag: 'env', val: { name, comparator, value } });
  }
  /** Match by config-var key/value. */
  static config(name: string, comparator: StringFilterComparator, value: string): Filter {
    return Filter.leaf({ tag: 'config', val: { name, comparator, value } });
  }

  private static leaf(node: AgentPropertyFilter): Filter {
    return new Filter({ kind: 'leaf', node });
  }

  /** Intersection — both must match. */
  and(other: Filter): Filter {
    return new Filter({
      kind: 'all',
      children:
        this.node.kind === 'all' ? [...this.node.children, other.node] : [this.node, other.node],
    });
  }
  /** Union — either may match. */
  or(other: Filter): Filter {
    return new Filter({
      kind: 'any',
      children:
        this.node.kind === 'any' ? [...this.node.children, other.node] : [this.node, other.node],
    });
  }

  /** Compile to the WIT `agent-any-filter` (disjunctive normal form: OR of ANDs). */
  toRaw(): AgentAnyFilter {
    return { filters: toDnf(this.node).map((conj) => ({ filters: conj })) };
  }
}

function toDnf(node: FilterNode): AgentPropertyFilter[][] {
  switch (node.kind) {
    case 'leaf':
      return [[node.node]];
    case 'any': {
      const out: AgentPropertyFilter[][] = [];
      for (const child of node.children) for (const conj of toDnf(child)) out.push(conj);
      return out;
    }
    case 'all': {
      let acc: AgentPropertyFilter[][] = [[]];
      for (const child of node.children) {
        const childDnf = toDnf(child);
        const next: AgentPropertyFilter[][] = [];
        for (const left of acc) for (const right of childDnf) next.push([...left, ...right]);
        acc = next;
      }
      return acc;
    }
  }
}

/**
 * Enumerate a component's agents, optionally filtered. Lazily pages through the
 * host `get-agents` resource, yielding enriched {@link AgentMetadata}. Collect
 * with `[...getAgents(id, filter)]` or iterate directly.
 */
export function* getAgents(
  componentId: ComponentId,
  filter?: Filter | AgentAnyFilter,
  precise = false,
): Generator<AgentMetadata> {
  const raw = filter instanceof Filter ? filter.toRaw() : filter;
  const pager = new GetAgents(componentId, raw, precise);
  let page = pager.getNext();
  while (page !== undefined) {
    yield* page;
    page = pager.getNext();
  }
}

/** Builders for the `revert-agent` target (see {@link revertAgent}). */
export const RevertTarget = {
  /** Revert to a specific oplog index (kept as the last retained entry). */
  toOplogIndex(index: OplogIndex): RevertAgentTarget {
    return { tag: 'revert-to-oplog-index', val: index };
  },
  /** Revert the last N invocations. */
  lastInvocations(n: number | bigint): RevertAgentTarget {
    return { tag: 'revert-last-invocations', val: BigInt(n) };
  },
} as const;

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
