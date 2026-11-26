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
  type Datetime,
  type RegisteredAgentType,
} from 'golem:agent/host@2.0.0';
import type { InvocationId, InvocationMetadata } from './invocationId';
export {
  DynamicToolClient,
  ToolCommand,
  ToolRemoteOutputError,
  ToolType,
  getAllToolTypes,
  getToolType,
  type ReflectedToolClient,
  type ReflectedToolFailure,
  type ToolArgument,
} from './toolReflection';
import type {
  AgentMethod as HostAgentMethod,
  InputSchema as HostInputSchema,
  OutputSchema as HostOutputSchema,
} from 'golem:agent/common@2.0.0';
import type { SchemaGraph as WitSchemaGraph } from 'golem:core/types@2.0.0';
import {
  invocationValueById,
  mapInvocation,
  RemoteOutputError,
  streamingMethodNotAwaitedError,
  upsertRemoteAgent,
  createRemoteAgent,
  getRemoteAgentById,
  type AgentConfigEntry,
  type Invocation,
  type RemoteAgentHandle,
} from './bridge/agent';
import {
  field,
  freezeSchemaGraph,
  schemaContainsStream,
  schemaShapesMatch,
  schemaGraphRootsFromWit,
  t,
  type SchemaGraph,
  type SchemaType,
  type SchemaValue,
} from './internal/schema-model';
import { SchemaRef, type JsonValue } from './schema/ref';
import { AgentId } from './agentId';
import type { ScopeCard } from './host/permissions';
import type { RemoteClientOptions } from './client';
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

/** Canonical JSON values for declared local configuration paths. */
export interface ReflectedConfigJsonEntry {
  readonly path: readonly string[];
  readonly value: JsonValue;
}

export class AgentMethod {
  readonly name: string;
  readonly description: string;
  readonly promptHint?: string;
  readonly input: SchemaRef;
  readonly output?: SchemaRef;

  constructor(raw: HostAgentMethod, graph: ReflectedGraph) {
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
  readonly constructorInput: SchemaRef;
  readonly methods: readonly AgentMethod[];
  readonly config: readonly {
    readonly path: readonly string[];
    readonly source: 'local' | 'secret';
    readonly schema: SchemaRef;
  }[];
  readonly client: ReflectedAgentClientFactory;

  constructor(registered: RegisteredAgentType) {
    const raw = registered.agentType;
    const graph = decodeReflectedGraph(raw.schema);
    this.name = raw.typeName;
    this.description = raw.description;
    this.sourceLanguage = raw.sourceLanguage;
    this.mode = raw.mode;
    this.constructorInput = inputSchemaRef(graph, raw.constructor.inputSchema);
    this.methods = Object.freeze(raw.methods.map((method) => new AgentMethod(method, graph)));
    this.config = Object.freeze(
      raw.config.map((decl) =>
        Object.freeze({
          path: Object.freeze([...decl.path]),
          source: decl.source,
          schema: SchemaRef.fromImmutableGraph(graph.graph, reflectedTypeAt(graph, decl.valueType)),
        }),
      ),
    );
    this.client = new ReflectedAgentClientFactory(this);
    Object.freeze(this);
  }

  method(name: string): AgentMethod | undefined {
    return this.methods.find((method) => method.name === name);
  }

  /** Pack canonical JSON overrides for declared local config fields. */
  packConfigJson(entries: readonly ReflectedConfigJsonEntry[]): AgentConfigEntry[] {
    return entries.map((entry) => {
      const declaration = this.configDeclaration(entry.path);
      return {
        path: [...entry.path],
        value: {
          graph: declaration.schema.graph,
          value: declaration.schema.packJson(entry.value),
        },
      };
    });
  }

  /** Validate schema-native overrides before opening an RPC. */
  validateConfigValues(entries: readonly AgentConfigEntry[]): AgentConfigEntry[] {
    return entries.map((entry) => {
      const declaration = this.configDeclaration(entry.path);
      if (!schemaShapesMatch(declaration.schema.graph, entry.value.graph)) {
        throw new TypeError(`Incompatible config schema at '${entry.path.join('.')}'`);
      }
      if (!declaration.schema.validateValue(entry.value.value).success) {
        throw new TypeError(`Invalid config value at '${entry.path.join('.')}'`);
      }
      return {
        path: [...entry.path],
        value: {
          graph: declaration.schema.graph,
          value: entry.value.value,
        },
      };
    });
  }

  private configDeclaration(path: readonly string[]): (typeof this.config)[number] {
    const declaration = this.config.find(
      (candidate) =>
        candidate.path.length === path.length &&
        candidate.path.every((part, i) => part === path[i]),
    );
    if (!declaration) throw new TypeError(`Unknown config path '${path.join('.')}'`);
    if (declaration.source === 'secret') {
      throw new TypeError(`Cannot override secret config field '${path.join('.')}' over RPC`);
    }
    return declaration;
  }
}

/** Options of {@link ReflectedAgentClientFactory.create} with canonical JSON config values. */
export interface ReflectedCreateOptions extends RemoteClientOptions {
  /**
   * Creates the keyed agent of the constructor parameters, and rejects with a `conflict`
   * `RemoteCallError` when it already exists. Otherwise the new agent has no key and a new ID.
   */
  readonly keyed?: boolean;
  /** Overrides of declared local configuration values. */
  readonly config?: readonly ReflectedConfigJsonEntry[];
  /** Environment variables added to the ones the manifest defines. */
  readonly env?: Readonly<Record<string, string>>;
}

/** Options of {@link ReflectedAgentClientFactory.createValue} with schema-native config values. */
export interface ReflectedCreateValueOptions extends RemoteClientOptions {
  readonly keyed?: boolean;
  readonly config?: readonly AgentConfigEntry[];
  readonly env?: Readonly<Record<string, string>>;
}

export class ReflectedAgentClientFactory {
  constructor(private readonly agentType: AgentType) {}

