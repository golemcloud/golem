/** Typed, schema-native RPC clients for agent definitions. @since 1.6.0 */
import { Effect, Result, Schema, Scope } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { parseUuid, uuidToString } from "golem:core/types@2.0.0"
import * as AgentIdentity from "./AgentIdentity.js"
import type { Identity } from "./AgentIdentity.js"
import { rawPhantomId } from "./AgentIdentity.js"
import type { AgentMetadata } from "./Agent.js"
import {
  ConfigError,
  encodeOverrides,
  type ConfigFields,
  type NonSecretOverride,
} from "./Config.js"
import * as Datetime from "./Datetime.js"
import { DurabilityModeClient } from "./host/DurabilityModeClient.js"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { RpcClient, RpcHostError, type RpcConnection } from "./host/RpcClient.js"
import { awaitInvocation, scheduleCancelableInvocation, wrapHostThrow } from "./internal/rpc.js"
import {
  compileCallerParamBindings,
  compileMethodSpec,
  type CallerInput,
  type CompiledInputCodec,
  type MethodCodec,
  type MethodInput,
  type MethodParams,
  type MethodSpec,
  type MethodSuccess,
  type MethodSuccessType,
} from "./Method.js"
import type { SchemaGraph, SchemaType } from "./internal/schema-model/model.js"
import { SchemaRef } from "./SchemaRef.js"
import { decodeFromWire, type UnsupportedSchemaError } from "./WitCodec.js"

type AnyMethodSpec = MethodSpec<any, any, any>
export type RpcError = AgentHost.RpcError
export type RemoteCallError =
  | { readonly _tag: "RpcCallError"; readonly cause: RpcError }
  | { readonly _tag: "InvalidUuidError"; readonly value: string; readonly reason: string }
  | { readonly _tag: "RemoteResponseError"; readonly reason: string }

const responseError = (reason: string): RemoteCallError => ({ _tag: "RemoteResponseError", reason })

export type InvocationMetadata = AgentHost.InvocationMetadata
export interface EphemeralInvocationResult<T> {
  readonly metadata: InvocationMetadata
  readonly value: T
}
export interface ScheduledInvocation {
  readonly cancel: () => Effect.Effect<void>
}
export interface EphemeralScheduledInvocation extends ScheduledInvocation {
  readonly metadata: InvocationMetadata
}

export interface RemoteMethod<
  P extends MethodParams,
  S extends MethodSuccess,
  E extends Schema.Top,
> {
  (input: MethodInput<P>): Effect.Effect<MethodSuccessType<S>, RemoteCallError | E["Type"]>
  readonly trigger: (input: MethodInput<P>) => Effect.Effect<void, RemoteCallError>
  readonly schedule: (
    at: Datetime.Input,
    input: MethodInput<P>,
  ) => Effect.Effect<
    ScheduledInvocation,
    RemoteCallError | Datetime.DatetimeConversionError,
    Scope.Scope
  >
}
export interface EphemeralRemoteMethod<
  P extends MethodParams,
  S extends MethodSuccess,
  E extends Schema.Top,
> {
  (
    input: MethodInput<P>,
  ): Effect.Effect<EphemeralInvocationResult<MethodSuccessType<S>>, RemoteCallError | E["Type"]>
  readonly trigger: (input: MethodInput<P>) => Effect.Effect<InvocationMetadata, RemoteCallError>
  readonly schedule: (
    at: Datetime.Input,
    input: MethodInput<P>,
  ) => Effect.Effect<
    EphemeralScheduledInvocation,
    RemoteCallError | Datetime.DatetimeConversionError,
    Scope.Scope
  >
}
export type RemoteAgent<Methods extends Record<string, AnyMethodSpec>> = {
  readonly [K in keyof Methods]: Methods[K] extends MethodSpec<infer P, infer S, infer E>
    ? RemoteMethod<P, S, E>
    : never
}
export type EphemeralRemoteAgent<Methods extends Record<string, AnyMethodSpec>> = {
  readonly [K in keyof Methods]: Methods[K] extends MethodSpec<infer P, infer S, infer E>
    ? EphemeralRemoteMethod<P, S, E>
    : never
}
export type PhantomRemoteAgent<Methods extends Record<string, AnyMethodSpec>> =
  RemoteAgent<Methods> & { readonly phantomId: string }

export interface GetOptions<F extends ConfigFields = never> {
  readonly overrides?: [F] extends [never]
    ? never
    : F extends ConfigFields
      ? NonSecretOverride<F>
      : never
}
export interface DurableClient<
  C extends MethodParams,
  M extends Record<string, AnyMethodSpec>,
  F extends ConfigFields,
