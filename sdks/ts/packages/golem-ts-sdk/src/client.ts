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

// Wasm-RPC clients attached to agent definitions. `def.client.get(id)` returns a
// typed proxy that calls a remote agent declared with the same definition. The wire encoding
// is built from the LOCAL def's `SchemaCodec`s — the exact codecs the exported
// component uses to decode (see runtime.ts `invoke`) — so the two sides are
// symmetric by construction. Reuses the host `WasmRpc` resource (no decorator
// `Type.Type`/metadata).

import type {
  Datetime,
  CancellationToken,
  InvocationMetadata,
  CancelableScheduledInvocationReceipt,
} from 'golem:agent/host@2.0.0';
import { v } from './internal/schema-model';
import type { SchemaGraph, SchemaType, SchemaValue } from './internal/schema-model';
import { compileConfig, ConfigDeclaration } from './config';
import { Uuid } from './uuid';
import { AgentId, bindAgentClient } from './agentId';
import { getSelfMetadata } from './host/hostapi';
import { compileSchema } from './schema/adapter';
import { SchemaCodec } from './schema/codec';
import { StandardSchemaV1 } from './schema/standardSchema';
import type {
  AgentClientContract,
  AgentClientBindingDefinition,
  AgentClientDefinition,
  CallerInput,
  ConfigSpec,
  IdRecord,
  MethodsRecord,
} from './defineAgent';
import { MethodSpec } from './method';
import {
  resolveRemoteAgent,
  resolveRemoteAgentFallibly,
  RemoteOutputError,
  type AgentConfigEntry,
} from './bridge/agent';

export {
  isRemoteCallError,
  RemoteCallError,
  RemoteOutputError,
  type RemoteAgentError,
  type RemoteCallErrorCause,
} from './bridge/agent';

type InferRecord<R extends Record<string, StandardSchemaV1>> = {
  [K in keyof R]: StandardSchemaV1.InferOutput<R[K]>;
};

/** The async remote signature for a durable method spec (no-arg when caller input is empty). */
type DurableRemoteMethodFor<M> =
  M extends MethodSpec<infer Input, infer Output, boolean>
    ? keyof CallerInput<Input> extends never
      ? {
          (options?: RemoteCallOptions): Promise<Output>;
          /** Fire-and-forget; no result is awaited. */
          trigger(): void;
          /** Enqueue at `at`, returning a token to cancel it before it runs. */
          schedule(at: Datetime): CancellationToken;
        }
      : {
          (input: InferRecord<CallerInput<Input>>, options?: RemoteCallOptions): Promise<Output>;
          trigger(input: InferRecord<CallerInput<Input>>): void;
          /** Enqueue at `at`, returning a token to cancel it before it runs. */
          schedule(at: Datetime, input: InferRecord<CallerInput<Input>>): CancellationToken;
        }
    : never;

/** Result of invoking an ephemeral agent, including its per-invocation identity. */
export interface EphemeralInvocationResult<T> {
  metadata: InvocationMetadata;
  value: T;
}

/** The async remote signature for an ephemeral method spec. */
type EphemeralRemoteMethodFor<M> =
  M extends MethodSpec<infer Input, infer Output, boolean>
    ? keyof CallerInput<Input> extends never
      ? {
          (options?: RemoteCallOptions): Promise<EphemeralInvocationResult<Output>>;
          trigger(): InvocationMetadata;
          schedule(at: Datetime): CancelableScheduledInvocationReceipt;
        }
      : {
          (
            input: InferRecord<CallerInput<Input>>,
            options?: RemoteCallOptions,
          ): Promise<EphemeralInvocationResult<Output>>;
          trigger(input: InferRecord<CallerInput<Input>>): InvocationMetadata;
          schedule(
            at: Datetime,
            input: InferRecord<CallerInput<Input>>,
          ): CancelableScheduledInvocationReceipt;
        }
    : never;

/** Options for an awaited remote call. */
export interface RemoteCallOptions {
  signal?: AbortSignal;
}

/** A typed remote client: one async method per declared method on the def. */
export type RemoteClient<
  Methods extends MethodsRecord,
  Mode extends 'durable' | 'ephemeral' = 'durable',