  /**
   * Address the keyed agent of the constructor parameters, and create a durable one when it is
   * missing. For an ephemeral type, each invocation runs on a new one-shot instance.
   */
  upsert(input: JsonValue, options?: RemoteClientOptions): ReflectedAgentClient {
    return this.upsertValue(this.agentType.constructorInput.packJson(input), options);
  }

  upsertValue(input: SchemaValue, options?: RemoteClientOptions): ReflectedAgentClient {
    this.validateConstructorValue(input);
    return new ReflectedAgentClient(
      this.agentType,
      upsertRemoteAgent(this.agentType.name, input),
      options?.scopeCard,
    );
  }

  /**
   * Create a durable agent, and resolve after its initialization. With `keyed`, an existing
   * keyed agent of these constructor parameters rejects with a `conflict` `RemoteCallError`.
   */
  async create(
    input: JsonValue,
    options: ReflectedCreateOptions = {},
  ): Promise<ReflectedAgentClient> {
    return this.createValue(this.agentType.constructorInput.packJson(input), {
      ...options,
      config: this.agentType.packConfigJson(options.config ?? []),
    });
  }

  async createValue(
    input: SchemaValue,
    options: ReflectedCreateValueOptions = {},
  ): Promise<ReflectedAgentClient> {
    this.requireMode('durable', 'create');
    this.validateConstructorValue(input);
    const remote = await createRemoteAgent(this.agentType.name, input, {
      keyed: options.keyed ?? false,
      config: this.agentType.validateConfigValues(options.config ?? []),
      env: Object.entries(options.env ?? {}),
    });
    return new ReflectedAgentClient(this.agentType, remote, options.scopeCard);
  }

  /**
   * Address the existing durable agent with this ID. Never creates the agent: an unknown ID, or
   * the ID of an agent of another type, throws a `not-found` `RemoteCallError`.
   */
  getById(agentId: AgentId, options?: RemoteClientOptions): ReflectedAgentClient {
    this.requireMode('durable', 'getById');
    return new ReflectedAgentClient(
      this.agentType,
      getRemoteAgentById(this.agentType.name, agentId),
      options?.scopeCard,
    );
  }

  private validateConstructorValue(input: SchemaValue): void {
    if (!this.agentType.constructorInput.validateValue(input).success) {
      throw new TypeError(
        `Invalid constructor value for reflected agent type '${this.agentType.name}'`,
      );
    }
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
    private readonly scopeCard?: ScopeCard,
  ) {}

  /**
   * The ID of the durable agent the client connects to. On a client of `upsert` it creates the
   * agent when it is missing, without invoking a method.
   */
  agentId(): Promise<AgentId> {
    if (this.agentType.mode === 'ephemeral') {
      throw new TypeError('agentId is not available for ephemeral agent types');
    }
    return this.remote.agentId();
  }

