/**
 * @since 1.5.0
 */
import { Effect, Result, Schema } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import type * as AgentHost from "golem:agent/host@1.5.0"
import { parseUuid, uuidToString } from "golem:core/types@1.5.0"
import type { AgentDefinition } from "./Agent.js"
import { DurabilityModeClient } from "./host/DurabilityModeClient.js"
import {
  RpcClient,
  type RpcCancellationToken,
  type RpcConnection,
  type RpcHostError,
} from "./host/RpcClient.js"
import {
  compileMethodSpec,
  compileParamBindings,
  type MethodCodec,
  type MethodInput,
  type MethodParams,
  type MethodSpec,
  type ParamBinding,
} from "./Method.js"
import { type UnsupportedSchemaError } from "./WitCodec.js"
import {
  ConfigError,
  encodeOverrides,
  type ConfigFields,
  type NonSecretOverride,
} from "./Config.js"

type AnyMethodSpec = MethodSpec<any, any, any>

/**
 * Re-exported for users who want to pattern-match on RPC errors.
 *
 * @since 1.5.0
 * @category re-exports
 */
export type RpcError = AgentHost.RpcError

/**
 * Errors a `RemoteMethod` call can produce, before adding any user-typed error.
 *
 * @since 1.5.0
 * @category errors
 */
export type RemoteCallError =
  | { readonly _tag: "RpcCallError"; readonly cause: RpcError }
  | { readonly _tag: "InvalidUuidError"; readonly value: string; readonly reason: string }
  | { readonly _tag: "RemoteResponseError"; readonly reason: string }

const rpcError = (cause: RpcError): RemoteCallError => ({ _tag: "RpcCallError", cause })

/**
 * Closed set of `RpcError["tag"]` values. The `Set<RpcError["tag"]>`
 * constructor type-checks the entries against the WIT-derived union,
 * so the only way an extra tag slips in is a typo. Used by
 * {@link extractRpcError} to gate the structural probe so we don't
 * accidentally classify an unrelated `{tag: "..."}` payload as an
 * `RpcError`.
 *
 * Same shape as `Websocket.WS_TAGS` and `RdbmsShared.RDBMS_ERROR_TAGS`.
 */
const RPC_TAGS = new Set<RpcError["tag"]>([
  "protocol-error",
  "denied",
  "not-found",
  "remote-internal-error",
  "remote-agent-error",
])

const isTaggedRpcError = (e: unknown): e is RpcError => {
  if (e === null || typeof e !== "object") return false
  const obj = e as { tag?: unknown }
  return typeof obj.tag === "string" && RPC_TAGS.has(obj.tag as RpcError["tag"])
}

/**
 * Recover an `RpcError` from a host throw. The wasm-rquickjs host
 * wrappers throw the WIT variant via `ctx.throw(IntoJs::into_js(err))`,
 * which lands on the JS `catch` slot in one of three observed shapes:
 *
 *   1. the bare `{tag, val}` object (synchronous host calls);
 *   2. a JS `Error` whose `.payload` is the bare object (some
 *      rquickjs / wstd async paths re-wrap the exception);
 *   3. a JS `Error` whose `.cause` is the bare object (alternate
 *      wrapper path observed in the wild).
 *
 * Returns `undefined` if the thrown value matches none of those.
 *
 * Mirrors the same probe in `Websocket.extractWsError` and
 * `RdbmsShared.extractTaggedError`.
 */
const extractRpcError = (e: unknown): RpcError | undefined => {
  if (isTaggedRpcError(e)) return e
  if (e instanceof Error) {
    const payload = (e as unknown as { payload?: unknown }).payload
    if (isTaggedRpcError(payload)) return payload
    const cause = (e as unknown as { cause?: unknown }).cause
    if (isTaggedRpcError(cause)) return cause
  }
  return undefined
}

const wrapHostThrow = (e: unknown): RemoteCallError => {
  const rpc = extractRpcError(e)
  if (rpc !== undefined) return rpcError(rpc)
  return rpcError({ tag: "protocol-error", val: e instanceof Error ? e.message : String(e) })
}

/**
 * A handle to a scheduled remote invocation. `cancel` is best-effort: if
 * the scheduled time has already passed and the invocation has started,
 * it is a no-op.
 *
 * @since 1.5.0
 * @category models
 */
export interface ScheduledInvocation {
  readonly cancel: () => Effect.Effect<void>
}