> = {
  [K in keyof Methods]: Mode extends 'ephemeral'
    ? EphemeralRemoteMethodFor<Methods[K]>
    : DurableRemoteMethodFor<Methods[K]>;
};

/** A newly generated phantom client together with its reusable phantom id. */
export interface PhantomClientDetails<Methods extends MethodsRecord> {
  readonly client: RemoteClient<Methods>;
  readonly agentId: AgentId;
  readonly phantomId: Uuid;
}

/** Address existing agents or create a fresh phantom agent client. */
export interface RemoteClientFactory<Id extends IdRecord, Methods extends MethodsRecord> {
  /** Address the durable agent with this constructor identity. */
  get(id: InferRecord<CallerInput<Id>>, config?: Record<string, unknown>): RemoteClient<Methods>;
  /** Address a known phantom instance. */
  getPhantom(
    id: InferRecord<CallerInput<Id>>,
    phantomId: Uuid,
    config?: Record<string, unknown>,
  ): RemoteClient<Methods>;
  /** Create a client with a newly generated phantom id. */
  newPhantom(
    id: InferRecord<CallerInput<Id>>,
    config?: Record<string, unknown>,
  ): PhantomClientDetails<Methods>;
}

/** Creates logical ephemeral clients whose final identity is allocated per invocation. */
export interface EphemeralRemoteClientFactory<Id extends IdRecord, Methods extends MethodsRecord> {
  /** Address a known ephemeral phantom instance. */
  getPhantom(
    id: InferRecord<CallerInput<Id>>,
    phantomId: Uuid,
    config?: Record<string, unknown>,
  ): RemoteClient<Methods, 'ephemeral'>;
  /** Create a logical client whose final identity is returned by each invocation. */
  newPhantom(
    id: InferRecord<CallerInput<Id>>,
    config?: Record<string, unknown>,
  ): RemoteClient<Methods, 'ephemeral'>;
}

export type AgentClientFactory<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Mode extends 'durable' | 'ephemeral',
> = Mode extends 'ephemeral'
  ? EphemeralRemoteClientFactory<Id, Methods>
  : RemoteClientFactory<Id, Methods>;

export type AgentClientSpec<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec = {},
  Mode extends 'durable' | 'ephemeral' = 'durable',
> = {
  readonly name: string;
  readonly id: Id;
  readonly methods: Methods;
  readonly config?: Config;
} & (Mode extends 'ephemeral' ? { readonly mode: 'ephemeral' } : { readonly mode?: 'durable' });

export interface AgentClientBindingSpec<Methods extends MethodsRecord> {
  readonly name?: string;
  readonly methods: Methods;
  readonly id?: never;
  readonly config?: never;
  readonly mode?: never;
}

/**
 * Build a typed client definition without registering or implementing an agent.
 * Its Standard Schema inputs may come from any schema library supported by the SDK.
 */
export function defineAgentClient<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec = {},
>(
  spec: AgentClientSpec<Id, Methods, Config, 'ephemeral'>,
): AgentClientDefinition<Id, Methods, Config, 'ephemeral'>;
export function defineAgentClient<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec = {},
>(
  spec: AgentClientSpec<Id, Methods, Config, 'durable'>,
): AgentClientDefinition<Id, Methods, Config, 'durable'>;
export function defineAgentClient<Methods extends MethodsRecord>(
  spec: AgentClientBindingSpec<Methods>,
): AgentClientBindingDefinition<Methods>;
export function defineAgentClient(spec: {
  readonly name?: string;
  readonly id?: IdRecord;
  readonly methods: MethodsRecord;
  readonly config?: ConfigSpec;
  readonly mode?: 'durable' | 'ephemeral';
}):
  | AgentClientDefinition<IdRecord, MethodsRecord, ConfigSpec, 'durable' | 'ephemeral'>
  | AgentClientBindingDefinition<MethodsRecord> {
  return defineAgentClientImpl(spec);
}