> {
  readonly get: (
    input: CallerInput<C>,
    options?: GetOptions<F>,
  ) => Effect.Effect<
    RemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    RpcClient | Scope.Scope
  >
  readonly getPhantom: (
    input: CallerInput<C>,
    phantomId: string,
    options?: GetOptions<F>,
  ) => Effect.Effect<
    RemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    RpcClient | Scope.Scope
  >
  readonly newPhantom: (
    input: CallerInput<C>,
    options?: GetOptions<F>,
  ) => Effect.Effect<
    PhantomRemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    DurabilityModeClient | RpcClient | Scope.Scope
  >
}
export interface EphemeralClient<
  C extends MethodParams,
  M extends Record<string, AnyMethodSpec>,
  F extends ConfigFields,
> {
  readonly getPhantom: (
    input: CallerInput<C>,
    phantomId: string,
    options?: GetOptions<F>,
  ) => Effect.Effect<
    EphemeralRemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    RpcClient | Scope.Scope
  >
  readonly newPhantom: (
    input: CallerInput<C>,
    options?: GetOptions<F>,
  ) => Effect.Effect<
    EphemeralRemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    RpcClient | Scope.Scope
  >
}
export type AgentClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  Mode extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
> = "ephemeral" extends Mode ? EphemeralClient<C, Methods, F> : DurableClient<C, Methods, F>

interface CompiledClient<C extends MethodParams = MethodParams> {
  readonly constructorCodec: CompiledInputCodec<C>
  readonly methods: ReadonlyMap<string, MethodCodec<MethodParams, MethodSuccess, Schema.Top>>
}

/** A caller-owned, lifecycle-free method contract. @since 1.6.0 @category models */
export interface MethodOnlyClient<
  Methods extends Record<string, AnyMethodSpec>,
> extends IdentityBinding<RemoteAgent<Methods>> {
  readonly methods: Methods
  /** Raw typed entries are forwarded when this binding creates a worker. */
  readonly bindWithEntries: (
    identity: Identity,
    entries: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
  ) => Effect.Effect<
    RemoteAgent<Methods>,
    RemoteCallError | UnsupportedSchemaError | ClientBindingError,
    RpcClient | Scope.Scope
  >
}

/** Complete caller-owned client definition with typed identity and lifecycle factories. @since 1.6.0 @category models */
export type CompleteClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  Mode extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
> = ClientDefinition<C, Methods, Mode, F> & {
  readonly client: AgentClient<C, Methods, Mode, F>
  readonly agentId: Mode extends "ephemeral"
    ? (
        input: CallerInput<C>,
        phantomId: string,
      ) => Effect.Effect<
        Identity,
        AgentIdentity.AgentIdentityError | UnsupportedSchemaError,
        AgentHostClient
      >
    : (
        input: CallerInput<C>,
        phantomId?: string,
      ) => Effect.Effect<
        Identity,
        AgentIdentity.AgentIdentityError | UnsupportedSchemaError,
        AgentHostClient
      >
} & IdentityBinding<RemoteAgent<Methods>> & {
    /** Declared overrides apply only when this binding creates a worker. */
    readonly bindWithConfig: (
      identity: Identity,
      options?: GetOptions<F>,
    ) => Effect.Effect<
      RemoteAgent<Methods>,
      RemoteCallError | UnsupportedSchemaError | ClientBindingError | ConfigError,
      RpcClient | Scope.Scope
    >
  }

/** Exact caller definition; name and id are inseparable. @since 1.6.0 @category models */
export interface ClientDefinition<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  Mode extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
> {
  readonly name: string
  readonly id: C
  readonly methods: Methods
  readonly mode?: Mode
  readonly config?: AgentMetadata<C, Methods, Mode, F>["config"]
}

/** Identity-only caller definition with no factories or identity construction. @since 1.6.0 @category models */
export interface MethodOnlyDefinition<Methods extends Record<string, AnyMethodSpec>> {
  readonly methods: Methods
  readonly name?: never
  readonly id?: never
  readonly mode?: never
  readonly config?: never
}

/** Local identity/contract validation failed before an RPC connection was opened. @since 1.6.0 @category errors */
export class ClientBindingError {
  readonly _tag = "ClientBindingError"
  constructor(readonly reason: string) {}
}