/**
 * The remote counterpart of a single agent method.
 *
 * Three call shapes per method:
 *
 * - **Calling the value as a function** performs an awaited
 *   invocation. The underlying host call is `WasmRpc.asyncInvokeAndAwait`
 *   awaited via `pollable.abortablePromise(signal)`. The returned
 *   `Effect` is **fully interruptible**: interrupting the surrounding
 *   fiber (`Fiber.interrupt`, `Effect.race`, `Effect.timeout`, etc.)
 *   triggers `future-invoke-result.cancel()` on the host, releasing
 *   the calling worker from waiting.
 *
 *   Cancellation is **best-effort by idempotency key**. If the remote
 *   side has not yet started executing, the host removes the
 *   invocation entry; if it has already started, the remote work
 *   continues but its result is dropped on the calling side. Treat
 *   the side effects of the remote call as possibly-already-applied.
 *
 * - **`.trigger(input)`** — fire-and-forget. Calls `WasmRpc.invoke` and
 *   resolves to `void` once the host accepts the request. Nothing to
 *   cancel on the caller side after that.
 *
 * - **`.schedule(scheduledAt, input)`** — schedules the invocation for
 *   later. Returns a {@link ScheduledInvocation} whose `cancel` Effect
 *   calls the host's `cancellation-token.cancel()` (a separate WIT
 *   primitive from the in-flight `future-invoke-result.cancel`).
 *
 * The canonical "future + cancel" pattern — Scala-style
 * `cancelableAwaitWith(...)` — is just `Effect.forkChild(method(input))`
 * plus `Fiber.interrupt(fiber)`; no separate API is required because
 * fiber-interrupt already chains to the host cancel.
 *
 * @since 1.5.0
 * @category models
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

/**
 * A typed remote handle to one agent instance.
 *
 * @since 1.5.0
 * @category models
 */
export type RemoteAgent<Methods extends Record<string, AnyMethodSpec>> = {
  readonly [K in keyof Methods]: Methods[K] extends MethodSpec<infer P, infer S, infer E>
    ? RemoteMethod<P, S, E>
    : never
}

/**
 * Same as {@link RemoteAgent} but additionally carries the generated phantom id.
 *
 * @since 1.5.0
 * @category models
 */
export type PhantomRemoteAgent<Methods extends Record<string, AnyMethodSpec>> =
  RemoteAgent<Methods> & { readonly phantomId: string }

/**
 * Optional knobs accepted by every constructor variant.
 *
 * @since 1.5.0
 * @category models
 */
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
 *
 * @since 1.5.0
 * @category models
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
      // Methods declared with `Schema.Void` success and NO typed error
      // emit an empty tuple on the wire. Methods that DO declare a typed error
      // always carry a 1-element `result<{}, E>` wrapper
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

/**
 * Wrap `WasmRpc.asyncInvokeAndAwait` as a fully-interruptible Effect.
 *
 * Structure:
 *
 * - `acquire` opens the host `future-invoke-result` resource by
 *   calling `asyncInvokeAndAwait`. Sync host throws are mapped through
 *   {@link wrapHostThrow}.
 * - `use` awaits completion via `pollable.abortablePromise(signal)`.
 *   The signal comes straight from `Effect.callback`'s second
 *   parameter; Effect 4 ties it to the surrounding fiber's interrupt
 *   observer. When the fiber is interrupted, the abortable promise
 *   rejects synchronously and the JS-side `.then(...)` chain is
 *   dropped — no leak.
 * - `release` ALWAYS calls `fut.cancel()` on every exit path
 *   (success, failure, defect, interrupt). Per the WIT contract on
 *   `future-invoke-result.cancel`:
 *
 *   > Best-effort attempt to cancel the remote invocation by
 *   > idempotency key. If the invocation has already started or
 *   > completed, this is a no-op.
 *
 *   So the post-success / post-failure cancel is harmless on the host
 *   side. Putting `cancel()` in the uninterruptible `release` clause
 *   guarantees the host is informed even if the user fiber is
 *   interrupted in a strange place.
 */