function defineAgentClientImpl(spec: {
  readonly name?: string;
  readonly id?: IdRecord;
  readonly methods: MethodsRecord;
  readonly config?: ConfigSpec;
  readonly mode?: 'durable' | 'ephemeral';
}):
  | AgentClientDefinition<IdRecord, MethodsRecord, ConfigSpec, 'durable' | 'ephemeral'>
  | AgentClientBindingDefinition<MethodsRecord> {
  if (spec.name !== undefined && spec.id !== undefined) {
    const exact: AgentClientContract<IdRecord, MethodsRecord, ConfigSpec, 'durable' | 'ephemeral'> =
      {
        name: spec.name,
        id: spec.id,
        methods: spec.methods,
        config: spec.config,
        mode: spec.mode ?? 'durable',
      };
    return Object.freeze({ ...exact, ...buildAgentClientSurface(exact, true) });
  }
  if (spec.id !== undefined || spec.config !== undefined || spec.mode !== undefined) {
    throw new TypeError(
      'Agent ID binding contracts may only define methods and an optional name; id, config, and mode require a complete exact name + id definition',
    );
  }
  const binding = buildAgentIdBinding(spec, true);
  return Object.freeze({
    ...(spec.name === undefined ? {} : { name: spec.name }),
    methods: spec.methods,
    ...binding,
  });
}

interface NamedCodec {
  name: string;
  codec: SchemaCodec;
}
interface CompiledRemoteMethod {
  name: string;
  inputCodecs: NamedCodec[];
  output: { tag: 'unit' } | { tag: 'single'; codec: SchemaCodec };
}

/** Encode a method/constructor input record (positional, declaration order). */
function encodeRecord(codecs: NamedCodec[], input: Record<string, unknown>) {
  return v.record(codecs.map((c) => c.codec.toValue(input[c.name])));
}

function assertValueMatchesType(value: SchemaValue, type: SchemaType, graph: SchemaGraph): void {
  let body = type.body;
  const seenRefs = new Set<string>();
  while (body.tag === 'ref') {
    if (seenRefs.has(body.id)) throw new Error(`Cyclic schema reference ${body.id}`);
    seenRefs.add(body.id);
    const def = graph.defs.get(body.id);
    if (!def) throw new Error(`Missing schema definition ${body.id}`);
    body = def.body.body;
  }

  if (value.tag !== body.tag) {
    throw new Error(`Expected schema value ${body.tag}, got ${value.tag}`);
  }
}

/** Walk a nested object by path; `present` is false if any segment is missing. */
function getAtPath(
  obj: Record<string, unknown>,
  path: string[],
): { present: boolean; value?: unknown } {
  let cur: unknown = obj;
  for (const seg of path) {
    if (cur === null || typeof cur !== 'object') {
      throw new Error(`Expected object while traversing config path ${path.join('.')}`);
    }
    if (!Object.prototype.hasOwnProperty.call(cur, seg)) {
      return { present: false };
    }
    cur = (cur as Record<string, unknown>)[seg];
  }
  return { present: true, value: cur };
}

/**
 * Encode config overrides (a nested object mirroring the agent's config shape)
 * into the `TypedAgentConfigValue[]` a remote `WasmRpc` accepts. Only `local`
 * (non-secret) leaves present in `overrides` are encoded; overriding a secret
 * leaf over RPC is rejected (secrets are provisioned host-side).
 */
function encodeConfigOverrides(
  declarations: ConfigDeclaration[],
  overrides: Record<string, unknown>,
): AgentConfigEntry[] {
  const out: AgentConfigEntry[] = [];
  for (const decl of declarations) {
    const found = getAtPath(overrides, decl.path);
    if (!found.present) continue;
    if (decl.source === 'secret') {
      throw new Error(
        `Cannot override secret config field '${decl.path.join('.')}' over RPC; secrets are provisioned host-side.`,
      );
    }
    out.push({
      path: [...decl.path],
      value: { graph: decl.graph, value: decl.codec.toValue(found.value) },
    });
  }
  return out;
}

function compileRemoteMethods(methods: MethodsRecord): CompiledRemoteMethod[] {
  return Object.entries(methods).map(([name, spec]) => {
    const inputCodecs: NamedCodec[] = Object.keys(spec.input)
      .map((key) => ({ name: key, codec: compileSchema(spec.input[key]) }))
      .filter((entry) => entry.codec.autoInjected !== 'principal');
    const returnCodec = compileSchema(spec.returns);
    return {
      name,
      inputCodecs,
      output: returnCodec.isUnit
        ? ({ tag: 'unit' } as const)
        : ({ tag: 'single', codec: returnCodec } as const),
    };
  });
}

