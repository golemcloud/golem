/** Typed, schema-native RPC clients for agent definitions. @since 1.6.0 */
import { Effect, Result, Schema, Scope } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { AgentId } from "./AgentId.js"
import type { AgentMetadata } from "./Agent.js"
import {
  ConfigError,
  encodeOverrides,
  type ConfigFields,
  type NonSecretOverride,
} from "./Config.js"
import * as Datetime from "./Datetime.js"
import type { InvocationId } from "./InvocationId.js"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { RpcClient, type RpcConnection } from "./host/RpcClient.js"
import { agentIdFromWit, agentIdToWit } from "./internal/agentId.js"
import {
  awaitInvocation,
  invocationMetadataFromHost,
  scheduleCancelableInvocation,
  wrapHostThrow,
} from "./internal/rpc.js"
import type { GuestScopeCard } from "./internal/scopeCard.js"
import {
  compileParamBindings,
  compileMethodSpec,
  type CompiledInputCodec,
  type MethodCodec,
  type MethodParams,
  type MethodSpec,
  type MethodSuccess,
  type MethodSuccessType,
} from "./Method.js"
import type { SchemaGraph, SchemaType } from "./internal/schema-model/model.js"
import { decodeFromWire, type UnsupportedSchemaError } from "./WitCodec.js"

type AnyMethodSpec = MethodSpec<any, any, any>
export type RpcError = AgentHost.RpcError
/**
 * Failures of remote calls and client construction. `RpcCallError` carries the host
 * `rpc-error`: `not-found` for an unknown agent ID or an ID of another agent type, and
 * `conflict` for a keyed create whose key is current.
 */
export type RemoteCallError =
  | { readonly _tag: "RpcCallError"; readonly cause: RpcError }
  | { readonly _tag: "InvalidUuidError"; readonly value: string; readonly reason: string }
  | { readonly _tag: "RemoteResponseError"; readonly reason: string }
  | { readonly _tag: "ScopeCardUnavailableError"; readonly reason: string }

const responseError = (reason: string): RemoteCallError => ({ _tag: "RemoteResponseError", reason })

/**
 * Host metadata of one invocation: the ID of the agent that ran it and the typed invocation ID.
 * For an ephemeral target, the agent ID is the ID of the one-shot instance.
 *
 * @since 1.6.0
 * @category models
 */
export interface InvocationMetadata {
  readonly agentId: AgentId
  readonly invocationId: InvocationId
}
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
  /**
   * The awaited call with `card` attached as its scope card. The card is borrowed for the call
   * and stays usable; it replaces the client's default scope card. A released card fails the
   * call with `ScopeCardUnavailableError` before the input is encoded.
   */
  readonly withScopeCard: (
    card: GuestScopeCard,
  ) => (input: MethodInput<P>) => Effect.Effect<MethodSuccessType<S>, RemoteCallError | E["Type"]>
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
  /**
   * The awaited call with `card` attached as its scope card. The card is borrowed for the call
   * and stays usable; it replaces the client's default scope card. A released card fails the
   * call with `ScopeCardUnavailableError` before the input is encoded.
   */
  readonly withScopeCard: (
    card: GuestScopeCard,
  ) => (
    input: MethodInput<P>,
  ) => Effect.Effect<EphemeralInvocationResult<MethodSuccessType<S>>, RemoteCallError | E["Type"]>
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

/**
 * Name of the client member that reads the ID of the connected agent. A method with this name
 * cannot be called through a typed client.
 */
const agentIdMember = "agentId"

/**
 * A client of one durable agent. `agentId` reads the ID of the agent from the host; on a client
 * of `upsert` it creates the agent when it is missing, without invoking a method.
 */
export type RemoteAgent<Methods extends Record<string, AnyMethodSpec>> = {
  readonly [K in Exclude<keyof Methods, typeof agentIdMember>]: Methods[K] extends MethodSpec<
    infer P,
    infer S,
    infer E
  >
    ? RemoteMethod<P, S, E>
    : never
} & { readonly agentId: Effect.Effect<AgentId, RemoteCallError> }
export type EphemeralRemoteAgent<Methods extends Record<string, AnyMethodSpec>> = {
  readonly [K in keyof Methods]: Methods[K] extends MethodSpec<infer P, infer S, infer E>
    ? EphemeralRemoteMethod<P, S, E>
    : never
}