const asyncInvoke = (
  rpc: RpcConnection,
  methodName: string,
  input: CoreTypes.DataValue,
): Effect.Effect<CoreTypes.DataValue, RemoteCallError> =>
  Effect.acquireUseRelease(
    Effect.try({
      try: () => rpc.asyncInvokeAndAwait(methodName, input),
      catch: wrapHostThrow,
    }),
    (fut) =>
      Effect.callback<CoreTypes.DataValue, RemoteCallError>((resume, signal) => {
        // Guard the setup phase against synchronous throws from
        // `fut.subscribe()` or `pollable.abortablePromise(...)` (a
        // misbehaving host could throw before the promise chain even
        // exists). Without this, the throw escapes the register
        // function and Effect treats it as a defect rather than a
        // typed `RemoteCallError`.
        try {
          const pollable = fut.subscribe()
          pollable
            .abortablePromise(signal)
            .then(() => {
              if (signal.aborted) return
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
              // `abortablePromise` rejects with an `AbortError`-shaped
              // DOMException when `signal` aborts; that path is the
              // fiber-interrupt path and must NOT be reported as a
              // RemoteCallError. Anything else is a host-side throw
              // and is forwarded to the caller.
              if (signal.aborted) return
              resume(Effect.fail(wrapHostThrow(e)))
            })
        } catch (e) {
          if (!signal.aborted) resume(Effect.fail(wrapHostThrow(e)))
        }
      }),
    (fut) =>
      Effect.sync(() => {
        try {
          fut.cancel()
        } catch {
          // best-effort: WIT contract says cancel is fire-and-forget,
          // and any thrown error here is unrecoverable.
        }
      }),
  )

/** Build a single `RemoteMethod` bound to an open {@link RpcConnection}. */
const buildRemoteMethod = (
  rpc: RpcConnection,
  mc: MethodCodec<MethodParams, Schema.Top, Schema.Top>,
): RemoteMethod<MethodParams, Schema.Top, Schema.Top> => {
  const call = (input: Record<string, unknown>) =>
    Effect.flatMap(encodeMethodInput(mc, input), (dv) =>
      Effect.flatMap(asyncInvoke(rpc, mc.name, dv), (out) =>
        // When the method declares a typed error, the wire response is
        // a component-model `result<S, E>`. Decode it via
        // `decodeMethodOutput` (which produces a `Result.Result<S, E>`),
        // then split: success → succeed; failure → typed `Effect.fail`.
        // `AgentError.custom-error` is NOT inspected — typed errors
        // travel exclusively on the success-DataValue's Result wrapper.
        Effect.flatMap(decodeMethodOutput(mc, out), (decoded) => {
          if (!mc.errorWrapped) return Effect.succeed(decoded)
          const r = decoded as Result.Result<unknown, unknown>
          if (Result.isSuccess(r)) {
            // Undo the empty-record stand-in for void-success methods
            // (server-side encodes `undefined` as `{}` so it can ride
            // the component-model `result<_, E>`).
            return Effect.succeed(mc.successVoid ? undefined : r.success)
          }
          return Effect.fail(r.failure) as Effect.Effect<unknown, unknown>
        }),
      ),
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
        (token: RpcCancellationToken): ScheduledInvocation => ({
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
  rpc: RpcConnection,
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
): Effect.Effect<RpcConnection, RemoteCallError, RpcClient> =>
  Effect.gen(function* () {
    const rpc = yield* RpcClient
    return yield* Effect.mapError(
      rpc.connect(agentTypeName, ctorValue, phantomId, agentConfig),
      (e: RpcHostError): RemoteCallError => wrapHostThrow(e.cause),
    )
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
 *
 * @since 1.5.0
 * @category constructors
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
    { rpc: RpcConnection; compiled: CompiledClient },
    RemoteCallError | UnsupportedSchemaError | ConfigError,
    RpcClient
  > =>
    Effect.gen(function* () {
      const compiled = yield* compile
      const ctorValue = yield* encodeConstructor(compiled, input)
      const agentConfig = yield* buildAgentConfig(opts)
      const rpc = yield* constructRpc(def.name, ctorValue, phantomId, agentConfig)
      return { rpc, compiled }
    })

  // Internally `get` / `getPhantom` / `newPhantom` widen `R` to include
  // `RpcClient` (constructor) and additionally `DurabilityModeClient`
  // (idempotency-key generation, only used by `newPhantom`). The public
  // `AgentClient` surface keeps `R = never`; the dispatcher's
  // `provideUserRuntime` (`src/agent.ts`) provides both services via
  // `HostLive`. The cast at the return statement of `clientFor` is the
  // erasure boundary.

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
      const dm = yield* DurabilityModeClient
      const uuid = yield* Effect.try({
        try: () => dm.generateIdempotencyKey(),
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