/** @internal Shared protocol implemented by caller-owned, exact, and reflected contracts. */
export const bindIdentity: unique symbol = Symbol.for("effect-golem/client/bind-identity")

/** A value that can bind a parsed identity without discovery. @since 1.6.0 @category models */
export interface IdentityBinding<
  Client,
  Error = RemoteCallError | UnsupportedSchemaError | ClientBindingError,
  Requirements = RpcClient | Scope.Scope,
> {
  readonly [bindIdentity]: (identity: Identity) => Effect.Effect<Client, Error, Requirements>
}

const decodeOutput = (
  mc: MethodCodec<MethodParams, MethodSuccess, Schema.Top>,
  tree: CoreTypes.SchemaValueTree | undefined,
) => {
  if (mc.outputCodec === undefined) {
    return tree === undefined
      ? Effect.succeed(undefined)
      : Effect.fail(responseError(`${mc.name}: expected unit output`))
  }
  if (tree === undefined) return Effect.fail(responseError(`${mc.name}: expected a result value`))
  return Effect.mapError(decodeFromWire(mc.outputCodec.codec, tree), (error) =>
    responseError(`${mc.name}: failed to decode output: ${String(error)}`),
  )
}

const finishOutput = (mc: MethodCodec<MethodParams, MethodSuccess, Schema.Top>, value: unknown) => {
  if (!mc.errorWrapped) return Effect.succeed(value)
  const result = value as Result.Result<unknown, unknown>
  return Result.isSuccess(result)
    ? Effect.succeed(mc.successVoid ? undefined : result.success)
    : Effect.fail(result.failure)
}

const graphUsesStreams = (graph: SchemaGraph): boolean => {
  const visited = new Set<string>()
  const visit = (type: SchemaType): boolean => {
    const body = type.body
    switch (body.tag) {
      case "stream":
        return true
      case "ref": {
        if (visited.has(body.id)) return false
        visited.add(body.id)
        const definition = graph.defs.get(body.id)
        return definition !== undefined && visit(definition.body)
      }
      case "record":
        return body.fields.some((field) => visit(field.body))
      case "variant":
        return body.cases.some((item) => item.payload !== undefined && visit(item.payload))
      case "tuple":
        return body.elements.some(visit)
      case "list":
      case "fixed-list":
      case "option":
        return visit(body.element)
      case "map":
        return visit(body.key) || visit(body.value)
      case "result":
        return (
          (body.ok !== undefined && visit(body.ok)) || (body.err !== undefined && visit(body.err))
        )
      case "union":
        return body.branches.some((branch) => visit(branch.body))
      case "secret":
        return visit(body.inner)
      default:
        return false
    }
  }
  return visit(graph.root)
}

const buildRemote = (
  rpc: RpcConnection,
  compiled: Pick<CompiledClient, "methods">,
  ephemeral: boolean,
) => {
  const remote: Record<string, unknown> = {}
  for (const [name, mc] of compiled.methods) {
    const streaming =
      graphUsesStreams(mc.inputCodec.graph) ||
      (mc.outputCodec !== undefined && graphUsesStreams(mc.outputCodec.graph))
    const rejectNonAwaitedStream = () =>
      Effect.fail(responseError(`${name}: live streams require invoke-and-await`))
    const encode = (input: Record<string, unknown>) =>
      Effect.mapError(mc.inputCodec.encodeAsync(input), (e) =>
        responseError(`${name}: failed to encode input: ${String(e)}`),
      )
    const call = (input: Record<string, unknown>) =>
      encode(input).pipe(
        Effect.flatMap((tree) => awaitInvocation(rpc, name, tree)),
        Effect.flatMap(({ metadata, result }) =>
          decodeOutput(mc, result).pipe(
            Effect.flatMap((value) => finishOutput(mc, value)),
            Effect.map((value) => (ephemeral ? { metadata, value } : value)),
          ),
        ),
      )
    const trigger = (input: Record<string, unknown>) => {
      if (streaming) return rejectNonAwaitedStream()
      return encode(input).pipe(
        Effect.flatMap((tree) =>
          Effect.try({ try: () => rpc.invoke(name, tree), catch: wrapHostThrow }),
        ),
        Effect.map((metadata) => (ephemeral ? metadata : undefined)),
      )
    }
    const schedule = (at: Datetime.Input, input: Record<string, unknown>) => {
      if (streaming) return rejectNonAwaitedStream()
      return Datetime.fromInput(at).pipe(
        Effect.flatMap((time) => encode(input).pipe(Effect.map((tree) => [time, tree] as const))),
        Effect.flatMap(([time, tree]) => scheduleCancelableInvocation(rpc, time, name, tree)),
        Effect.map(({ metadata, cancel }) => {
          const scheduled = {
            cancel: () => cancel,
          }
          return ephemeral ? { ...scheduled, metadata } : scheduled
        }),
      )
    }
    remote[name] = Object.assign(call, { trigger, schedule })
  }
  return remote
}

