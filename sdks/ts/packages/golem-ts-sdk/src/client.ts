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

// Wasm-RPC clients attached to agent definitions. `def.client.upsert(id)` returns a
// typed proxy that calls a remote agent declared with the same definition. The wire encoding
// is built from the LOCAL def's `SchemaCodec`s — the same codecs the exported
// component uses to decode (see runtime.ts `invoke`) — so the two sides are
// symmetric by construction. Reuses the host `WasmRpc` resource (no decorator
// `Type.Type`/metadata).

import {
  getAgentTypeByAgentId,
  type Datetime,
  type CancellationToken,
} from 'golem:agent/host@2.0.0';
import type { CancelableScheduledInvocationReceipt, InvocationMetadata } from './invocationId';
import {
  encodeChild,
  v,
  withIsolatedCapabilityAdoptionTransaction,
  schemaValueFromWit,
} from './internal/schema-model';
import type { SchemaGraph, SchemaType, SchemaValue } from './internal/schema-model';
import { compileConfig, ConfigDeclaration } from './config';
import type { AgentId } from './agentId';
import { compileSchema } from './schema/adapter';
import { SchemaCodec, SchemaValueWriter, SchemaValueReader } from './schema/codec';
import type { SchemaValueTree } from 'golem:core/types@2.0.0';
import type { MarkerKindOf } from './schema/markers';
import type { ScopeCard } from './host/permissions';
import { StandardSchemaV1 } from './schema/standardSchema';
import type {
  FullAgentClientShape,
  MethodOnlyAgentClientDefinition,
  FullAgentClientDefinition,
  ConfigSpec,
  IdRecord,
  MethodsRecord,
} from './defineAgent';
import { MethodSpec } from './method';
import {
  upsertRemoteAgent,
  createRemoteAgent,
  getRemoteAgentById,
  bindSelfRemoteAgent,
  wireRemoteAgent,
  RemoteCallError,
  RemoteOutputError,
  type AgentConfigEntry,
  type RemoteAgentHandle,
} from './bridge/agent';
export type { AgentConfigEntry } from './bridge/agent';

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
    ? keyof Input extends never
      ? {
          (options?: RemoteCallOptions): Promise<Output>;
          /** Fire-and-forget; no result is awaited. */
          trigger(): void;
          /** Enqueue at `at`, returning a token to cancel it before it runs. */
          schedule(at: Datetime): CancellationToken;
        }
      : {
          (input: InferRecord<Input>, options?: RemoteCallOptions): Promise<Output>;
          trigger(input: InferRecord<Input>): void;
          /** Enqueue at `at`, returning a token to cancel it before it runs. */
          schedule(at: Datetime, input: InferRecord<Input>): CancellationToken;
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
    ? keyof Input extends never
      ? {
          (options?: RemoteCallOptions): Promise<EphemeralInvocationResult<Output>>;
          trigger(): InvocationMetadata;
          schedule(at: Datetime): CancelableScheduledInvocationReceipt;
        }
      : {
          (
            input: InferRecord<Input>,
            options?: RemoteCallOptions,
          ): Promise<EphemeralInvocationResult<Output>>;
          trigger(input: InferRecord<Input>): InvocationMetadata;
          schedule(at: Datetime, input: InferRecord<Input>): CancelableScheduledInvocationReceipt;
        }
    : never;

/** Options for an awaited remote call. */
export interface RemoteCallOptions {
  signal?: AbortSignal;
  /**
   * A scope card for this call. It replaces the client's default scope card;
   * `null` sends no scope card. The call borrows the card: the callee holds its
   * authority until the invocation ends, and the caller keeps the card. A scope
   * card is valid only during the invocation that derived it.
   */
  scopeCard?: ScopeCard | null;
}

/** Options for a remote client. */
export interface RemoteClientOptions {
  /**
   * The default scope card for every awaited call of the client. Each call
   * borrows the card; `trigger` and `schedule` never send a scope card. A scope
   * card is valid only during the invocation that derived it, so a client with
   * a default scope card is too: later awaited calls fail with `denied`.
   */
  scopeCard?: ScopeCard;
}

/** The ID accessor of a durable remote client. */
export interface RemoteAgentIdAccessor {
  /**
   * The ID of the agent the client connects to. On a client of `upsert` it creates the agent
   * when it is missing, without invoking a method. Host failures reject with a
   * {@link RemoteCallError}.
   */
  agentId(): Promise<AgentId>;
}

/**
 * A typed remote client: one async method per declared method on the def. A durable client also
 * has `agentId()`, so a durable agent type cannot declare a method named `agentId`.
 */
export type RemoteClient<
  Methods extends MethodsRecord,
  Mode extends 'durable' | 'ephemeral' = 'durable',
> = {
  [K in keyof Methods]: Mode extends 'ephemeral'
    ? EphemeralRemoteMethodFor<Methods[K]>
    : DurableRemoteMethodFor<Methods[K]>;
} & (Mode extends 'ephemeral' ? unknown : RemoteAgentIdAccessor);

type ConfigObjectShapeOf<S> = S extends { readonly shape: infer Shape }
  ? Shape
  : S extends { readonly entries: infer Shape }
    ? Shape
    : S extends { readonly fields: infer Shape }
      ? Shape
      : never;

type ConfigOverrideField<S> =
  MarkerKindOf<S> extends 'secret'
    ? never
    : [ConfigObjectShapeOf<S>] extends [never]
      ? S extends StandardSchemaV1
        ? StandardSchemaV1.InferOutput<S>
        : never
      : ConfigOverrides<Extract<ConfigObjectShapeOf<S>, ConfigSpec>>;

/** Typed, partial non-secret configuration values that `create` sets on the new agent. */
export type ConfigOverrides<Config extends ConfigSpec> = {
  readonly [K in keyof Config]?: ConfigOverrideField<Config[K]>;
};

/** Options of `create`. */
export interface RemoteCreateOptions<Config extends ConfigSpec = {}> extends RemoteClientOptions {
  /**
   * Creates the keyed agent of the constructor parameters, and rejects with a `conflict`
   * {@link RemoteCallError} when it already exists. Otherwise the new agent has no key and a
   * new ID.
   */
  readonly keyed?: boolean;
  /** Overrides of the agent type's local configuration values. */
  readonly config?: ConfigOverrides<Config>;
  /** Environment variables added to the ones the manifest defines. */
  readonly env?: Readonly<Record<string, string>>;
}

export interface RemoteClientFactory<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec = {},
> {
  /**
   * Bind the current durable incarnation without creating it or waiting for initialization.
   *
   * Use this to schedule work on self from inside the constructor. Normal `upsert` reuses
   * an initialized agent without rerunning its constructor, but waits for an unfinished
   * constructor. Calling `upsert` on self from that constructor would therefore deadlock.
   *
   * Scheduled work runs after construction finishes. This does not make awaited
   * self-calls reentrant or bypass method permissions. A different client type throws
   * an error. Replay preserves the recorded incarnation.
   */
  bindSelf(options?: RemoteClientOptions): RemoteClient<Methods>;
  /**
   * Address the keyed agent of these constructor parameters, and create it when it is missing.
   * Host failures throw a {@link RemoteCallError}.
   */
  upsert(id: InferRecord<Id>, options?: RemoteClientOptions): RemoteClient<Methods>;
  /**
   * Create an agent, and resolve after its initialization. With `keyed`, it is the keyed agent of
   * these constructor parameters, and an existing one rejects with a `conflict`
   * {@link RemoteCallError}. Without `keyed`, each call creates a new agent with a new ID.
   */
  create(
    id: InferRecord<Id>,
    options?: RemoteCreateOptions<Config>,
  ): Promise<RemoteClient<Methods>>;
  /**
   * Address the existing agent with this ID. Never creates the agent: an unknown ID, or the ID of
   * an agent of another type, rejects with a `not-found` {@link RemoteCallError}.
   */
  getById(agentId: AgentId, options?: RemoteClientOptions): Promise<RemoteClient<Methods>>;
}

/** Creates logical ephemeral clients whose final identity is allocated per invocation. */
export interface EphemeralRemoteClientFactory<Id extends IdRecord, Methods extends MethodsRecord> {
  /**
   * Create a logical client; each invocation runs on a new one-shot instance whose identity the
   * invocation returns. Host failures throw a {@link RemoteCallError}.
   */
  upsert(id: InferRecord<Id>, options?: RemoteClientOptions): RemoteClient<Methods, 'ephemeral'>;
}

export type FullAgentClientFactory<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec,
  Mode extends 'durable' | 'ephemeral',
> = Mode extends 'ephemeral'
  ? EphemeralRemoteClientFactory<Id, Methods>
  : RemoteClientFactory<Id, Methods, Config>;

export type FullAgentClientSpec<
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

export interface MethodOnlyAgentClientSpec<Methods extends MethodsRecord> {
  readonly methods: Methods;
  readonly name?: never;
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
  spec: FullAgentClientSpec<Id, Methods, Config, 'ephemeral'>,
): FullAgentClientDefinition<Id, Methods, Config, 'ephemeral'>;
export function defineAgentClient<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec = {},
>(
  spec: FullAgentClientSpec<Id, Methods, Config, 'durable'>,
): FullAgentClientDefinition<Id, Methods, Config, 'durable'>;
export function defineAgentClient<Methods extends MethodsRecord>(
  spec: MethodOnlyAgentClientSpec<Methods>,
): MethodOnlyAgentClientDefinition<Methods>;
export function defineAgentClient(spec: {
  readonly name?: string;
  readonly id?: IdRecord;
  readonly methods: MethodsRecord;
  readonly config?: ConfigSpec;
  readonly mode?: 'durable' | 'ephemeral';
}):
  | FullAgentClientDefinition<IdRecord, MethodsRecord, ConfigSpec, 'durable' | 'ephemeral'>
  | MethodOnlyAgentClientDefinition<MethodsRecord> {
  return defineAgentClientImpl(spec);
}

function defineAgentClientImpl(spec: {
  readonly name?: string;
  readonly id?: IdRecord;
  readonly methods: MethodsRecord;
  readonly config?: ConfigSpec;
  readonly mode?: 'durable' | 'ephemeral';
}):
  | FullAgentClientDefinition<IdRecord, MethodsRecord, ConfigSpec, 'durable' | 'ephemeral'>
  | MethodOnlyAgentClientDefinition<MethodsRecord> {
  if (spec.name !== undefined && spec.id !== undefined) {
    const full: FullAgentClientShape<IdRecord, MethodsRecord, ConfigSpec, 'durable' | 'ephemeral'> =
      {
        name: spec.name,
        id: spec.id,
        methods: spec.methods,
        config: spec.config,
        mode: spec.mode ?? 'durable',
      };
    return Object.freeze({ ...full, ...buildAgentClientSurface(full) });
  }
  if (
    spec.name !== undefined ||
    spec.id !== undefined ||
    spec.config !== undefined ||
    spec.mode !== undefined
  ) {
    throw new TypeError(
      'A method-only client may define only methods; name, id, config, and mode require a fully defined client',
    );
  }
  const methodCodecs = compileRemoteMethods(spec.methods);
  return Object.freeze({
    methods: spec.methods,
    getById: async (agentId: AgentId, options?: RemoteClientOptions) => {
      const registered = getAgentTypeByAgentId(agentId);
      if (registered === undefined) {
        throw new RemoteCallError(`Failed to connect to remote agent ${agentId.toString()}`, {
          tag: 'not-found',
          details: `no deployed agent type has the type tag of agent ${agentId.toString()}`,
        });
      }
      if (registered.agentType.mode === 'ephemeral') {
        throw new RemoteCallError(`Failed to connect to remote agent ${agentId.toString()}`, {
          tag: 'not-found',
          details: `agent ${agentId.toString()} has the ephemeral agent type '${registered.agentType.typeName}', which cannot be addressed by ID`,
        });
      }
      return createRemoteClient<MethodsRecord, 'durable'>(
        methodCodecs,
        'durable',
        await getRemoteAgentById(registered.agentType.typeName, agentId),
        options,
      );
    },
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
  return withIsolatedCapabilityAdoptionTransaction(() =>
    v.record(codecs.map((c) => encodeChild(c.codec, input[c.name]))),
  );
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
  const paths = declarations.map((decl) => decl.path);
  const checkPaths = (value: unknown, prefix: string[]): void => {
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
      throw new TypeError(`Expected object at config path '${prefix.join('.')}'`);
    }
    for (const [key, child] of Object.entries(value)) {
      const path = [...prefix, key];
      if (
        paths.some(
          (declared) =>
            declared.length === path.length && declared.every((part, i) => part === path[i]),
        )
      ) {
        continue;
      }
      if (
        !paths.some(
          (declared) =>
            declared.length > path.length && path.every((part, i) => declared[i] === part),
        )
      ) {
        throw new TypeError(`Unknown config path '${path.join('.')}'`);
      }
      checkPaths(child, path);
    }
  };
  checkPaths(overrides, []);
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
    const inputCodecs: NamedCodec[] = Object.keys(spec.input).map((key) => ({
      name: key,
      codec: compileSchema(spec.input[key]),
    }));
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
  remote: RemoteAgentHandle,
  clientOptions: RemoteClientOptions = {},
): RemoteClient<Methods, Mode> {
  const wire = wireRemoteAgent(remote);
  const defaultScopeCard = clientOptions.scopeCard;
  const decodeOutput = (method: CompiledRemoteMethod, value: unknown): unknown => {
    if (method.output.tag === 'unit') {
      if (value !== undefined) {
        throw new RemoteOutputError(
          `Remote agent ${remote.target}.${method.name} returned a value for a unit output`,
        );
      }
      return undefined;
    }
    if (value === undefined) {
      throw new RemoteOutputError(
        `Remote agent ${remote.target}.${method.name} returned no value for a non-unit output`,
      );
    }
    try {
      const decoded = value as SchemaValue;
      assertValueMatchesType(decoded, method.output.codec.graph.root, method.output.codec.graph);
      return method.output.codec.fromValue(decoded);
    } catch (error) {
      throw new RemoteOutputError(
        `Remote agent ${remote.target}.${method.name} returned an invalid output: ${error instanceof Error ? error.message : String(error)}`,
        { cause: error },
      );
    }
  };

  const client: Record<string, unknown> = {};
  for (const method of methodCodecs) {
    const direct =
      wire &&
      method.inputCodecs.every((entry) => entry.codec.invocationDirect) &&
      method.output.tag === 'single' &&
      method.output.codec.invocationDirect;
    const encodeWire = (input: Record<string, unknown>) => {
      const writer = new SchemaValueWriter();
      const fields = method.inputCodecs.map(
        (entry) => entry.codec.invocationDirect!.write(input[entry.name], writer)!,
      );
      const root = writer.add({ tag: 'record-value', val: fields });
      return { valueNodes: writer.valueNodes, root };
    };
    const decodeWire = (value: SchemaValueTree | undefined) => {
      if (method.output.tag === 'unit' || value === undefined) return decodeOutput(method, value);
      const codec = method.output.codec;
      if (
        value.valueNodes[value.root]?.tag !== `${codec.graph.root.body.tag}-value` ||
        value.valueNodes.some(
          (node) =>
            node?.tag === 'secret-value' ||
            node?.tag === 'quota-token-handle' ||
            node?.tag === 'permission-card-handle' ||
            node?.tag === 'stream-value',
        )
      )
        return decodeOutput(method, schemaValueFromWit(value));
      try {
        return codec.invocationDirect!.read(
          new SchemaValueReader(value.valueNodes, true),
          value.root,
        );
      } catch (error) {
        throw new RemoteOutputError(
          `Remote agent ${remote.target}.${method.name} returned an invalid output: ${error instanceof Error ? error.message : String(error)}`,
          { cause: error },
        );
      }
    };
    const invoke = async (input: Record<string, unknown> = {}, options?: RemoteCallOptions) => {
      const signal = options?.signal;
      const scopeCard =
        options?.scopeCard === null ? undefined : (options?.scopeCard ?? defaultScopeCard);
      if (direct) {
        const invocation = await wire!.invokeAndAwaitWithMetadata(
          method.name,
          encodeWire(input),
          signal,
          scopeCard,
        );
        const value = decodeWire(invocation.value);
        return mode === 'ephemeral' ? { metadata: invocation.metadata, value } : value;
      }
      const invocation = await remote.invokeAndAwaitWithMetadata(
        method.name,
        encodeRecord(method.inputCodecs, input),
        signal,
        scopeCard,
      );
      const value = decodeOutput(method, invocation.value);
      return mode === 'ephemeral' ? { metadata: invocation.metadata, value } : value;
    };
    const methodFn =
      method.inputCodecs.length === 0
        ? (options?: RemoteCallOptions) => invoke({}, options)
        : (input: Record<string, unknown>, options?: RemoteCallOptions) => invoke(input, options);
    client[method.name] = Object.assign(methodFn, {
      trigger: (input: Record<string, unknown> = {}) => {
        const metadata = direct
          ? wire!.invokeWithMetadata(method.name, encodeWire(input))
          : remote.invokeWithMetadata(method.name, encodeRecord(method.inputCodecs, input));
        return mode === 'ephemeral' ? metadata : undefined;
      },
      schedule: (at: Datetime, input: Record<string, unknown> = {}) => {
        const receipt = direct
          ? wire!.scheduleCancelableWithMetadata(at, method.name, encodeWire(input))
          : remote.scheduleCancelableWithMetadata(
              at,
              method.name,
              encodeRecord(method.inputCodecs, input),
            );
        return mode === 'ephemeral' ? receipt : receipt.cancellationToken;
      },
    });
  }
  if (mode !== 'ephemeral') {
    if (Object.prototype.hasOwnProperty.call(client, 'agentId')) {
      throw new TypeError(
        "A durable agent client cannot have a method named 'agentId'; the client's agentId() returns the agent ID",
      );
    }
    client.agentId = () => remote.agentId();
  }
  return client as RemoteClient<Methods, Mode>;
}

/** @internal Build the typed-client surface attached to a definition. */
export function buildAgentClientSurface<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec,
  Mode extends 'durable' | 'ephemeral',
>(
  def: FullAgentClientShape<Id, Methods, Config, Mode> & {
    readonly name: string;
    readonly id: Id;
  },
): {
  client: FullAgentClientFactory<Id, Methods, Config, Mode>;
} {
  // Compile the def's id + method codecs once (cached in this closure).
  const idCodecs: NamedCodec[] = Object.keys(def.id).map((k) => ({
    name: k,
    codec: compileSchema((def.id as Id)[k]),
  }));
  const methodCodecs = compileRemoteMethods(def.methods);

  const configDecls: ConfigDeclaration[] = compileConfig(def.config);

  const encodeId = (id: InferRecord<Id>) => encodeRecord(idCodecs, id as Record<string, unknown>);

  const upsert = (id: InferRecord<Id>, options?: RemoteClientOptions) =>
    createRemoteClient<Methods, Mode>(
      methodCodecs,
      def.mode,
      upsertRemoteAgent(def.name, encodeId(id)),
      options,
    );

  let client;
  if (def.mode === 'ephemeral') {
    client = { upsert } as EphemeralRemoteClientFactory<Id, Methods>;
  } else {
    const durableClient = (remote: RemoteAgentHandle, options?: RemoteClientOptions) =>
      createRemoteClient<Methods, 'durable'>(methodCodecs, 'durable', remote, options);
    client = {
      bindSelf: (options) => durableClient(bindSelfRemoteAgent(def.name), options),
      upsert: (id, options) => upsert(id, options) as RemoteClient<Methods>,
      create: async (id, options = {}) => {
        const config = options.config
          ? encodeConfigOverrides(configDecls, options.config as Record<string, unknown>)
          : [];
        const remote = await createRemoteAgent(def.name, encodeId(id), {
          keyed: options.keyed ?? false,
          config,
          env: Object.entries(options.env ?? {}),
        });
        return durableClient(remote, options);
      },
      getById: async (agentId, options) =>
        durableClient(await getRemoteAgentById(def.name, agentId), options),
    } as RemoteClientFactory<Id, Methods, Config>;
  }

  return { client: client as FullAgentClientFactory<Id, Methods, Config, Mode> };
}
