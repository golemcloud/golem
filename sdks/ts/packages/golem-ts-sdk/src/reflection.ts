// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import {
  getAllAgentTypes as hostGetAllAgentTypes,
  getAgentType as hostGetAgentType,
  getAgentTypeByAgentId as hostGetAgentTypeByAgentId,
  type CancelableScheduledInvocationReceipt,
  type Datetime,
  type InvocationMetadata,
  type RegisteredAgentType,
} from 'golem:agent/host@2.0.0';
import type {
  AgentMethod as HostAgentMethod,
  InputSchema as HostInputSchema,
  OutputSchema as HostOutputSchema,
} from 'golem:agent/common@2.0.0';
import type { SchemaGraph as WitSchemaGraph } from 'golem:core/types@2.0.0';
import { resolveRemoteAgentFallibly, type RemoteAgentHandle } from './bridge/agent';
import {
  field,
  schemaGraphFromWit,
  t,
  type SchemaGraph,
  type SchemaValue,
} from './internal/schema-model';
import { SchemaRef, type JsonValue } from './schema/ref';
import { Uuid } from './uuid';
import { ComponentId } from './ids';
import { AgentId, bindAgentClient } from './agentId';
export {
  DynamicAgentClient,
  DynamicAgentMethod,
  type DynamicAgentClientSurface,
  type DynamicAgentMethodSurface,
  type DynamicInvocation,
} from './dynamicClient';

export {
  isRemoteCallError,
  RemoteCallError,
  RemoteOutputError,
  type RemoteAgentError,
  type RemoteCallErrorCause,
} from './bridge/agent';

export interface ReflectedInvocation<T> {
  readonly metadata: InvocationMetadata;
  readonly value?: T;
}

export class AgentMethod {
  readonly name: string;
  readonly description: string;
  readonly promptHint?: string;
  readonly input: SchemaRef;
  readonly output?: SchemaRef;

  constructor(raw: HostAgentMethod, graph: WitSchemaGraph) {
    this.name = raw.name;
    this.description = raw.description;
    this.promptHint = raw.promptHint;
    this.input = inputSchemaRef(graph, raw.inputSchema);
    this.output = outputSchemaRef(graph, raw.outputSchema);
    Object.freeze(this);
  }
}

export class AgentType {
  readonly name: string;
  readonly description: string;
  readonly sourceLanguage: string;
  readonly mode: 'durable' | 'ephemeral';
  readonly implementedBy: RegisteredAgentType['implementedBy'];
  readonly constructorInput: SchemaRef;
  readonly methods: readonly AgentMethod[];
  readonly client: ReflectedAgentClientFactory;

  constructor(registered: RegisteredAgentType) {
    const raw = registered.agentType;
    this.name = raw.typeName;
    this.description = raw.description;
    this.sourceLanguage = raw.sourceLanguage;
    this.mode = raw.mode;
    this.implementedBy = registered.implementedBy;
    this.constructorInput = inputSchemaRef(raw.schema, raw.constructor.inputSchema);
    this.methods = Object.freeze(raw.methods.map((method) => new AgentMethod(method, raw.schema)));
    this.client = new ReflectedAgentClientFactory(this);
    Object.freeze(this);
  }

  method(name: string): AgentMethod | undefined {
    return this.methods.find((method) => method.name === name);
  }

  /** Construct an agent identity from canonical JSON constructor input. */
  agentId(input: JsonValue, phantomId?: Uuid): AgentId {
    return this.agentIdValue(this.constructorInput.packJson(input), phantomId);
  }

  /** Construct an agent identity from an already packed constructor value. */
  agentIdValue(input: SchemaValue, phantomId?: Uuid): AgentId {
    return AgentId.create({
      componentId: ComponentId.from(this.implementedBy),
      typeName: this.name,
      constructorValue: input,
      phantomId,
    });
  }

  [bindAgentClient](agentId: AgentId): ReflectedAgentClient {
    const parts = AgentId.parse(agentId);
    if (parts.typeName !== this.name) {
      throw new TypeError(`Reflected agent type '${this.name}' cannot bind '${parts.typeName}'`);
    }
    if (this.mode === 'ephemeral') {
      throw new TypeError(
        `Cannot bind existing AgentId '${agentId.agentId}' to ephemeral agent type '${this.name}'; use agentType.client.newPhantom(...)`,
      );
    }
    return new ReflectedAgentClient(
      this,
      resolveRemoteAgentFallibly(
        parts.typeName,
        parts.constructorValue,
        parts.phantomId,
        [],
        this.mode,
      ),
    );
  }
}

export interface ReflectedPhantomClient {
  readonly agentId: AgentId;
  readonly phantomId: Uuid;
  readonly client: ReflectedAgentClient;
}

export class ReflectedAgentClientFactory {
  constructor(private readonly agentType: AgentType) {}

  get(input: JsonValue): ReflectedAgentClient {
    this.requireMode('durable', 'get');
    return this.create(this.agentType.constructorInput.packJson(input));
  }