export interface ScopeCardOptions {
  /**
   * Default scope card for every awaited call of the client. Each call borrows the card. A scope
   * card is valid only during the invocation that derived it: a released card fails the call
   * with `ScopeCardUnavailableError`, and a card kept for a later invocation makes the host deny
   * the call (`RpcCallError` with a `denied` cause). Triggers and schedules never carry a scope
   * card.
   */
  readonly scopeCard?: GuestScopeCard
}

/**
 * Options of `create`. `keyed` creates the keyed agent of the constructor input and fails with
 * an `RpcCallError` whose cause is `conflict` when that agent already exists; without `keyed`,
 * every create makes a new agent with a new ID. `config` overrides local configuration values,
 * and `env` adds environment variables to the ones the manifest defines. The new agent never
 * inherits the caller's environment.
 */
export interface CreateOptions<F extends ConfigFields = never> extends ScopeCardOptions {
  readonly keyed?: boolean
  readonly config?: [F] extends [never]
    ? never
    : F extends ConfigFields
      ? NonSecretOverride<F>
      : never
  readonly env?: Readonly<Record<string, string>>
}
export interface DurableClient<
  C extends MethodParams,
  M extends Record<string, AnyMethodSpec>,
  F extends ConfigFields,
> {
  /**
   * Bind the current durable incarnation without creating it or waiting for initialization.
   *
   * Use this to schedule work on self from inside the constructor. Normal `upsert` reuses
   * an initialized agent without rerunning its constructor, but waits for an unfinished
   * constructor. Calling `upsert` on self from that constructor would therefore deadlock.
   *
   * Scheduled work runs after construction finishes. This does not make awaited
   * self-calls reentrant or bypass method permissions. A different client type fails
   * with a typed error. Replay preserves the recorded incarnation.
   */
  readonly bindSelf: (
    options?: ScopeCardOptions,
  ) => Effect.Effect<
    RemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError,
    RpcClient | AgentHostClient | Scope.Scope
  >
  /**
   * Connect to the keyed agent of the constructor input, creating it when it is missing. An
   * initialized agent is reused without rerunning its constructor.
   */
  readonly upsert: (
    input: CallerInput<C>,
    options?: ScopeCardOptions,
  ) => Effect.Effect<
    RemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError,
    RpcClient | Scope.Scope
  >
  /** Create an agent and connect to it after its successful initialization. */
  readonly create: (
    input: CallerInput<C>,
    options?: CreateOptions<F>,
  ) => Effect.Effect<
    RemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    RpcClient | Scope.Scope
  >
  /**
   * Connect to the existing agent with the given ID. It never creates an agent: an unknown ID or
   * an ID of another agent type fails with an `RpcCallError` whose cause is `not-found`.
   */
  readonly getById: (
    id: AgentId,
    options?: ScopeCardOptions,
  ) => Effect.Effect<
    RemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError,
    RpcClient | Scope.Scope
  >
}
export interface EphemeralClient<C extends MethodParams, M extends Record<string, AnyMethodSpec>> {
  /** Connect to the agent type; each invocation runs on a new one-shot instance. */
  readonly upsert: (
    input: CallerInput<C>,
    options?: ScopeCardOptions,
  ) => Effect.Effect<
    EphemeralRemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError,
    RpcClient | Scope.Scope
  >
}
export type AgentClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  Mode extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
> = "ephemeral" extends Mode ? EphemeralClient<C, Methods> : DurableClient<C, Methods, F>

interface WireMethod {
  readonly name: string
  readonly inputCodec: Pick<CompiledInputCodec, "encodeAsync">
  readonly decodeOutput?: (tree: CoreTypes.SchemaValueTree) => Effect.Effect<unknown, unknown, any>
  readonly streaming: boolean
  readonly errorWrapped: boolean
  readonly successVoid: boolean
}

interface CompiledClient<C extends MethodParams = MethodParams> {
  readonly constructorCodec: CompiledInputCodec<C, MethodInput<C>>
  readonly methods: ReadonlyMap<string, WireMethod>
}

export interface MethodOnlyClient<
  Methods extends Record<string, AnyMethodSpec>,
> extends AgentIdBinding<RemoteAgent<Methods>> {
  readonly methods: Methods
}

