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

// Fluent wasm-RPC client. `clientFor(def)(id)` returns a typed proxy that calls a
// remote agent declared with the same `defineAgent` definition. The wire encoding
// is built from the LOCAL def's `FluentCodec`s — the exact codecs the exported
// component uses to decode (see runtime.ts `invoke`) — so the two sides are
// symmetric by construction. Reuses the host `WasmRpc` resource (no decorator
// `Type.Type`/metadata).

import {
  makeAgentId,
  WasmRpc,
  Datetime,
  CancellationToken,
  TypedAgentConfigValue,
  InvocationMetadata,
  CancelableScheduledInvocationReceipt,
  RpcError,
} from 'golem:agent/host@2.0.0';
import {
  schemaValueToWit,
  schemaValueFromWit,
  typedSchemaValueToWit,
  v,
} from '../internal/schema-model';
import type { SchemaGraph, SchemaType, SchemaValue } from '../internal/schema-model';
import { awaitAbortable, throwIfAborted } from '../internal/pollableUtils';
import { compileConfig, ConfigDeclaration } from './config';
import { Uuid } from '../uuid';
import { compileSchema } from './schema/adapter';
import { FluentCodec } from './schema/codec';
import { StandardSchemaV1 } from './schema/standardSchema';
import type { MarkerKindOf } from './schema/markers';
import { AgentDefinition, ConfigSpec, IdRecord, MethodsRecord } from './defineAgent';
import { MethodSpec } from './method';

type InferRecord<R extends Record<string, StandardSchemaV1>> = {
  [K in keyof R]: StandardSchemaV1.InferOutput<R[K]>;
};

/** Keys of `C` that are auto-injected `s.principal()` params (host-supplied). */
type AutoInjectedKeys<C> = {
  [K in keyof C & string]: [MarkerKindOf<C[K]>] extends ['principal'] ? K : never;
}[keyof C & string];

/**
 * Caller-facing input for a remote method: the declared params MINUS any
 * auto-injected `s.principal()` param. The callee's host injects the caller's
 * principal, so the RPC caller neither supplies nor encodes it.
 */
type CallerInput<Input extends Record<string, StandardSchemaV1>> = Omit<
  Input,
  AutoInjectedKeys<Input>
>;

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
  readonly phantomId: Uuid;
}

/** Address existing agents or create a fresh phantom agent client. */
export interface RemoteClientFactory<Id extends IdRecord, Methods extends MethodsRecord> {
  (
    id: InferRecord<CallerInput<Id>>,
    phantomId?: Uuid,
    config?: Record<string, unknown>,
  ): RemoteClient<Methods>;
  newPhantom(
    id: InferRecord<CallerInput<Id>>,
    config?: Record<string, unknown>,
  ): PhantomClientDetails<Methods>;
}

/** Creates logical ephemeral clients whose final identity is allocated per invocation. */
export interface EphemeralRemoteClientFactory<Id extends IdRecord, Methods extends MethodsRecord> {
  newPhantom(
    id: InferRecord<CallerInput<Id>>,
    config?: Record<string, unknown>,
  ): RemoteClient<Methods, 'ephemeral'>;
}

interface NamedCodec {
  name: string;
  codec: FluentCodec;
}
interface CompiledRemoteMethod {
  name: string;
  inputCodecs: NamedCodec[];
  output: { tag: 'unit' } | { tag: 'single'; codec: FluentCodec };
}

/** Raised when a remote agent invocation fails or returns an error result. */
export class RemoteCallError extends Error {
  readonly _tag = 'RemoteCallError';
}

function isRpcError(error: unknown): error is RpcError {
  if (error === null || typeof error !== 'object') return false;

  switch ((error as { tag?: unknown }).tag) {
    case 'protocol-error':
    case 'denied':
    case 'not-found':
    case 'remote-internal-error':
    case 'remote-agent-error':
      return true;
    default:
      return false;
  }
}

/** Encode a method/constructor input record (positional, declaration order). */
function encodeRecord(codecs: NamedCodec[], input: Record<string, unknown>) {
  return schemaValueToWit(v.record(codecs.map((c) => c.codec.toValue(input[c.name]))));
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
): TypedAgentConfigValue[] {
  const out: TypedAgentConfigValue[] = [];
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
      value: typedSchemaValueToWit({ graph: decl.graph, value: decl.codec.toValue(found.value) }),
    });
  }
  return out;
}

/**
 * Build a typed RPC client factory for a remote agent definition.
 *
 * ```ts
 * const counter = clientFor(CounterDef);
 * const c1 = counter({ name: 'c1' });
 * const next = await c1.increment({ by: 5 });
 * c1.increment.trigger({ by: 1 }); // fire-and-forget
 * ```
 */
export function clientFor<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec,
  StateSchema extends StandardSchemaV1,
  Mode extends 'durable' | 'ephemeral',