const bindingForDefinition = <Methods extends Record<string, AnyMethodSpec>>(definition: {
  readonly methods: Methods
  readonly name?: string
  readonly id?: MethodParams
  readonly mode?: AgentCommon.AgentMode
}): IdentityBinding<RemoteAgent<Methods>> & {
  readonly bindWithEntries: (
    identity: Identity,
    entries: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
  ) => Effect.Effect<
    RemoteAgent<Methods>,
    RemoteCallError | UnsupportedSchemaError | ClientBindingError,
    RpcClient | Scope.Scope
  >
} => {
  let cachedMethods:
    | ReadonlyMap<string, MethodCodec<MethodParams, MethodSuccess, Schema.Top>>
    | undefined
  let cachedConstructor: CompiledInputCodec | undefined
  const compile = Effect.gen(function* () {
    if (cachedMethods === undefined) {
      const methods = new Map<string, MethodCodec<MethodParams, MethodSuccess, Schema.Top>>()
      for (const [name, spec] of Object.entries(definition.methods))
        methods.set(name, (yield* compileMethodSpec(name, spec)) as never)
      cachedMethods = methods
    }
    if (definition.id !== undefined && cachedConstructor === undefined)
      cachedConstructor = yield* compileCallerParamBindings(
        `${definition.name ?? "agent"} constructor`,
        definition.id,
      )
    return { methods: cachedMethods, constructorCodec: cachedConstructor }
  })
  const bindWithEntries = (
    identity: Identity,
    entries: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
  ) =>
    Effect.gen(function* () {
      if (definition.mode === "ephemeral") {
        return yield* Effect.fail(
          new ClientBindingError(
            `Cannot bind existing identity '${identity.encoded}' to ephemeral agent type '${identity.typeName}'; use getPhantom(...) or newPhantom(...)`,
          ),
        )
      }
      if (definition.name !== undefined && identity.typeName !== definition.name) {
        return yield* Effect.fail(
          new ClientBindingError(
            `Agent client contract '${definition.name}' cannot bind agent type '${identity.typeName}'`,
          ),
        )
      }
      const compiled = yield* compile
      if (compiled.constructorCodec !== undefined) {
        const constructor = compiled.constructorCodec.graph
        if (
          !SchemaRef.fromImmutableGraph(constructor, constructor.root).validateValue(
            identity.constructorValue,
          ).success
        ) {
          return yield* Effect.fail(
            new ClientBindingError(
              `Agent client contract '${definition.name}' cannot bind identity '${identity.encoded}': constructor value does not conform to the contract ID schema`,
            ),
          )
        }
      }
      const phantom = yield* Effect.try({
        try: () => rawPhantomId(identity),
        catch: (cause) => new ClientBindingError(String(cause)),
      })
      const host = yield* RpcClient
      const rpc = yield* Effect.acquireRelease(
        host
          .connect(identity.typeName, identity.constructorValue, phantom, entries)
          .pipe(Effect.mapError((error) => wrapHostThrow(error.cause))),
        (connection) => Effect.sync(() => connection.drop()),
      )
      return buildRemote(rpc, compiled, false) as RemoteAgent<Methods>
    })
  return {
    [bindIdentity]: (identity) => bindWithEntries(identity, []),
    bindWithEntries,
  }
}

/** Define a caller-only client without registering an agent. @since 1.6.0 @category constructors */
export function defineAgentClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  F extends ConfigFields = never,
>(
  definition: ClientDefinition<C, Methods, "ephemeral", F> & { readonly mode: "ephemeral" },
): CompleteClient<C, Methods, "ephemeral", F>
export function defineAgentClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  F extends ConfigFields = never,
>(definition: ClientDefinition<C, Methods, "durable", F>): CompleteClient<C, Methods, "durable", F>
export function defineAgentClient<
  const Definition extends { readonly methods: Record<string, AnyMethodSpec> },
