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

// The fluent, config-object authoring surface (issue #3449). A `defineAgent`
// def carries the structural contract (name + identity + methods); the paired
// `.implement({ init, methods })` carries plain-async handlers whose `this` is
// bound to the state returned by `init`. Schemas are Standard Schema values
// (Zod / Valibot / ArkType / Effect Schema). No Effect runtime.

import { StandardSchemaV1 } from './schema/standardSchema';
import { MethodSpec } from './method';
import type { InputRecord, MethodHasHttpOf } from './method';
import { ParsedAgentId } from '../agentId';
import { Principal } from '../principal';
import { Uuid } from '../uuid';
import { registerAgentInitiator, registerAgentType, RegisteredAgent } from './runtime';
import { ConfigSpec } from './config';
import { HttpMountSpec } from './http';
import type { MountSpecCovering, WebhookVarsValid } from './httpTypes';
import type { MarkerKindOf, SecretInnerOf } from './schema/markers';
import type { Secret } from './secret';
import { AgentTypeRegistry } from '../internal/registry/agentTypeRegistry';

export type { ConfigSpec } from './config';

export type IdRecord = Record<string, StandardSchemaV1>;
export type MethodsRecord = Record<string, MethodSpec<InputRecord, unknown, boolean>>;

type MethodsHaveHttp<Methods extends MethodsRecord> = true extends {
  [K in keyof Methods]: MethodHasHttpOf<Methods[K]>;
}[keyof Methods]
  ? true
  : false;

/**
 * Recover the field-schema record of an OBJECT schema, or `never` for a
 * non-object. Vendor-agnostic structural detection: Zod exposes the field
 * schemas under `.shape`, Valibot under `.entries`, Effect Schema under
 * `.fields`. Non-object schemas (primitives, arrays, unions, maps, markers)
 * carry none of these, so they resolve to `never` and are typed by their
 * `InferOutput` (read whole), matching the runtime flattening scope.
 */
type ConfigObjectShapeOf<S> = S extends { readonly shape: infer Sh }
  ? Sh
  : S extends { readonly entries: infer Sh }
    ? Sh
    : S extends { readonly fields: infer Sh }
      ? Sh
      : never;

/**
 * Deep view of a single config field's schema `S`:
 * - a secret marker (at any depth) → a lazy {@link Secret} handle over its inner
 *   type (call `.get()` for the value);
 * - an object schema → recurse field-by-field (each field deeply transformed);
 * - anything else (primitive / union / array / map / scalar marker) → its
 *   decoded `InferOutput` value, read whole.
 */
type ConfigFieldView<S> =
  MarkerKindOf<S> extends 'secret'
    ? Secret<SecretInnerOf<S>>
    : [ConfigObjectShapeOf<S>] extends [never]
      ? OutputOf<S>
      : {
          readonly [K in keyof ConfigObjectShapeOf<S>]: ConfigFieldView<ConfigObjectShapeOf<S>[K]>;
        };

/** `InferOutput` guarded for the unconstrained recursion parameter. */
type OutputOf<S> = S extends StandardSchemaV1 ? StandardSchemaV1.InferOutput<S> : unknown;

/**
 * The typed view of an agent's config on `this.config` / `InitContext.config`:
 * one property per declared field, deeply transformed by {@link ConfigFieldView}
 * (secrets → {@link Secret} handles at any depth; nested objects recursed).
 * Accessing an undeclared field is a compile error. Defaults to `{}` (no config).
 */
export type ConfigView<C extends ConfigSpec> = {
  readonly [K in keyof C]: ConfigFieldView<C[K]>;
};

/**
 * Resolved, name-only agent-level metadata handed to {@link registerAgentType}.
 * `dependencies` is a list of dependency agent-type *names* (resolved against
 * the {@link AgentTypeRegistry} when the WIT `AgentType` is assembled).
 */
export interface AgentMetadataSpec {
  readonly description?: string;
  readonly promptHint?: string;
  readonly mode?: 'durable' | 'ephemeral';
  readonly dependencies?: readonly string[];
  readonly snapshotting?: SnapshottingSpec;
  readonly config?: ConfigSpec;
  /** HTTP mount declaration; surfaced as `agent-type.http-mount`. */
  readonly http?: HttpMountSpec;
}

type InferRecord<R extends Record<string, StandardSchemaV1>> = {
  [K in keyof R]: StandardSchemaV1.InferOutput<R[K]>;
};