function createRemoteClient<Methods extends MethodsRecord, Mode extends 'durable' | 'ephemeral'>(
  methodCodecs: CompiledRemoteMethod[],
  mode: Mode,
  remote: ReturnType<typeof resolveRemoteAgentFallibly>,
): RemoteClient<Methods, Mode> {
  const decodeOutput = (method: CompiledRemoteMethod, value: unknown): unknown => {
    if (method.output.tag === 'unit') return undefined;
    if (value === undefined) {
      throw new RemoteOutputError(
        `Remote agent ${remote.agentId}.${method.name} returned no value for a non-unit output`,
      );
    }
    try {
      const decoded = value as SchemaValue;
      assertValueMatchesType(decoded, method.output.codec.graph.root, method.output.codec.graph);
      return method.output.codec.fromValue(decoded);
    } catch (error) {
      throw new RemoteOutputError(
        `Remote agent ${remote.agentId}.${method.name} returned an invalid output: ${error instanceof Error ? error.message : String(error)}`,
        { cause: error },
      );
    }
  };

  const client: Record<string, unknown> = {};
  for (const method of methodCodecs) {
    const invoke = async (input: Record<string, unknown> = {}, signal?: AbortSignal) => {
      const invocation = await remote.invokeAndAwaitWithMetadata(
        method.name,
        encodeRecord(method.inputCodecs, input),
        signal,
      );
      const value = decodeOutput(method, invocation.value);
      return mode === 'ephemeral' ? { metadata: invocation.metadata, value } : value;
    };
    const methodFn =
      method.inputCodecs.length === 0
        ? (options?: RemoteCallOptions) => invoke({}, options?.signal)
        : (input: Record<string, unknown>, options?: RemoteCallOptions) =>
            invoke(input, options?.signal);
    client[method.name] = Object.assign(methodFn, {
      trigger: (input: Record<string, unknown> = {}) => {
        const metadata = remote.invokeWithMetadata(
          method.name,
          encodeRecord(method.inputCodecs, input),
        );
        return mode === 'ephemeral' ? metadata : undefined;
      },
      schedule: (at: Datetime, input: Record<string, unknown> = {}) => {
        const receipt = remote.scheduleCancelableWithMetadata(
          at,
          method.name,
          encodeRecord(method.inputCodecs, input),
        );
        return mode === 'ephemeral' ? receipt : receipt.cancellationToken;
      },
    });
  }
  return client as RemoteClient<Methods, Mode>;
}

function buildAgentIdBinding<Methods extends MethodsRecord>(
  def: { readonly name?: string; readonly methods: Methods },
  fallible: boolean,
): { [bindAgentClient](agentId: AgentId): RemoteClient<Methods> } {
  const methodCodecs = compileRemoteMethods(def.methods);
  return {
    [bindAgentClient](agentId) {
      return bindExistingAgent(def.name, methodCodecs, fallible, agentId, 'durable');
    },
  };
}

function bindExistingAgent<Methods extends MethodsRecord, Mode extends 'durable' | 'ephemeral'>(
  exactName: string | undefined,
  methodCodecs: CompiledRemoteMethod[],
  fallible: boolean,
  agentId: AgentId,
  mode: Mode,
): RemoteClient<Methods, Mode> {
  const parts = AgentId.parse(agentId);
  if (exactName !== undefined && exactName !== parts.typeName) {
    throw new TypeError(
      `Agent client contract '${exactName}' cannot bind agent type '${parts.typeName}'`,
    );
  }
  if (mode === 'ephemeral') {
    throw new TypeError(
      `Cannot bind existing AgentId '${agentId.agentId}' to ephemeral agent type '${parts.typeName}'; use its client.newPhantom(...) factory`,
    );
  }
  const remote = (fallible ? resolveRemoteAgentFallibly : resolveRemoteAgent)(
    parts.typeName,
    parts.constructorValue,
    parts.phantomId,
    [],
    mode,
  );
  return createRemoteClient<Methods, Mode>(methodCodecs, mode, remote);
}