  method(name: string): ReflectedAgentMethod {
    const method = this.agentType.method(name);
    if (!method) throw new TypeError(`Agent type '${this.agentType.name}' has no method '${name}'`);
    return new ReflectedAgentMethod(method, this.remote, this.scopeCard);
  }
}

/**
 * One method of a reflected agent. The awaited calls borrow `scopeCard` when it
 * is given, and otherwise the client's default scope card; `null` sends none.
 */
export class ReflectedAgentMethod {
  constructor(
    public readonly definition: AgentMethod,
    private readonly remote: RemoteAgentHandle,
    private readonly defaultScopeCard?: ScopeCard,
  ) {}

  invoke(
    input: JsonValue,
    signal?: AbortSignal,
    scopeCard?: ScopeCard | null,
  ): Promise<ReflectedInvocation<JsonValue>> {
    return this.invokeJson(input, signal, scopeCard);
  }

  async invokeJson(
    input: JsonValue,
    signal?: AbortSignal,
    scopeCard?: ScopeCard | null,
  ): Promise<ReflectedInvocation<JsonValue>> {
    const result = await this.invokeValue(this.definition.input.packJson(input), signal, scopeCard);
    try {
      return {
        metadata: result.metadata,
        value:
          result.value === undefined ? undefined : this.definition.output?.unpackJson(result.value),
      };
    } catch (error) {
      throw new RemoteOutputError(
        `Remote agent ${this.remote.target}.${this.definition.name} returned invalid canonical JSON`,
        { cause: error },
      );
    }
  }

  async invokeValue(
    input: SchemaValue,
    signal?: AbortSignal,
    scopeCard?: ScopeCard | null,
  ): Promise<ReflectedInvocation<SchemaValue>> {
    this.checkInput(input);
    const result = await this.remote.invokeAndAwaitWithMetadata(
      this.definition.name,
      input,
      signal,
      scopeCard === null ? undefined : (scopeCard ?? this.defaultScopeCard),
    );
    this.checkOutput(result.value);
    return result;
  }

  /**
   * Triggers the method without awaiting its result, and resolves to the invocation's handle once
   * the target has durably accepted it. Never sends a scope card, and rejects when the method's
   * input or output contains streams.
   */
  async trigger(input: JsonValue): Promise<Invocation<JsonValue | undefined>> {
    return this.jsonInvocation(await this.triggerValue(this.definition.input.packJson(input)));
  }

  async triggerValue(input: SchemaValue): Promise<Invocation<SchemaValue | undefined>> {
    this.rejectStreaming();
    this.checkInput(input);
    return this.checkedInvocation(await this.remote.trigger(this.definition.name, input));
  }

  /**
   * Schedules the method for `at`, and resolves to the invocation's handle once the target has
   * durably accepted it. Never sends a scope card, and rejects when the method's input or output
   * contains streams.
   */
  async schedule(at: Datetime, input: JsonValue): Promise<Invocation<JsonValue | undefined>> {
    return this.jsonInvocation(await this.scheduleValue(at, this.definition.input.packJson(input)));
  }

  async scheduleValue(
    at: Datetime,
    input: SchemaValue,
  ): Promise<Invocation<SchemaValue | undefined>> {
    this.rejectStreaming();
    this.checkInput(input);
    return this.checkedInvocation(await this.remote.schedule(at, this.definition.name, input));
  }

  /**
   * Rebuilds the handle of an invocation of this method from its ID. Makes no remote call. The
   * result is checked against the method's output schema and decoded as canonical JSON.
   */
  invocation(id: InvocationId): Invocation<JsonValue | undefined> {
    return this.jsonInvocation(this.invocationValue(id));
  }

  invocationValue(id: InvocationId): Invocation<SchemaValue | undefined> {
    return this.checkedInvocation(invocationValueById(id));
  }

  private rejectStreaming(): void {
    const { input, output } = this.definition;
    if (
      schemaContainsStream(input.graph, input.root) ||
      (output !== undefined && schemaContainsStream(output.graph, output.root))
    ) {
      throw streamingMethodNotAwaitedError(
        `Remote agent ${this.remote.target}.${this.definition.name}`,
      );
    }
  }