>(
  def: AgentDefinition<Id, Methods, Config, StateSchema, Mode>,
): Mode extends 'ephemeral'
  ? EphemeralRemoteClientFactory<Id, Methods>
  : RemoteClientFactory<Id, Methods> {
  // Compile the def's id + method codecs once (cached in this closure).
  const idCodecs: NamedCodec[] = Object.keys(def.id)
    .map((k) => ({ name: k, codec: compileSchema(def.id[k]) }))
    .filter((nc) => nc.codec.autoInjected !== 'principal');
  const methodCodecs: CompiledRemoteMethod[] = Object.entries(def.methods).map(([name, spec]) => {
    // Skip auto-injected `s.principal()` params: the callee's host injects the
    // caller principal, so the RPC caller encodes no wire field for them (the
    // remaining user-supplied codecs stay in declaration order, matching the
    // callee's cursor decode in runtime.ts `invoke`).
    const inputCodecs: NamedCodec[] = Object.keys((spec as MethodSpec).input)
      .map((k) => ({ name: k, codec: compileSchema((spec as MethodSpec).input[k]) }))
      .filter((nc) => nc.codec.autoInjected !== 'principal');
    const retCodec = compileSchema((spec as MethodSpec).returns);
    const output = retCodec.isUnit
      ? ({ tag: 'unit' } as const)
      : ({ tag: 'single', codec: retCodec } as const);
    return { name, inputCodecs, output };
  });

  const configDecls: ConfigDeclaration[] = compileConfig(def.config);

  const createClient = (
    id: InferRecord<CallerInput<Id>>,
    phantomId?: Uuid,
    config?: Record<string, unknown>,
  ): RemoteClient<Methods, Mode> => {
    const constructorTree = encodeRecord(idCodecs, id as Record<string, unknown>);
    const agentId = makeAgentId(def.name, constructorTree, phantomId);
    const agentConfig = config ? encodeConfigOverrides(configDecls, config) : [];
    const wasmRpc = new WasmRpc(def.name, constructorTree, phantomId, agentConfig);

    const decodeOutput = (mc: CompiledRemoteMethod, val: unknown): unknown => {
      if (mc.output.tag === 'unit') return undefined;
      if (val === undefined) {
        throw new RemoteCallError(
          `Remote agent ${agentId}.${mc.name} returned no value for a non-unit output`,
        );
      }
      try {
        const decoded = schemaValueFromWit(val as Parameters<typeof schemaValueFromWit>[0]);
        assertValueMatchesType(decoded, mc.output.codec.graph.root, mc.output.codec.graph);
        return mc.output.codec.fromValue(decoded);
      } catch (error) {
        throw new RemoteCallError(
          `Remote agent ${agentId}.${mc.name} returned an invalid output: ${error instanceof Error ? error.message : String(error)}`,
        );
      }
    };

    const client: Record<string, unknown> = {};
    for (const mc of methodCodecs) {
      const invoke = async (input: Record<string, unknown> = {}, signal?: AbortSignal) => {
        throwIfAborted(signal);
        const inputTree = encodeRecord(mc.inputCodecs, input);
        const invocation = wasmRpc.asyncInvokeAndAwait(mc.name, inputTree);
        const future = invocation.future;
        let result;
        try {
          result = await awaitAbortable(future.get(), signal, () => future.cancel());
        } catch (error) {
          if (!isRpcError(error)) throw error;

          throw new RemoteCallError(
            `Remote agent ${agentId}.${mc.name} errored: ${JSON.stringify(error, (_, value) =>
              typeof value === 'bigint' ? value.toString() : value,
            )}`,
          );
        }
        const value = decodeOutput(mc, result);
        return def.mode === 'ephemeral' ? { metadata: invocation.metadata, value } : value;
      };
      const methodFn =
        mc.inputCodecs.length === 0
          ? (options?: RemoteCallOptions) => invoke({}, options?.signal)
          : (input: Record<string, unknown>, options?: RemoteCallOptions) =>
              invoke(input, options?.signal);
      client[mc.name] = Object.assign(methodFn, {
        trigger: (input: Record<string, unknown> = {}) => {
          const metadata = wasmRpc.invoke(mc.name, encodeRecord(mc.inputCodecs, input));
          return def.mode === 'ephemeral' ? metadata : undefined;
        },
        schedule: (at: Datetime, input: Record<string, unknown> = {}) => {
          const receipt = wasmRpc.scheduleCancelableInvocation(
            at,
            mc.name,
            encodeRecord(mc.inputCodecs, input),
          );
          return def.mode === 'ephemeral' ? receipt : receipt.cancellationToken;
        },
      });
    }
    return client as RemoteClient<Methods, Mode>;
  };

  createClient.newPhantom = (
    id: InferRecord<CallerInput<Id>>,
    config?: Record<string, unknown>,
  ): PhantomClientDetails<Methods> | RemoteClient<Methods, 'ephemeral'> => {
    if (def.mode === 'ephemeral') {
      return createClient(id, undefined, config) as RemoteClient<Methods, 'ephemeral'>;
    }
    const phantomId = Uuid.generate();
    return { client: createClient(id, phantomId, config) as RemoteClient<Methods>, phantomId };
  };

  return createClient as Mode extends 'ephemeral'
    ? EphemeralRemoteClientFactory<Id, Methods>
    : RemoteClientFactory<Id, Methods>;
}