/** @internal Build the identity and typed-client surface attached to a definition. */
export function buildAgentClientSurface<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec,
  Mode extends 'durable' | 'ephemeral',
>(
  def: AgentClientContract<Id, Methods, Config, Mode> & { readonly name: string; readonly id: Id },
  fallible: boolean,
): {
  client: Mode extends 'ephemeral'
    ? EphemeralRemoteClientFactory<Id, Methods>
    : RemoteClientFactory<Id, Methods>;
  agentId: Mode extends 'ephemeral'
    ? (id: InferRecord<CallerInput<Id>>, phantomId: Uuid) => AgentId
    : (id: InferRecord<CallerInput<Id>>, phantomId?: Uuid) => AgentId;
  [bindAgentClient](agentId: AgentId): RemoteClient<Methods, Mode>;
} {
  // Compile the def's id + method codecs once (cached in this closure).
  const idCodecs: NamedCodec[] = Object.keys(def.id)
    .map((k) => ({ name: k, codec: compileSchema((def.id as Id)[k]) }))
    .filter((nc) => nc.codec.autoInjected !== 'principal');
  const methodCodecs = compileRemoteMethods(def.methods);

  const configDecls: ConfigDeclaration[] = compileConfig(def.config);

  const createAgentId = (id: InferRecord<CallerInput<Id>>, phantomId?: Uuid): AgentId =>
    AgentId.create({
      componentId: getSelfMetadata().agentId.componentId,
      typeName: def.name,
      constructorValue: encodeRecord(idCodecs, id as Record<string, unknown>),
      phantomId,
    });

  const createClient = (
    id: InferRecord<CallerInput<Id>>,
    phantomId?: Uuid,
    config?: Record<string, unknown>,
  ): RemoteClient<Methods, Mode> => {
    const agentConfig = config ? encodeConfigOverrides(configDecls, config) : [];
    const remote = (fallible ? resolveRemoteAgentFallibly : resolveRemoteAgent)(
      def.name,
      encodeRecord(idCodecs, id as Record<string, unknown>),
      phantomId,
      agentConfig,
      def.mode,
    );
    return createRemoteClient<Methods, Mode>(methodCodecs, def.mode, remote);
  };

  const newPhantom = (
    id: InferRecord<CallerInput<Id>>,
    config?: Record<string, unknown>,
  ): PhantomClientDetails<Methods> | RemoteClient<Methods, 'ephemeral'> => {
    if (def.mode === 'ephemeral') {
      return createClient(id, undefined, config) as RemoteClient<Methods, 'ephemeral'>;
    }
    const phantomId = Uuid.generate();
    return {
      client: createClient(id, phantomId, config) as RemoteClient<Methods>,
      agentId: createAgentId(id, phantomId),
      phantomId,
    };
  };

  let client;
  if (def.mode === 'ephemeral') {
    client = {
      getPhantom: (id, phantomId, config) =>
        createClient(id, phantomId, config) as RemoteClient<Methods, 'ephemeral'>,
      newPhantom,
    } as EphemeralRemoteClientFactory<Id, Methods>;
  } else {
    client = {
      get: (id, config) => createClient(id, undefined, config) as RemoteClient<Methods>,
      getPhantom: (id, phantomId, config) =>
        createClient(id, phantomId, config) as RemoteClient<Methods>,
      newPhantom,
    } as RemoteClientFactory<Id, Methods>;
  }

  return {
    client: client as Mode extends 'ephemeral'
      ? EphemeralRemoteClientFactory<Id, Methods>
      : RemoteClientFactory<Id, Methods>,
    agentId: createAgentId as Mode extends 'ephemeral'
      ? (id: InferRecord<CallerInput<Id>>, phantomId: Uuid) => AgentId
      : (id: InferRecord<CallerInput<Id>>, phantomId?: Uuid) => AgentId,
    [bindAgentClient](agentId) {
      return bindExistingAgent(def.name, methodCodecs, fallible, agentId, def.mode);
    },
  };
}
