/** Typed, schema-native RPC clients for agent definitions. @since 1.6.0 */
import { Effect, Result, Schema, Scope } from "effect"
import type * as AgentCommon from "golem:agent/common@2.0.0"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { parseUuid, uuidToString } from "golem:core/types@2.0.0"
import type { AgentMetadata } from "./Agent.js"
import {
  ConfigError,
  encodeOverrides,
  type ConfigFields,
  type NonSecretOverride,
} from "./Config.js"
import * as Datetime from "./Datetime.js"
import { DurabilityModeClient } from "./host/DurabilityModeClient.js"
import { RpcClient, RpcHostError, type RpcConnection } from "./host/RpcClient.js"
import { awaitInvocation, scheduleCancelableInvocation, wrapHostThrow } from "./internal/rpc.js"
import {
  compileMethodSpec,
  compileParamBindings,
  type CompiledInputCodec,
  type MethodCodec,
  type MethodInput,
  type MethodParams,
  type MethodSpec,
  type MethodSuccess,
  type MethodSuccessType,
} from "./Method.js"
import type { SchemaGraph, SchemaType } from "./internal/schema-model/model.js"
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
interface DurableClient<
  C extends MethodParams,
  M extends Record<string, AnyMethodSpec>,
  F extends ConfigFields,
> {
  readonly get: (
    input: MethodInput<C>,
    options?: GetOptions<F>,
  ) => Effect.Effect<
    RemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    RpcClient | Scope.Scope
  >
  readonly getPhantom: (
    input: MethodInput<C>,
    phantomId: string,
    options?: GetOptions<F>,
  ) => Effect.Effect<
    RemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    RpcClient | Scope.Scope
  >
  readonly newPhantom: (
    input: MethodInput<C>,
    options?: GetOptions<F>,
  ) => Effect.Effect<
    PhantomRemoteAgent<M>,
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    DurabilityModeClient | RpcClient | Scope.Scope
  >
}
interface EphemeralClient<
  C extends MethodParams,
  M extends Record<string, AnyMethodSpec>,
  F extends ConfigFields,
> {
  readonly newPhantom: (
    input: MethodInput<C>,
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
          const constructorCodec = yield* compileParamBindings(`${def.name} constructor`, def.id)
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
    input: MethodInput<C>,
    phantom: CoreTypes.Uuid | undefined,
    options?: GetOptions<F>,
  ) =>
    Effect.gen(function* () {
      const codecs = yield* compile
      const overrides = yield* config(options)
      const constructorTree = yield* Effect.mapError(
        codecs.constructorCodec.encodeAsync(input),
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
  const get = (input: MethodInput<C>, options?: GetOptions<F>) =>
    construct(input, undefined, options).pipe(
      Effect.map(({ rpc, codecs }) => buildRemote(rpc, codecs, false) as RemoteAgent<Methods>),
    )
  const getPhantom = (input: MethodInput<C>, id: string, options?: GetOptions<F>) =>
    Effect.try({
      try: () => parseUuid(id),
      catch: (e): RemoteCallError => ({ _tag: "InvalidUuidError", value: id, reason: String(e) }),
    }).pipe(
      Effect.flatMap((uuid) => construct(input, uuid, options)),
      Effect.map(({ rpc, codecs }) => buildRemote(rpc, codecs, false) as RemoteAgent<Methods>),
    )
  const newPhantom = (input: MethodInput<C>, options?: GetOptions<F>) =>
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
    (def.mode ?? "durable") === "ephemeral" ? { newPhantom } : { get, getPhantom, newPhantom }
  ) as AgentClient<C, Methods, Mode, F>
}