export type FullClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  Mode extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
> = ClientDefinition<C, Methods, Mode, F> & {
  readonly client: AgentClient<C, Methods, Mode, F>
} & AgentIdBinding<RemoteAgent<Methods>>

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

export interface MethodOnlyDefinition<Methods extends Record<string, AnyMethodSpec>> {
  readonly methods: Methods
  readonly name?: never
  readonly id?: never
  readonly mode?: never
  readonly config?: never
}

export class ClientBindingError {
  readonly _tag = "ClientBindingError"
  constructor(readonly reason: string) {}
}

export const bindAgentId: unique symbol = Symbol.for("effect-golem/client/bind-agent-id")

/** A client definition that connects to an existing agent by its ID. */
export interface AgentIdBinding<
  Client,
  Error = RemoteCallError | UnsupportedSchemaError | ClientBindingError,
  Requirements = RpcClient | AgentHostClient | Scope.Scope,
> {
  readonly [bindAgentId]: (id: AgentId) => Effect.Effect<Client, Error, Requirements>
}

const witAgentId = (id: AgentId) =>
  Effect.try({
    try: () => agentIdToWit(id),
    catch: (cause): RemoteCallError => ({
      _tag: "InvalidUuidError",
      value: String(id?.uuid),
      reason: String(cause),
    }),
  })

/**
 * The deployed agent type that the type tag of `id` names. An ID whose tag names no deployed
 * agent type fails like an unknown ID, with `not-found`.
 */
export const registeredTypeOf = (id: AgentId) =>
  Effect.gen(function* () {
    const raw = yield* witAgentId(id)
    const host = yield* AgentHostClient
    const registered = yield* Effect.try({
      try: () => host.getAgentTypeByAgentId(raw),
      catch: wrapHostThrow,
    })
    if (registered === undefined)
      return yield* Effect.fail<RemoteCallError>({
        _tag: "RpcCallError",
        cause: { tag: "not-found", val: `no deployed agent type has agent ID ${id.uuid}` },
      })
    return { raw, registered }
  })

/** Open a scoped connection to the existing agent with the given ID. */
export const connectById = (agentTypeName: string, id: CoreTypes.AgentId) =>
  Effect.gen(function* () {
    const host = yield* RpcClient
    return yield* Effect.acquireRelease(
      host.getById(agentTypeName, id).pipe(Effect.mapError((error) => wrapHostThrow(error.cause))),
      (connection) => Effect.sync(() => connection.drop()),
    )
  })

/** Read the ID of the connected agent from the host. */
export const connectionAgentId = (rpc: RpcConnection): Effect.Effect<AgentId, RemoteCallError> =>
  Effect.tryPromise({ try: () => rpc.agentId(), catch: wrapHostThrow }).pipe(
    Effect.map(agentIdFromWit),
  )

const decodeOutput = (mc: WireMethod, tree: CoreTypes.SchemaValueTree | undefined) => {
  if (mc.decodeOutput === undefined) {
    return tree === undefined
      ? Effect.succeed(undefined)
      : Effect.fail(responseError(`${mc.name}: expected unit output`))
  }
  if (tree === undefined) return Effect.fail(responseError(`${mc.name}: expected a result value`))
  return Effect.mapError(mc.decodeOutput(tree), (error) =>
    responseError(`${mc.name}: failed to decode output: ${String(error)}`),
  )
}