>(
  definition: Definition &
    (Extract<keyof Definition, "name" | "id" | "mode" | "config"> extends never
      ? MethodOnlyDefinition<Definition["methods"]>
      : never),
): MethodOnlyClient<Definition["methods"]>
export function defineAgentClient(definition: {
  readonly name?: string
  readonly id?: MethodParams
  readonly methods: Record<string, AnyMethodSpec>
  readonly mode?: AgentCommon.AgentMode
  readonly config?: AgentMetadata<
    MethodParams,
    Record<string, AnyMethodSpec>,
    AgentCommon.AgentMode,
    ConfigFields
  >["config"]
}): unknown {
  const has = (field: string) => Object.prototype.hasOwnProperty.call(definition, field)
  if (
    definition === null ||
    typeof definition !== "object" ||
    !has("methods") ||
    definition.methods === null ||
    typeof definition.methods !== "object"
  )
    throw new TypeError("Agent client definitions require methods")
  if (
    typeof definition.name !== "string" ||
    definition.id === undefined ||
    definition.id === null ||
    typeof definition.id !== "object"
  ) {
    if (has("name") || has("id") || has("mode") || has("config"))
      throw new TypeError(
        "Agent client definitions require both name and id; method-only definitions may only contain methods",
      )
    const methods = Object.freeze({ ...definition.methods })
    return Object.freeze({ methods, ...bindingForDefinition({ methods }) })
  }
  const canonical = Object.freeze({
    ...definition,
    id: Object.freeze({ ...definition.id }),
    methods: Object.freeze({ ...definition.methods }),
  })
  const client = clientFor(
    canonical as AgentMetadata<
      MethodParams,
      Record<string, AnyMethodSpec>,
      AgentCommon.AgentMode,
      ConfigFields
    >,
  )
  const binding = bindingForDefinition(canonical)
  const bindWithConfig = (identity: Identity, options?: GetOptions<ConfigFields>) =>
    Effect.gen(function* () {
      let entries: AgentCommon.TypedAgentConfigValue[] = []
      if (options?.overrides !== undefined) {
        if (canonical.config === undefined)
          return yield* Effect.fail(
            new ConfigError([], {
              _tag: "Unsupported",
              reason: `agent '${canonical.name}' has no config; cannot apply overrides`,
            }),
          )
        entries = yield* encodeOverrides(
          yield* canonical.config.__compile(),
          options.overrides as Record<string, unknown>,
        ).pipe(
          Effect.mapError((error) =>
            error instanceof ConfigError
              ? error
              : responseError(`failed to encode config override: ${String(error)}`),
          ),
        )
      }
      return yield* binding.bindWithEntries(identity, entries)
    })
  let identityCodec: CompiledInputCodec | undefined
  const agentId = (input: CallerInput<MethodParams>, phantomId?: string) =>
    Effect.gen(function* () {
      if (canonical.mode === "ephemeral" && phantomId === undefined)
        return yield* Effect.fail(
          new AgentIdentity.AgentIdentityError(
            new TypeError(`ephemeral agent type '${canonical.name}' requires a phantom ID`),
          ),
        )
      const codec = (identityCodec ??= yield* compileCallerParamBindings(
        `${canonical.name} constructor`,
        canonical.id,
      ))
      const constructorValue = yield* codec
        .encodeAsync(input as MethodInput<MethodParams>)
        .pipe(Effect.mapError((cause) => new AgentIdentity.AgentIdentityError(cause)))
      return yield* AgentIdentity.make({
        typeName: canonical.name!,
        constructorValue,
        ...(phantomId === undefined ? {} : { phantomId }),
      })
    })
  return Object.freeze({ ...canonical, client, agentId, ...binding, bindWithConfig })
}

/** Bind a parsed identity through a caller-owned, exact, or reflected contract. @since 1.6.0 @category constructors */
export const bind = <Client, Error, Requirements>(
  identity: Identity,
  binding: IdentityBinding<Client, Error, Requirements>,
): Effect.Effect<Client, Error, Requirements> => binding[bindIdentity](identity)

/** @internal Build the exact binding protocol shared by `defineAgent` specs. */
export const bindingFor = <
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  Mode extends AgentCommon.AgentMode,
  F extends ConfigFields,
>(
  definition: AgentMetadata<C, Methods, Mode, F>,
): IdentityBinding<RemoteAgent<Methods>> => bindingForDefinition(definition)