  private checkInput(input: SchemaValue): void {
    if (!this.definition.input.validateValue(input).success) {
      throw new TypeError(`Invalid input for reflected method '${this.definition.name}'`);
    }
  }

  private checkOutput(value: SchemaValue | undefined): void {
    if (this.definition.output !== undefined && value === undefined) {
      throw new RemoteOutputError(
        `Remote agent ${this.remote.target}.${this.definition.name} returned no value for a non-unit output`,
      );
    }
    if (this.definition.output === undefined && value !== undefined) {
      throw new RemoteOutputError(
        `Remote agent ${this.remote.target}.${this.definition.name} returned a value for a unit output`,
      );
    }
    if (this.definition.output !== undefined && value !== undefined) {
      const validation = this.definition.output.validateValue(value);
      if (!validation.success) {
        throw new RemoteOutputError(
          `Remote agent ${this.remote.target}.${this.definition.name} returned an invalid output: ${validation.issues.map((issue) => issue.message).join('; ')}`,
        );
      }
    }
  }

  private checkedInvocation(
    invocation: Invocation<SchemaValue | undefined>,
  ): Invocation<SchemaValue | undefined> {
    return mapInvocation(invocation, (value) => {
      this.checkOutput(value);
      return value;
    });
  }

  private jsonInvocation(
    invocation: Invocation<SchemaValue | undefined>,
  ): Invocation<JsonValue | undefined> {
    return mapInvocation(invocation, (value) => {
      try {
        return value === undefined ? undefined : this.definition.output?.unpackJson(value);
      } catch (error) {
        throw new RemoteOutputError(
          `Remote agent ${this.remote.target}.${this.definition.name} returned invalid canonical JSON`,
          { cause: error },
        );
      }
    });
  }
}

export function getAllAgentTypes(): readonly AgentType[] {
  return Object.freeze(
    hostGetAllAgentTypes()
      .filter((registered) => registered.agentType.kind === 'regular')
      .map((registered) => new AgentType(registered)),
  );
}

/**
 * Look up a deployed agent type by name.
 *
 * This is an optional discovery operation: it returns `undefined` for routers or types not visible in
 * the current environment. Once returned, each schema operation is strict and throws for malformed
 * JSON or schema values.
 */
export function getAgentType(name: string): AgentType | undefined {
  const registered = hostGetAgentType(name);
  return registered?.agentType.kind === 'regular' ? new AgentType(registered) : undefined;
}

/**
 * Look up the deployed agent type that the type tag of an agent ID names.
 *
 * Returns `undefined` for routers, and when no deployed agent type has that tag. It does not check
 * that the agent exists.
 */
export function getAgentTypeByAgentId(agentId: AgentId): AgentType | undefined {
  const registered = hostGetAgentTypeByAgentId(AgentId.toWit(agentId));
  return registered?.agentType.kind === 'regular' ? new AgentType(registered) : undefined;
}

interface ReflectedGraph {
  readonly graph: SchemaGraph;
  readonly types: readonly SchemaType[];
}

function decodeReflectedGraph(graph: WitSchemaGraph): ReflectedGraph {
  const indices = graph.typeNodes.map((_, index) => index);
  const decoded = schemaGraphRootsFromWit(graph, indices);
  const shared = freezeSchemaGraph({ defs: decoded.defs, root: t.tuple([...decoded.roots]) });
  return { graph: shared, types: decoded.roots };
}

function inputSchemaRef(graph: ReflectedGraph, input: HostInputSchema): SchemaRef {
  const fields = input.val
    .filter((entry) => entry.source.tag === 'user-supplied')
    .map((entry) => field(entry.name, reflectedTypeAt(graph, entry.schema), entry.metadata));
  return SchemaRef.fromImmutableGraph(graph.graph, t.record(fields));
}

function outputSchemaRef(graph: ReflectedGraph, output: HostOutputSchema): SchemaRef | undefined {
  if (output.tag === 'unit') return undefined;
  return SchemaRef.fromImmutableGraph(graph.graph, reflectedTypeAt(graph, output.val));
}

function reflectedTypeAt(graph: ReflectedGraph, index: number): SchemaType {
  const type = graph.types[index];
  if (type === undefined) {
    throw new TypeError(`reflected schema type node index out of range: ${index}`);
  }
  return type;
}
