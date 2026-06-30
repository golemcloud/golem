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
import { InputRecord, MethodSpec } from './method';
import { ParsedAgentId } from '../agentId';
import { Principal } from '../principal';
import { Uuid } from '../uuid';
import { registerAgentInitiator, registerAgentType, RegisteredAgent } from './runtime';

export type IdRecord = Record<string, StandardSchemaV1>;
export type MethodsRecord = Record<string, MethodSpec>;

type InferRecord<R extends Record<string, StandardSchemaV1>> = {
  [K in keyof R]: StandardSchemaV1.InferOutput<R[K]>;
};

/** The handler signature inferred for a method spec (no-arg when input is empty). */
type HandlerFor<M> =
  M extends MethodSpec<infer Input, infer Output>
    ? keyof Input extends never
      ? () => Output | Promise<Output>
      : (input: InferRecord<Input>) => Output | Promise<Output>
    : never;

/** SDK helpers available on a handler's `this` (alongside state). */
export interface FluentAgentThis {
  getId(): ParsedAgentId;
  getPhantomId(): Uuid | undefined;
  getPrincipal(): Principal;
}

export interface InitContext<Id extends IdRecord> {
  readonly id: InferRecord<Id>;
  readonly principal: Principal;
  readonly phantomId: Uuid | undefined;
}

export interface AgentImplementation<
  Id extends IdRecord,
  Methods extends MethodsRecord,
  State extends object,
> {
  init: (ctx: InitContext<Id>) => State | Promise<State>;
  /** One handler per declared method; `this` is bound to `State` + SDK helpers. */
  methods: { [K in keyof Methods]: HandlerFor<Methods[K]> } & ThisType<State & FluentAgentThis>;
}

export interface AgentImpl {
  readonly name: string;
}

export interface AgentDefinition<Id extends IdRecord, Methods extends MethodsRecord> {
  readonly name: string;
  readonly id: Id;
  readonly methods: Methods;
  /** Supply the runtime behaviour. Registers the agent at module-load time. */
  implement<State extends object>(impl: AgentImplementation<Id, Methods, State>): AgentImpl;
}

export interface AgentSpec<Id extends IdRecord, Methods extends MethodsRecord> {
  /** The wire-level agent type name. */
  name: string;
  id: Id;
  methods: Methods;
}

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
export function defineAgent<Id extends IdRecord, Methods extends MethodsRecord>(
  spec: AgentSpec<Id, Methods>,
): AgentDefinition<Id, Methods> {
  const registered: RegisteredAgent = registerAgentType(spec.name, spec.id, spec.methods);
  return {
    name: spec.name,
    id: spec.id,
    methods: spec.methods,
    implement(impl) {
      registerAgentInitiator(registered, impl as AgentImplementation<IdRecord, MethodsRecord, object>);
      return { name: spec.name };
    },
  };
}