/** The handler signature inferred for a method spec (no-arg when input is empty). */
type HandlerFor<M> =
  M extends MethodSpec<infer Input, infer Output, boolean>
    ? keyof Input extends never
      ? () => Output | Promise<Output>
      : (input: InferRecord<Input>) => Output | Promise<Output>
    : never;

/** SDK helpers available on a handler's `this` (alongside state). */
export interface FluentAgentThis<Config extends ConfigSpec = {}> {
  getId(): ParsedAgentId;
  getPhantomId(): Uuid | undefined;
  getPrincipal(): Principal;
  /**
   * The agent's config accessor: one fresh-reading getter per declared field.
   * Local fields read their decoded value directly; secret fields yield a lazy
   * {@link Secret} handle (call `.get()`). See {@link ConfigView}.
   */
  readonly config: ConfigView<Config>;
}

export interface InitContext<Id extends IdRecord, Config extends ConfigSpec = {}> {
  readonly id: InferRecord<Id>;
  readonly principal: Principal;
  readonly phantomId: Uuid | undefined;
  /** The agent's config accessor (see {@link FluentAgentThis.config}). */
  readonly config: ConfigView<Config>;
}

export interface AgentImplementation<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec,
  State extends object,
> {
  init: (ctx: InitContext<Id, Config>) => State | Promise<State>;
  /** One handler per declared method; `this` is bound to `State` + SDK helpers. */
  methods: { [K in keyof Methods]: HandlerFor<Methods[K]> } & ThisType<
    State & FluentAgentThis<Config>
  >;
  /**
   * Optional custom snapshot serializer — overrides the default (reflective or
   * typed-`state`) serialization entirely. `this` is the agent instance. `save`
   * returns the raw snapshot bytes; `load` restores from them. Use for state the
   * default JSON path can't represent (mirrors the decorator SDK's
   * `BaseAgent.save/loadSnapshot` and effect's `Snapshot.custom`).
   */
  snapshot?: {
    save: () => Uint8Array | Promise<Uint8Array>;
    load: (bytes: Uint8Array) => void | Promise<void>;
  } & ThisType<State & FluentAgentThis<Config>>;
}

export interface AgentImpl {
  readonly name: string;
}

export interface AgentDefinition<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec = {},
  StateSchema extends StandardSchemaV1 = StandardSchemaV1,
> {
  readonly name: string;
  readonly id: Id;
  readonly methods: Methods;
  /** The agent's config schema (used by `clientFor` to encode config overrides). */
  readonly config?: Config;
  /** Supply the runtime behaviour. Registers the agent at module-load time. */
  implement<State extends object & StandardSchemaV1.InferOutput<StateSchema>>(
    impl: AgentImplementation<Id, Methods, Config, State>,
  ): AgentImpl;
}

/**
 * When the executor should snapshot, mapped to the WIT `snapshotting` variant:
 * - `'disabled'` → `disabled`
 * - `'default'` → `enabled(default)`
 * - `{ periodicSeconds }` → `enabled(periodic(duration))`
 * - `{ everyNInvocations }` → `enabled(every-n-invocation(u16))`
 */
export type SnapshotPolicy =
  | 'disabled'
  | 'default'
  | { periodicSeconds: number }
  | { everyNInvocations: number };

/**
 * Snapshotting configuration. Either a bare {@link SnapshotPolicy} (the SDK
 * snapshots all of `this` by reflection — back-compat default), or `{ policy,
 * state }` where `state` is a Standard Schema: only the schema-declared fields of
 * `this` are serialized (typed + scoped), fixing over-broad snapshots. For fully
 * custom serialization supply `snapshot: { save, load }` on `implement(...)`.
 */
export type SnapshottingSpec<StateSchema extends StandardSchemaV1 = StandardSchemaV1> =
  | SnapshotPolicy
  | { policy?: SnapshotPolicy; state: StateSchema };

interface AgentSpecBase<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec = {},
  StateSchema extends StandardSchemaV1 = StandardSchemaV1,
