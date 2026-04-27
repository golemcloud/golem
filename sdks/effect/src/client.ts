import { Effect, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import type * as AgentHost from "golem:agent/host@1.5.0"
import { CancellationToken as HostCancellationToken, WasmRpc } from "golem:agent/host@1.5.0"
import { generateIdempotencyKey } from "golem:api/host@1.5.0"
import { parseUuid, uuidToString } from "golem:core/types@1.5.0"
import type { AgentDefinition } from "./agent.js"
import {
  compileMethodSpec,
  compileParamBindings,
  type MethodCodec,
  type MethodInput,
  type MethodParams,
  type MethodSpec,
  type ParamBinding,
} from "./method.js"
import { type UnsupportedSchemaError } from "./wit-codec.js"
import {
  ConfigError,
  encodeOverrides,
  type ConfigFields,
  type NonSecretOverride,
} from "./config.js"

type AnyMethodSpec = MethodSpec<any, any, any>

/** Re-exported for users who want to pattern-match on RPC errors. */
export type RpcError = AgentHost.RpcError

/** Errors a `RemoteMethod` call can produce, before adding any user-typed error. */
export type RemoteCallError =
  | { readonly _tag: "RpcCallError"; readonly cause: RpcError }
  | { readonly _tag: "InvalidUuidError"; readonly value: string; readonly reason: string }
  | { readonly _tag: "RemoteResponseError"; readonly reason: string }

const rpcError = (cause: RpcError): RemoteCallError => ({ _tag: "RpcCallError", cause })

const wrapHostThrow = (e: unknown): RemoteCallError => {
  // The host wraps RpcError in a JS Error; if it's already shaped like
  // an RpcError, pass it through; otherwise treat it as a protocol-level
  // error.
  if (
    typeof e === "object" &&
    e !== null &&
    "tag" in e &&
    typeof (e as { tag: unknown }).tag === "string"
  ) {
    return rpcError(e as RpcError)
  }
  return rpcError({ tag: "protocol-error", val: e instanceof Error ? e.message : String(e) })
}

/**
 * A handle to a scheduled remote invocation. `cancel` is best-effort: if
 * the scheduled time has already passed and the invocation has started,
 * it is a no-op.
 */
export interface ScheduledInvocation {
  readonly cancel: () => Effect.Effect<void>
}

/**
 * The remote counterpart of a single agent method.
 *
 * Calling the value as a function performs an awaited invocation. The
 * underlying host call (`asyncInvokeAndAwait` + `Pollable`) is integrated
 * with Effect interruption — interrupting the fiber best-effort cancels
 * the in-flight invocation on the remote side.
 */
export interface RemoteMethod<
  Params extends MethodParams,
  Success extends Schema.Top,
  Error extends Schema.Top,
> {
  (input: MethodInput<Params>): Effect.Effect<Success["Type"], RemoteCallError | Error["Type"]>
  readonly trigger: (input: MethodInput<Params>) => Effect.Effect<void, RemoteCallError>
  readonly schedule: (
    scheduledAt: AgentHost.Datetime,
    input: MethodInput<Params>,
  ) => Effect.Effect<ScheduledInvocation, RemoteCallError>
}

/** A typed remote handle to one agent instance. */
export type RemoteAgent<Methods extends Record<string, AnyMethodSpec>> = {
  readonly [K in keyof Methods]: Methods[K] extends MethodSpec<infer P, infer S, infer E>
    ? RemoteMethod<P, S, E>
    : never
}

/** Same as {@link RemoteAgent} but additionally carries the generated phantom id. */
export type PhantomRemoteAgent<Methods extends Record<string, AnyMethodSpec>> =
  RemoteAgent<Methods> & { readonly phantomId: string }

/** Optional knobs accepted by every constructor variant. */
export interface GetOptions<F extends ConfigFields = never> {
  readonly agentConfig?: ReadonlyArray<AgentCommon.TypedAgentConfigValue>
  /**
   * Per-call overrides for non-secret config values. Schema-driven and
   * structurally `Partial`; secret leaves (`Schema.Redacted(...)`) are
   * stripped at compile time and a runtime guard rejects them too.
   *
   * Encoded into `TypedAgentConfigValue[]` and concatenated to
   * `agentConfig` before the WasmRpc constructor runs.
   */
  readonly overrides?: [F] extends [never]
    ? never
    : F extends ConfigFields
      ? NonSecretOverride<F>
      : never
}

interface DurableClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  F extends ConfigFields = never,
> {
  readonly get: (
    input: MethodInput<C>,
    opts?: GetOptions<F>,
  ) => Effect.Effect<RemoteAgent<Methods>, RemoteCallError | UnsupportedSchemaError | ConfigError>
  readonly getPhantom: (
    input: MethodInput<C>,
    phantomId: string,
    opts?: GetOptions<F>,
  ) => Effect.Effect<RemoteAgent<Methods>, RemoteCallError | UnsupportedSchemaError | ConfigError>
  readonly newPhantom: (
    input: MethodInput<C>,
    opts?: GetOptions<F>,
  ) => Effect.Effect<
    PhantomRemoteAgent<Methods>,
    RemoteCallError | UnsupportedSchemaError | ConfigError
  >
}

interface EphemeralClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  F extends ConfigFields = never,
> {
  readonly getPhantom: (
    input: MethodInput<C>,
    phantomId: string,
    opts?: GetOptions<F>,
  ) => Effect.Effect<RemoteAgent<Methods>, RemoteCallError | UnsupportedSchemaError | ConfigError>
  readonly newPhantom: (
    input: MethodInput<C>,
    opts?: GetOptions<F>,
  ) => Effect.Effect<
    PhantomRemoteAgent<Methods>,
    RemoteCallError | UnsupportedSchemaError | ConfigError
  >
}

/**
 * Mode-conditional client surface for an agent definition. Ephemeral
 * agents are not addressable by constructor arguments alone, so `get`
 * is hidden at the type level; only `getPhantom` and `newPhantom`
 * remain.
 */
export type AgentClient<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
> = "ephemeral" extends M ? EphemeralClient<C, Methods, F> : DurableClient<C, Methods, F>

interface CompiledClient {
  readonly constructorBindings: ReadonlyArray<ParamBinding>
  readonly methodCodecs: ReadonlyMap<string, MethodCodec<MethodParams, Schema.Top, Schema.Top>>
}

/** Lazily compile a definition's codecs (constructor + methods), then cache. */
const makeCompiler = (
  def: AgentDefinition<MethodParams, Record<string, AnyMethodSpec>>,
): Effect.Effect<CompiledClient, UnsupportedSchemaError> => {
  let cached: CompiledClient | null = null
  return Effect.suspend(() => {
    if (cached !== null) return Effect.succeed(cached)
    return Effect.gen(function* () {
      const constructorBindings = (yield* compileParamBindings(
        `${def.name} constructor`,
        def.constructorParams,
      )) as ReadonlyArray<ParamBinding>
      const methodCodecs = new Map<string, MethodCodec<MethodParams, Schema.Top, Schema.Top>>()
      for (const [name, spec] of Object.entries(def.methods)) {
        const mc = (yield* compileMethodSpec(name, spec)) as MethodCodec<
          MethodParams,
          Schema.Top,
          Schema.Top
        >
        methodCodecs.set(name, mc)
      }
      cached = { constructorBindings, methodCodecs }
      return cached
    })
  })
}

const wireBindingsOf = (bs: ReadonlyArray<ParamBinding>) =>
  bs.filter((b): b is Extract<ParamBinding, { kind: "wire" }> => b.kind === "wire")

/** Encode a list of named bindings + an input record into a `DataValue.tuple`. */
const encodeBindings = (
  context: string,
  bindings: ReadonlyArray<ParamBinding>,
  input: Record<string, unknown>,
): Effect.Effect<CoreTypes.DataValue, RemoteCallError> =>
  Effect.gen(function* () {
    const elements: Array<CoreTypes.ElementValue> = []
    for (const b of wireBindingsOf(bindings)) {
      const ev = yield* Effect.mapError(
        b.element.encode(input[b.name] as never),
        (e): RemoteCallError => ({
          _tag: "RemoteResponseError",
          reason: `failed to encode ${context} '${b.name}': ${String(e)}`,
        }),
      )
      elements.push(ev)
    }
    return { tag: "tuple", val: elements } as CoreTypes.DataValue
  })

/** Encode the constructor's named-input record positionally to a Golem `DataValue`. */
const encodeConstructor = (
  compiled: CompiledClient,
  input: Record<string, unknown>,
): Effect.Effect<CoreTypes.DataValue, RemoteCallError> =>
  encodeBindings("constructor argument", compiled.constructorBindings, input)

/** Encode a method's named-input record positionally to a Golem `DataValue`. */
const encodeMethodInput = (
  mc: MethodCodec<MethodParams, Schema.Top, Schema.Top>,
  input: Record<string, unknown>,
): Effect.Effect<CoreTypes.DataValue, RemoteCallError> => {
  const mm = mc.bindings.find(
    (b): b is Extract<ParamBinding, { kind: "multimodal" }> => b.kind === "multimodal",
  )
  if (mm !== undefined) {
    return Effect.mapError(
      mm.multimodal.encode(input[mm.name]),
      (e): RemoteCallError => ({
        _tag: "RemoteResponseError",
        reason: `${mc.name}: failed to encode multimodal '${mm.name}': ${String(e)}`,
      }),
    )
  }
  return encodeBindings(`${mc.name} argument`, mc.bindings, input)
}

/** Decode a method's `DataValue` response into the success type. */
const decodeMethodOutput = (
  mc: MethodCodec<MethodParams, Schema.Top, Schema.Top>,
  output: CoreTypes.DataValue,
): Effect.Effect<unknown, RemoteCallError> =>
  Effect.gen(function* () {
    if (output.tag !== "tuple") {
      return yield* Effect.fail<RemoteCallError>({
        _tag: "RemoteResponseError",
        reason: `${mc.name}: expected tuple DataValue, got ${output.tag}`,
      })
    }
    if (mc.outputElement === null) {
      if (output.val.length !== 0) {
        return yield* Effect.fail<RemoteCallError>({
          _tag: "RemoteResponseError",
          reason: `${mc.name}: expected empty tuple, got ${output.val.length} elements`,
        })
      }
      return undefined
    }
    if (output.val.length !== 1) {
      return yield* Effect.fail<RemoteCallError>({
        _tag: "RemoteResponseError",
        reason: `${mc.name}: expected 1 element, got ${output.val.length}`,
      })
    }
    const elem = output.val[0]!
    return yield* Effect.mapError(
      mc.outputElement.decode(elem),
      (e): RemoteCallError => ({
        _tag: "RemoteResponseError",
        reason: `${mc.name}: failed to decode output: ${String(e)}`,
      }),
    )
  })

/** Wrap `WasmRpc.asyncInvokeAndAwait` as an interruptible Effect. */
const asyncInvoke = (
  rpc: WasmRpc,
  methodName: string,
  input: CoreTypes.DataValue,
): Effect.Effect<CoreTypes.DataValue, RemoteCallError> =>
  Effect.flatMap(
    Effect.try({
      try: () => rpc.asyncInvokeAndAwait(methodName, input),
      catch: wrapHostThrow,
    }),
    (fut) =>
      Effect.callback<CoreTypes.DataValue, RemoteCallError>((resume) => {
        let cancelled = false
        const pollable = fut.subscribe()
        pollable
          .promise()
          .then(() => {
            if (cancelled) return
            const result = fut.get()
            if (result === undefined) {
              resume(
                Effect.fail<RemoteCallError>({
                  _tag: "RemoteResponseError",
                  reason: `${methodName}: pollable signalled ready but result is missing`,
                }),
              )
              return
            }
            if (result.tag === "ok") resume(Effect.succeed(result.val))
            else resume(Effect.fail(rpcError(result.val)))
          })
          .catch((e: unknown) => {
            if (cancelled) return
            resume(Effect.fail(wrapHostThrow(e)))
          })
        return Effect.sync(() => {
          cancelled = true
          try {
            fut.cancel()
          } catch {
            // best-effort
          }
        })
      }),
  )

/** Build a single `RemoteMethod` bound to an open `WasmRpc` handle. */
const buildRemoteMethod = (
  rpc: WasmRpc,
  mc: MethodCodec<MethodParams, Schema.Top, Schema.Top>,
): RemoteMethod<MethodParams, Schema.Top, Schema.Top> => {
  const call = (input: Record<string, unknown>) =>
    Effect.flatMap(encodeMethodInput(mc, input), (dv) =>
      Effect.flatMap(asyncInvoke(rpc, mc.name, dv), (out) => decodeMethodOutput(mc, out)),
    )
  const trigger = (input: Record<string, unknown>) =>
    Effect.flatMap(encodeMethodInput(mc, input), (dv) =>
      Effect.try({
        try: () => rpc.invoke(mc.name, dv),
        catch: wrapHostThrow,
      }),
    )
  const schedule = (scheduledAt: AgentHost.Datetime, input: Record<string, unknown>) =>
    Effect.flatMap(encodeMethodInput(mc, input), (dv) =>
      Effect.map(
        Effect.try({
          try: () => rpc.scheduleCancelableInvocation(scheduledAt, mc.name, dv),
          catch: wrapHostThrow,
        }),
        (token: HostCancellationToken): ScheduledInvocation => ({
          cancel: () =>
            Effect.sync(() => {
              try {
                token.cancel()
              } catch {
                // best-effort
              }
            }),
        }),
      ),
    )
  const fn = ((input: Record<string, unknown>) => call(input)) as RemoteMethod<
    MethodParams,
    Schema.Top,
    Schema.Top
  >
  ;(fn as { trigger: typeof trigger }).trigger = trigger
  ;(fn as { schedule: typeof schedule }).schedule = schedule
  return fn
}

/** Build the full `RemoteAgent` surface for the given compiled methods. */
const buildRemoteAgent = (
  rpc: WasmRpc,
  compiled: CompiledClient,
): Record<string, RemoteMethod<MethodParams, Schema.Top, Schema.Top>> => {
  const out: Record<string, RemoteMethod<MethodParams, Schema.Top, Schema.Top>> = {}
  for (const [name, mc] of compiled.methodCodecs) {
    out[name] = buildRemoteMethod(rpc, mc)
  }
  return out
}

const constructRpc = (
  agentTypeName: string,
  ctorValue: CoreTypes.DataValue,
  phantomId: CoreTypes.Uuid | undefined,
  agentConfig: ReadonlyArray<AgentCommon.TypedAgentConfigValue>,
): Effect.Effect<WasmRpc, RemoteCallError> =>
  Effect.try({
    try: () =>
      new WasmRpc(
        agentTypeName,
        ctorValue,
        phantomId,
        agentConfig as Array<AgentCommon.TypedAgentConfigValue>,
      ),
    catch: wrapHostThrow,
  })

const parsePhantomId = (id: string): Effect.Effect<CoreTypes.Uuid, RemoteCallError> =>
  Effect.try({
    try: () => parseUuid(id),
    catch: (e): RemoteCallError => ({
      _tag: "InvalidUuidError",
      value: id,
      reason: e instanceof Error ? e.message : String(e),
    }),
  })

/**
 * Build an {@link AgentClient} for the given agent definition. Codecs are
 * compiled lazily on first use and cached for the lifetime of the
 * returned client. The client does not require the agent to also be
 * `registerAgent`'d in the same component — pure consumers can use it
 * standalone.
 */
export const clientFor = <
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
>(
  def: AgentDefinition<C, Methods, M, F>,
): AgentClient<C, Methods, M, F> => {
  const compile = makeCompiler(def as AgentDefinition<MethodParams, Record<string, AnyMethodSpec>>)

  /**
   * Encode any user-supplied non-secret overrides through the
   * agent-definition's compiled config table, then concatenate them
   * onto the explicit `agentConfig` array. Secret leaves are rejected
   * defensively at runtime by {@link encodeOverrides}.
   */
  const buildAgentConfig = (
    opts: GetOptions<F> | undefined,
  ): Effect.Effect<
    Array<AgentCommon.TypedAgentConfigValue>,
    RemoteCallError | UnsupportedSchemaError | ConfigError
  > =>
    Effect.gen(function* () {
      const overridesArr: Array<AgentCommon.TypedAgentConfigValue> = []
      const overrides = (opts?.overrides ?? undefined) as Record<string, unknown> | undefined
      if (overrides !== undefined) {
        if (def.config === undefined) {
          return yield* Effect.fail<ConfigError>(
            new ConfigError([], {
              _tag: "Unsupported",
              reason: `agent '${def.name}' has no config; cannot apply overrides`,
            }),
          )
        }
        const compiledCfg = yield* def.config.__compile()
        const encoded = (yield* Effect.mapError(
          encodeOverrides(compiledCfg, overrides),
          (e): RemoteCallError | ConfigError =>
            e instanceof ConfigError
              ? e
              : {
                  _tag: "RemoteResponseError",
                  reason: `failed to encode overrides: ${String(e)}`,
                },
        )) as ReadonlyArray<AgentCommon.TypedAgentConfigValue>
        overridesArr.push(...encoded)
      }
      return [...(opts?.agentConfig ?? []), ...overridesArr]
    })

  const construct = (
    input: Record<string, unknown>,
    phantomId: CoreTypes.Uuid | undefined,
    opts: GetOptions<F> | undefined,
  ): Effect.Effect<
    { rpc: WasmRpc; compiled: CompiledClient },
    RemoteCallError | UnsupportedSchemaError | ConfigError
  > =>
    Effect.gen(function* () {
      const compiled = yield* compile
      const ctorValue = yield* encodeConstructor(compiled, input)
      const agentConfig = yield* buildAgentConfig(opts)
      const rpc = yield* constructRpc(def.name, ctorValue, phantomId, agentConfig)
      return { rpc, compiled }
    })

  const get = (input: MethodInput<C>, opts?: GetOptions<F>) =>
    Effect.map(
      construct(input as Record<string, unknown>, undefined, opts),
      ({ rpc, compiled }) => buildRemoteAgent(rpc, compiled) as RemoteAgent<Methods>,
    )

  const getPhantom = (input: MethodInput<C>, phantomId: string, opts?: GetOptions<F>) =>
    Effect.gen(function* () {
      const uuid = yield* parsePhantomId(phantomId)
      const { rpc, compiled } = yield* construct(input as Record<string, unknown>, uuid, opts)
      return buildRemoteAgent(rpc, compiled) as RemoteAgent<Methods>
    })

  const newPhantom = (input: MethodInput<C>, opts?: GetOptions<F>) =>
    Effect.gen(function* () {
      const uuid = yield* Effect.try({
        try: () => generateIdempotencyKey(),
        catch: wrapHostThrow,
      })
      const { rpc, compiled } = yield* construct(input as Record<string, unknown>, uuid, opts)
      const remote = buildRemoteAgent(rpc, compiled) as RemoteAgent<Methods>
      ;(remote as PhantomRemoteAgent<Methods> & { phantomId: string }).phantomId =
        uuidToString(uuid)
      return remote as PhantomRemoteAgent<Methods>
    })

  const mode: AgentCommon.AgentMode = def.mode ?? "durable"
  if (mode === "ephemeral") {
    return { getPhantom, newPhantom } as AgentClient<C, Methods, M, F>
  }
  return { get, getPhantom, newPhantom } as AgentClient<C, Methods, M, F>
}