const finishOutput = (mc: WireMethod, value: unknown) => {
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
  defaultScopeCard: GuestScopeCard | undefined,
) => {
  const remote: Record<string, unknown> = {}
  for (const [name, mc] of compiled.methods) {
    const streaming = mc.streaming
    const rejectNonAwaitedStream = () =>
      Effect.fail(responseError(`${name}: live streams require invoke-and-await`))
    const encode = (input: Record<string, unknown>) =>
      Effect.mapError(mc.inputCodec.encodeAsync(input), (e) =>
        responseError(`${name}: failed to encode input: ${String(e)}`),
      )
    const awaited = (input: Record<string, unknown>, scopeCard: GuestScopeCard | undefined) =>
      awaitInvocation(rpc, name, encode(input), scopeCard).pipe(
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
          Effect.try({
            try: () => invocationMetadataFromHost(rpc.invoke(name, tree)),
            catch: wrapHostThrow,
          }),
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
    const call = (input: Record<string, unknown>) => awaited(input, defaultScopeCard)
    const withScopeCard = (card: GuestScopeCard) => (input: Record<string, unknown>) =>
      awaited(input, card)
    remote[name] = Object.assign(call, { trigger, schedule, withScopeCard })
  }
  if (!ephemeral) remote[agentIdMember] = connectionAgentId(rpc)
  return remote
}

const compileMethods = (methods: Record<string, AnyMethodSpec>) =>
  Effect.gen(function* () {
    const compiled = new Map<string, WireMethod>()
    for (const [name, spec] of Object.entries(methods)) {
      const mc = (yield* compileMethodSpec(name, spec)) as MethodCodec<
        MethodParams,
        MethodSuccess,
        Schema.Top
      >
      const output = mc.outputCodec
      compiled.set(name, {
        name,
        inputCodec: mc.inputCodec,
        decodeOutput: output ? (tree) => decodeFromWire(output.codec, tree) : undefined,
        streaming:
          graphUsesStreams(mc.inputCodec.graph) ||
          (output !== undefined && graphUsesStreams(output.graph)),
        errorWrapped: mc.errorWrapped,
        successVoid: mc.successVoid,
      })
    }
    return compiled as ReadonlyMap<string, WireMethod>
  })

const bindingForDefinition = <Methods extends Record<string, AnyMethodSpec>>(definition: {
  readonly methods: Methods
  readonly name?: string
  readonly mode?: AgentCommon.AgentMode
}): AgentIdBinding<RemoteAgent<Methods>> => {
  let cachedMethods: ReadonlyMap<string, WireMethod> | undefined
  const compile = Effect.suspend(() =>
    cachedMethods === undefined
      ? Effect.map(compileMethods(definition.methods), (methods) => (cachedMethods = methods))
      : Effect.succeed(cachedMethods),
  )
  const bindById = (id: AgentId) =>
    Effect.gen(function* () {
      if (definition.mode === "ephemeral")
        return yield* Effect.fail(
          new ClientBindingError(
            `Cannot bind agent ID '${id.uuid}' to ephemeral agent type '${definition.name}'; use upsert(...)`,
          ),
        )
      let typeName = definition.name
      let raw: CoreTypes.AgentId
      if (typeName === undefined) {
        const resolved = yield* registeredTypeOf(id)
        if (resolved.registered.agentType.mode === "ephemeral")
          return yield* Effect.fail(
            new ClientBindingError(
              `Cannot bind agent ID '${id.uuid}' to ephemeral agent type '${resolved.registered.agentType.typeName}'`,
            ),
          )
        typeName = resolved.registered.agentType.typeName
        raw = resolved.raw
      } else {
        raw = yield* witAgentId(id)
      }
      const methods = yield* compile
      const rpc = yield* connectById(typeName, raw)
      return buildRemote(rpc, { methods }, false, undefined) as RemoteAgent<Methods>
    })
  return { [bindAgentId]: bindById }
}

/** Define a caller-only client without registering an agent. @since 1.6.0 @category constructors */
export function defineAgentClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  F extends ConfigFields = never,
>(
  definition: ClientDefinition<C, Methods, "ephemeral", F> & { readonly mode: "ephemeral" },
): FullClient<C, Methods, "ephemeral", F>
export function defineAgentClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  F extends ConfigFields = never,
>(definition: ClientDefinition<C, Methods, "durable", F>): FullClient<C, Methods, "durable", F>
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
  return Object.freeze({ ...canonical, client, ...bindingForDefinition(canonical) })
}

/**
 * Connect a method-only, full, definition-owned, or reflected client to the existing agent with
 * the given ID. It never creates an agent: an unknown ID fails with `not-found`.
 *
 * @since 1.6.0
 * @category constructors
 */
export const bind = <Client, Error, Requirements>(
  id: AgentId,
  binding: AgentIdBinding<Client, Error, Requirements>,
): Effect.Effect<Client, Error, Requirements> => binding[bindAgentId](id)

/** @internal Build the binding protocol shared by `defineAgent` specs. */
export const bindingFor = <
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  Mode extends AgentCommon.AgentMode,
  F extends ConfigFields,
>(
  definition: AgentMetadata<C, Methods, Mode, F>,
): AgentIdBinding<RemoteAgent<Methods>> => bindingForDefinition(definition)