/** Build a client from the exact agent-definition input: `name`, `mode`, `id`, `methods`, and optional `config`. */
export const clientFor = <
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  Mode extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
>(
  def: AgentMetadata<C, Methods, Mode, F>,
): AgentClient<C, Methods, Mode, F> => {
  let cached: CompiledClient<C> | undefined
  const compile = Effect.suspend(() =>
    cached
      ? Effect.succeed(cached)
      : Effect.gen(function* () {
          const constructorCodec = yield* compileCallerParamBindings(
            `${def.name} constructor`,
            def.id,
          )
          const methods = new Map<string, MethodCodec<MethodParams, MethodSuccess, Schema.Top>>()
          for (const [name, spec] of Object.entries(def.methods))
            methods.set(name, (yield* compileMethodSpec(name, spec)) as never)
          return (cached = { constructorCodec, methods })
        }),
  )
  const config = (options?: GetOptions<F>) =>
    Effect.gen(function* () {
      const values: AgentCommon.TypedAgentConfigValue[] = []
      if (options?.overrides !== undefined) {
        if (def.config === undefined)
          return yield* Effect.fail(
            new ConfigError([], {
              _tag: "Unsupported",
              reason: `agent '${def.name}' has no config; cannot apply overrides`,
            }),
          )
        const encoded = yield* encodeOverrides(
          yield* def.config.__compile(),
          options.overrides as Record<string, unknown>,
        ).pipe(
          Effect.mapError((error) =>
            error instanceof ConfigError
              ? error
              : responseError(`failed to encode config override: ${String(error)}`),
          ),
        )
        values.push(...encoded)
      }
      return values
    })
  const construct = (
    input: CallerInput<C>,
    phantom: CoreTypes.Uuid | undefined,
    options?: GetOptions<F>,
  ) =>
    Effect.gen(function* () {
      const codecs = yield* compile
      const overrides = yield* config(options)
      const constructorTree = yield* Effect.mapError(
        codecs.constructorCodec.encodeAsync(input as unknown as MethodInput<C>),
        (e) => responseError(`failed to encode constructor input: ${String(e)}`),
      )
      const host = yield* RpcClient
      const rpc = yield* Effect.acquireRelease(
        Effect.mapError(
          host.connect(def.name, constructorTree, phantom, overrides),
          (e: RpcHostError) => wrapHostThrow(e.cause),
        ),
        (connection) => Effect.sync(() => connection.drop()),
      )
      return { rpc, codecs }
    })
  const get = (input: CallerInput<C>, options?: GetOptions<F>) =>
    construct(input, undefined, options).pipe(
      Effect.map(({ rpc, codecs }) => buildRemote(rpc, codecs, false) as RemoteAgent<Methods>),
    )
  const getPhantom = (input: CallerInput<C>, id: string, options?: GetOptions<F>) =>
    Effect.try({
      try: () => parseUuid(id),
      catch: (e): RemoteCallError => ({ _tag: "InvalidUuidError", value: id, reason: String(e) }),
    }).pipe(
      Effect.flatMap((uuid) => construct(input, uuid, options)),
      Effect.map(({ rpc, codecs }) => buildRemote(rpc, codecs, false) as RemoteAgent<Methods>),
    )
  const newPhantom = (input: CallerInput<C>, options?: GetOptions<F>) =>
    Effect.gen(function* () {
      if ((def.mode ?? "durable") === "ephemeral") {
        const { rpc, codecs } = yield* construct(input, undefined, options)
        return buildRemote(rpc, codecs, true) as EphemeralRemoteAgent<Methods>
      }
      const dm = yield* DurabilityModeClient
      const uuid = yield* Effect.try({
        try: () => dm.generateIdempotencyKey(),
        catch: wrapHostThrow,
      })
      const { rpc, codecs } = yield* construct(input, uuid, options)
      return Object.assign(buildRemote(rpc, codecs, false), {
        phantomId: uuidToString(uuid),
      }) as PhantomRemoteAgent<Methods>
    })
  return (
    (def.mode ?? "durable") === "ephemeral"
      ? {
          getPhantom: (input: CallerInput<C>, id: string, options?: GetOptions<F>) =>
            Effect.try({
              try: () => parseUuid(id),
              catch: (e): RemoteCallError => ({
                _tag: "InvalidUuidError",
                value: id,
                reason: String(e),
              }),
            }).pipe(
              Effect.flatMap((uuid) => construct(input, uuid, options)),
              Effect.map(
                ({ rpc, codecs }) =>
                  buildRemote(rpc, codecs, true) as EphemeralRemoteAgent<Methods>,
              ),
            ),
          newPhantom,
        }
      : { get, getPhantom, newPhantom }
  ) as AgentClient<C, Methods, Mode, F>
}