> {
  /** The wire-level agent type name. */
  name: string;
  id: Id;
  methods: Methods;
  /** Human-readable description, surfaced as `agent-type.description`. */
  description?: string;
  /** Optional `prompt-hint`, surfaced as `agent-constructor.prompt-hint`. */
  promptHint?: string;
  /** Execution mode; defaults to `'durable'`. Surfaced as `agent-type.mode`. */
  mode?: 'durable' | 'ephemeral';
  /**
   * Other agent definitions this agent depends on. Each is emitted as an
   * `agent-dependency` record built from the dependency's already-registered
   * `AgentType`. The dependency MUST have been `defineAgent`-ed before this one.
   */
  dependencies?: AgentDefinition<any, any>[];
  /** Snapshotting policy; defaults to `'disabled'`. Surfaced as `agent-type.snapshotting`. */
  snapshotting?: SnapshottingSpec<StateSchema>;
  /**
   * Named config fields, one Standard Schema value each. Mark a field with
   * `s.secret(inner)` to declare it to the host as `secret<inner>`; any other
   * field is a plain local field. Surfaced as `agent-type.config` and typed on
   * `this.config` / `InitContext.config` via {@link ConfigView}.
   */
  config?: Config;
}

/**
 * HTTP mount for the agent: required when any method declares HTTP endpoints,
 * and optional otherwise. Literal `http.mount('/…')` calls additionally verify
 * that mount variables cover the id record and webhook variables name valid id
 * fields. Plain object-literal / segment-array forms defer those checks to the
 * runtime validation in `runtime.ts`.
 */
type AgentHttpSpec<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  MV extends string,
  WV extends string,
> =
  MethodsHaveHttp<Methods> extends true
    ? { http: MountSpecCovering<Id, MV, WV> & WebhookVarsValid<Id, WV> }
    : { http?: MountSpecCovering<Id, MV, WV> & WebhookVarsValid<Id, WV> };

export type AgentSpec<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec = {},
  MV extends string = keyof Id & string,
  WV extends string = never,
  StateSchema extends StandardSchemaV1 = StandardSchemaV1,
> = AgentSpecBase<Id, Methods, Config, StateSchema> & AgentHttpSpec<Id, Methods, MV, WV>;

/**
 * Define an agent. Registers the agent's `AgentType` metadata immediately (so
 * the host can discover it); the returned def's `.implement(...)` registers the
 * runtime initiator.
 *
 * ```ts
 * export const counterDef = defineAgent({
 *   name: 'counter',
 *   id: { name: z.string() },
 *   methods: {
 *     increment: method({ input: { by: z.number() }, returns: z.number() }),
 *     current:   method({ input: {},                returns: z.number() }),
 *   },
 * });
 *
 * export const counterImpl = counterDef.implement({
 *   init: () => ({ count: 0 }),
 *   methods: {
 *     increment({ by }) { this.count += by; return this.count; },
 *     current()         { return this.count; },
 *   },
 * });
 * ```
 */
export function defineAgent<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  Config extends ConfigSpec = {},
  MV extends string = keyof Id & string,
  WV extends string = never,
  StateSchema extends StandardSchemaV1 = StandardSchemaV1,
>(
  spec: AgentSpec<Id, Methods, Config, MV, WV, StateSchema>,
): AgentDefinition<Id, Methods, Config, StateSchema> {
  const name = spec.name;
  let registered: RegisteredAgent | undefined;
  try {
    registered = registerAgentType(name, spec.id, spec.methods, {
      description: spec.description,
      promptHint: spec.promptHint,
      mode: spec.mode,
      dependencies: (spec.dependencies ?? []).map((d) => d.name),
      snapshotting: spec.snapshotting,
      config: spec.config,
      // The branded `MountSpecCovering<…>` type is a compile-time gate only; strip
      // the phantom brands back to the wide registration-side `HttpMountSpec`.
      http: spec.http as HttpMountSpec | undefined,
    });
  } catch (error) {
    AgentTypeRegistry.recordRegistrationError(
      name,
      `Definition failed: ${error instanceof Error ? error.message : String(error)}`,
    );
  }
  let implemented = false;
  return {
    name,
    id: spec.id,
    methods: spec.methods,
    // Expose the config schema on the def so `clientFor` can encode config
    // overrides for RPC (config-on-RPC); undefined when the agent has no config.
    config: spec.config,
    implement(impl) {
      if (implemented) {
        AgentTypeRegistry.recordRegistrationError(
          name,
          'Implementation failed: implement() was called more than once for this definition',
        );
        return { name };
      }
      implemented = true;
      if (registered) {
        try {
          registerAgentInitiator(
            registered,
            impl as AgentImplementation<IdRecord, MethodsRecord, ConfigSpec, object>,
          );
        } catch (error) {
          AgentTypeRegistry.recordRegistrationError(
            name,
            `Implementation failed: ${error instanceof Error ? error.message : String(error)}`,
          );
        }
      }
      return { name };
    },
  };
}