/** Build a client from the full agent-definition input: `name`, `mode`, `id`, `methods`, and optional `config`. */
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
          const methods = yield* compileMethods(def.methods)
          return (cached = { constructorCodec, methods })
        }),
  )
  const config = (options?: CreateOptions<F>) =>
    Effect.gen(function* () {
      if (options?.config === undefined) return [] as AgentCommon.TypedAgentConfigValue[]
      if (def.config === undefined)
        return yield* Effect.fail(
          new ConfigError([], {
            _tag: "Unsupported",
            reason: `agent '${def.name}' has no config; cannot apply overrides`,
          }),
        )
      return yield* encodeOverrides(
        yield* def.config.__compile(),
        options.config as Record<string, unknown>,
      ).pipe(
        Effect.mapError((error) =>
          error instanceof ConfigError
            ? error
            : responseError(`failed to encode config override: ${String(error)}`),
        ),
      )
    })
  const encodeConstructor = (codecs: CompiledClient<C>, input: CallerInput<C>) =>
    Effect.mapError(codecs.constructorCodec.encodeAsync(input), (e) =>
      responseError(`failed to encode constructor input: ${String(e)}`),
    )
  const connected = (
    acquire: Effect.Effect<RpcConnection, RemoteCallError, RpcClient>,
    codecs: CompiledClient<C>,
    ephemeral: boolean,
    scopeCard: GuestScopeCard | undefined,
  ) =>
    Effect.map(
      Effect.acquireRelease(acquire, (connection) => Effect.sync(() => connection.drop())),
      (rpc) => buildRemote(rpc, codecs, ephemeral, scopeCard),
    )
  const upsert = (input: CallerInput<C>, options?: ScopeCardOptions) =>
    Effect.gen(function* () {
      const codecs = yield* compile
      const constructorTree = yield* encodeConstructor(codecs, input)
      const host = yield* RpcClient
      return yield* connected(
        Effect.mapError(host.upsert(def.name, constructorTree), (e) => wrapHostThrow(e.cause)),
        codecs,
        (def.mode ?? "durable") === "ephemeral",
        options?.scopeCard,
      )
    })
  const create = (input: CallerInput<C>, options?: CreateOptions<F>) =>
    Effect.gen(function* () {
      const codecs = yield* compile
      const agentConfig = yield* config(options)
      const constructorTree = yield* encodeConstructor(codecs, input)
      const host = yield* RpcClient
      return (yield* connected(
        host
          .create(def.name, constructorTree, {
            keyed: options?.keyed ?? false,
            agentConfig,
            env: Object.entries(options?.env ?? {}),
          })
          .pipe(
            Effect.map(({ connection }) => connection),
            Effect.mapError((e) => wrapHostThrow(e.cause)),
          ),
        codecs,
        false,
        options?.scopeCard,
      )) as RemoteAgent<Methods>
    })
  const getById = (id: AgentId, options?: ScopeCardOptions) =>
    Effect.gen(function* () {
      const raw = yield* witAgentId(id)
      const codecs = yield* compile
      const host = yield* RpcClient
      return (yield* connected(
        Effect.mapError(host.getById(def.name, raw), (e) => wrapHostThrow(e.cause)),
        codecs,
        false,
        options?.scopeCard,
      )) as RemoteAgent<Methods>
    })
  const bindSelf = (options?: ScopeCardOptions) =>
    Effect.gen(function* () {
      const host = yield* RpcClient
      const [id, rpc] = yield* Effect.acquireRelease(
        host.bindSelf().pipe(Effect.mapError((error) => wrapHostThrow(error.cause))),
        ([, connection]) => Effect.sync(() => connection.drop()),
      )
      const { registered } = yield* registeredTypeOf(agentIdFromWit(id))
      const typeName = registered.agentType.typeName
      if (typeName !== def.name)
        return yield* Effect.fail<RemoteCallError>({
          _tag: "RpcCallError",
          cause: {
            tag: "remote-agent-error",
            val: {
              tag: "invalid-type",
              val: `self binding expects ${def.name}, the agent type is ${typeName}`,
            },
          },
        })
      const codecs = yield* compile
      return buildRemote(rpc, codecs, false, options?.scopeCard) as RemoteAgent<Methods>
    })
  return (
    (def.mode ?? "durable") === "ephemeral" ? { upsert } : { bindSelf, upsert, create, getById }
  ) as AgentClient<C, Methods, Mode, F>
}