  getValue(input: SchemaValue): ReflectedAgentClient {
    this.requireMode('durable', 'getValue');
    return this.create(input);
  }

  getPhantom(input: JsonValue, phantomId: Uuid): ReflectedAgentClient {
    return this.create(this.agentType.constructorInput.packJson(input), phantomId);
  }

  getPhantomValue(input: SchemaValue, phantomId: Uuid): ReflectedAgentClient {
    return this.create(input, phantomId);
  }

  newPhantom(input: JsonValue): ReflectedPhantomClient | ReflectedAgentClient {
    return this.newPhantomValue(this.agentType.constructorInput.packJson(input));
  }

  newPhantomValue(input: SchemaValue): ReflectedPhantomClient | ReflectedAgentClient {
    if (this.agentType.mode === 'ephemeral') return this.create(input);
    const phantomId = Uuid.generate();
    const agentId = this.agentType.agentIdValue(input, phantomId);
    return {
      agentId,
      phantomId,
      client: this.create(input, phantomId),
    };
  }

  private create(input: SchemaValue, phantomId?: Uuid): ReflectedAgentClient {
    return new ReflectedAgentClient(
      this.agentType,
      resolveRemoteAgentFallibly(this.agentType.name, input, phantomId, [], this.agentType.mode),
    );
  }

  private requireMode(expected: AgentType['mode'], operation: string): void {
    if (this.agentType.mode !== expected) {
      throw new TypeError(`${operation} is not available for ${this.agentType.mode} agent types`);
    }
  }
}

export class ReflectedAgentClient {
  constructor(
    private readonly agentType: AgentType,
    private readonly remote: RemoteAgentHandle,
  ) {}

  method(name: string): ReflectedAgentMethod {
    const method = this.agentType.method(name);
    if (!method) throw new TypeError(`Agent type '${this.agentType.name}' has no method '${name}'`);
    return new ReflectedAgentMethod(method, this.remote);
  }
}

export class ReflectedAgentMethod {
  constructor(
    public readonly definition: AgentMethod,
    private readonly remote: RemoteAgentHandle,
  ) {}

  invoke(input: JsonValue, signal?: AbortSignal): Promise<ReflectedInvocation<JsonValue>> {
    return this.invokeJson(input, signal);
  }

  async invokeJson(
    input: JsonValue,
    signal?: AbortSignal,
  ): Promise<ReflectedInvocation<JsonValue>> {
    const result = await this.invokeValue(this.definition.input.packJson(input), signal);
    return {
      metadata: result.metadata,
      value:
        result.value === undefined || this.definition.output === undefined
          ? undefined
          : this.definition.output.unpackJson(result.value),
    };
  }

  async invokeValue(
    input: SchemaValue,
    signal?: AbortSignal,
  ): Promise<ReflectedInvocation<SchemaValue>> {
    return this.remote.invokeAndAwaitWithMetadata(this.definition.name, input, signal);
  }

  trigger(input: JsonValue): InvocationMetadata {
    return this.triggerValue(this.definition.input.packJson(input));
  }

  triggerValue(input: SchemaValue): InvocationMetadata {
    return this.remote.invokeWithMetadata(this.definition.name, input);
  }

  schedule(at: Datetime, input: JsonValue): CancelableScheduledInvocationReceipt {
    return this.scheduleValue(at, this.definition.input.packJson(input));
  }

  scheduleValue(at: Datetime, input: SchemaValue): CancelableScheduledInvocationReceipt {
    return this.remote.scheduleCancelableWithMetadata(at, this.definition.name, input);
  }
}

export function getAllAgentTypes(): readonly AgentType[] {
  return Object.freeze(hostGetAllAgentTypes().map((registered) => new AgentType(registered)));
}

export function getAgentType(name: string): AgentType | undefined {
  const registered = hostGetAgentType(name);
  return registered === undefined ? undefined : new AgentType(registered);
}

export function getAgentTypeByAgentId(agentId: AgentId): AgentType | undefined {
  const registered = hostGetAgentTypeByAgentId(agentId);
  return registered === undefined ? undefined : new AgentType(registered);
}

function inputSchemaRef(graph: WitSchemaGraph, input: HostInputSchema): SchemaRef {
  const decoded = schemaGraphFromWit(graph);
  const fields = input.val
    .filter((entry) => entry.source.tag === 'user-supplied')
    .map((entry) =>
      field(entry.name, schemaGraphFromWit({ ...graph, root: entry.schema }).root, entry.metadata),
    );
  const reflectedGraph: SchemaGraph = { defs: decoded.defs, root: t.record(fields) };
  return new SchemaRef(reflectedGraph);
}

function outputSchemaRef(graph: WitSchemaGraph, output: HostOutputSchema): SchemaRef | undefined {
  if (output.tag === 'unit') return undefined;
  return new SchemaRef(schemaGraphFromWit({ ...graph, root: output.val }));
}
