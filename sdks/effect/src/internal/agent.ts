/**
 * @since 1.5.0
 */
import { Effect, Exit, Layer, ManagedRuntime, Ref, Schema, Scope } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as ApiHost from "golem:api/host@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import type { DatabaseSync } from "node:sqlite"
import { ElementValueKindError } from "../Element.js"
import { AgentHostClient } from "../host/AgentHostClient.js"
import { EnvironmentClient } from "../host/EnvironmentClient.js"
import { HostLive, type HostServices } from "../host/HostLive.js"
import { SqliteHostExtClient } from "../host/SqliteHostExtClient.js"
import {
  HttpRouteError,
  isStringBindableSchema,
  validateAgentHttp,
  type MethodHttpInput,
  type MountDef,
} from "../Http.js"
import { isMultimodal } from "../Multimodal.js"
import {
  compileMethodSpec,
  compileParamBindings,
  invokeDataValue,
  type Handler,
  type MethodCodec,
  type MethodInput,
  type MethodParams,
  type MethodSpec,
  type ParamBinding,
} from "./method.js"
import { Principal } from "../Principal.js"
import { SelfAgentId } from "../SelfAgentId.js"
import {
  compileSnapshot,
  createBinding,
  InvalidSnapshotError,
  SnapshotDatabaseHasAttachmentsError,
  SnapshotDatabaseMissingPartError,
  SnapshotDatabaseNotInAutocommitError,
  SnapshotDatabaseUnknownPartError,
  SnapshotNotBoundError,
  type BindingHandle,
  type BoundSnapshot,
  type CompiledSnapshot,
  type SnapshotBinding,
  type SnapshotDef,
} from "../Snapshot.js"
import {
  decodeEnvelope,
  encodeBinaryEnvelope,
  encodeJsonEnvelope,
  encodeMultipartJsonEnvelope,
  SnapshotEnvelopeError,
  UnsupportedSnapshotFormatError,
} from "./snapshotEnvelope.js"
import { isElementSpec } from "../Unstructured.js"
import { type UnsupportedSchemaError, type WitCodec } from "../WitCodec.js"
import { clientFor, type AgentClient } from "../Client.js"
import type { CompiledConfig, ConfigClass, ConfigFields, ConfigShape } from "../Config.js"
import * as GolemLogging from "../Logging.js"
import * as GolemTracing from "../Tracing.js"

/**
 * Combined Logger + Tracer layer applied automatically to every piece
 * of user code the dispatcher runs (`impl`, method handlers, custom
 * snapshot save/load). Routes `Effect.log*` to `wasi:logging` and
 * `Effect.withSpan` to `golem:api/context`.
 */
const observabilityLayer = Layer.mergeAll(GolemLogging.layer, GolemTracing.layer)

/**
 * The runtime layer applied to every piece of user code the dispatcher
 * runs. Provides:
 * - All host services (env, config, …) via {@link HostLive} so user
 *   effects that yield from a host-service tag (e.g. `ConfigClient`)
 *   resolve against the real WIT specifier in production and against
 *   test fakes in tests.
 * - {@link observabilityLayer} on top, so `Effect.log*` /
 *   `Effect.withSpan` route through the host bindings.
 *
 * `Layer.provideMerge` is used (not `Layer.mergeAll`) so that
 * {@link observabilityLayer}'s dependency on `LoggingHost` and
 * `TracingHost` is satisfied by `HostLive`. The result also re-merges
 * `HostLive` into its output, so user code that yields a host-service
 * tag directly resolves the same way.
 *
 * Built once at module load (matches the previous `observabilityLayer`
 * pattern); per-invocation cost is zero allocations.
 */
const userRuntimeLayer = Layer.provideMerge(observabilityLayer, HostLive)

/**
 * Module-level `ManagedRuntime` backed by {@link userRuntimeLayer}.
 * Built lazily on first use; reused across every `dispatch*` entry
 * point (`initialize`, `invoke`, `save-snapshot`, `load-snapshot`).
 * This is what makes the "host services constructed once per process"
 * claim mechanically true: `Layer.scoped` acquire-counters fire
 * exactly once across multiple invocations.
 */
const userRuntime = ManagedRuntime.make(userRuntimeLayer)

/**
 * Run a user effect on the cached {@link userRuntime}. Reuses the
 * pre-built layer context across every `dispatch*` entry point, so
 * host services aren't reconstructed per dispatch.
 *
 * `withInvocationParent` is applied *first* (widening the inner
 * effect's `R` channel with `TracingHost`); the runtime then
 * fulfills `TracingHost` via `HostLive`. This is required because
 * `withInvocationParent` reads the host's invocation context via the
 * `TracingHost` service.
 */
const runUserPromise = <A, E, R>(eff: Effect.Effect<A, E, R>): Promise<A> =>
  userRuntime.runPromise(GolemTracing.withInvocationParent(eff) as Effect.Effect<A, E, never>)

type AnyMethodSpec = MethodSpec<any, any, any>

interface ParamCodec {
  readonly name: string
  readonly codec: WitCodec<Schema.Top>
}

/**
 * Per-instance handlers derived from a record of method specs. Each
 * handler takes its decoded input record and returns an Effect of the
 * declared success/error. The optional `CfgTag` parameter widens every
 * handler's required-services slot to include an agent-supplied config
 * service tag (provided by the dispatcher); defaults to `never` for
 * agents without `config`.
 *
 * @since 1.5.0
 * @category models
 */
export type Handlers<Methods extends Record<string, AnyMethodSpec>, CfgTag = never> = {
  readonly [K in keyof Methods]: Handler<Methods[K], CfgTag>
}

/**
 * Optional `config` field on an {@link AgentDefinition}. Built with
 * {@link defineConfig}; serves as both an Effect-Context tag (yieldable
 * to its compiled `ConfigShape`) and a carrier for the field schema
 * record so {@link clientFor} can derive a typed `overrides` channel.
 *
 * The structural shape is intentionally loose so that user-extended
 * classes — `class MyCfg extends defineConfig(...) {}` — remain
 * assignable. The runtime tag identity comes from the unique
 * `KeyClass` minted by `Context.Service` for each `defineConfig` call.
 *
 * @since 1.5.0
 * @category models
 */
export type ConfigDef<F extends ConfigFields> = ConfigClass<F>

/**
 * Compute the `CfgTag` (R-slot identity) carried by a config field
 * record. `never` for agents without a `config` field — that collapses
 * the union below back to `Scope.Scope | Principal`.
 *
 * @since 1.5.0
 * @category models
 */
export type CfgTagOf<F> = [F] extends [never]
  ? never
  : F extends ConfigFields
    ? ConfigShape<F>
    : never

/**
 * Conditional `impl` parameter list. When `S` is `never` (no
 * `snapshot` field on the agent), `impl` takes only the decoded
 * constructor input. When `S` is a {@link SnapshotDef}, `impl` takes a
 * second argument: the per-instance {@link SnapshotBinding} that
 * lets it `init` (auto) or `register` (custom) the snapshot source.
 *
 * The optional `CfgTag` parameter widens the `R` channel of the
 * binding's user-supplied custom save/load effects to `Principal |
 * CfgTag`. Defaults to `never`, so agents without a `config:` field
 * keep the original `R = Principal` exactly. The dispatcher always
 * provides the matching services at runtime.
 *
 * @since 1.5.0
 * @category models
 */
export type ImplArgs<C extends MethodParams, S, CfgTag = never> = [S] extends [never]
  ? readonly [input: MethodInput<C>]
  : readonly [input: MethodInput<C>, snapshot: SnapshotBinding<S, Principal | CfgTag>]

/**
 * A user-defined agent: a named bundle of method *specs* (no bodies) plus
 * an `impl` that, given the decoded constructor input, returns an Effect
 * producing per-instance handlers.
 *
 * `impl` runs in the agent's lifetime `Scope`, so resources allocated
 * with `Effect.acquireRelease` are released when the agent shuts down.
 *
 * The optional `F` (config-fields) generic threads a config service tag
 * through impl's required-services slot AND every Handler. Defaults to
 * `never` so agents without `config` keep the pre-existing API exactly.
 *
 * The optional `S` (snapshot) generic captures the agent's
 * snapshot-definition shape — `Snapshot.define({ schema, policy })` or
 * `Snapshot.custom({ policy })`. When present, `impl` receives a second
 * `SnapshotBinding<S>` argument and the agent's WIT
 * `snapshotting` metadata is `enabled` rather than `disabled`.
 *
 * @since 1.5.0
 * @category models
 */
export interface AgentDefinition<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode = AgentCommon.AgentMode,
  F extends ConfigFields = never,
  S extends SnapshotDef = never,
> {
  readonly name: string
  readonly description?: string
  /** Optional `prompt-hint`, surfaced as `agent-constructor.prompt-hint`. */
  readonly promptHint?: string
  readonly mode?: M // defaults to "durable"
  readonly constructorParams: C
  readonly methods: Methods
  /**
   * Optional HTTP mount declaration. When present, this agent is exposed
   * via the Golem host's HTTP server under the declared path prefix.
   * Type-level constraint: every `{var}` in the mount path must be a
   * `keyof C` (a constructor parameter name).
   */
  readonly http?: MountDef<keyof C & string>
  /**
   * Optional Effect-Context-based config service. Built with
   * {@link defineConfig}. When present, the dispatcher fetches each
   * declared field from the host on demand and provides the matching
   * service for the duration of an `initialize` / `invoke` call.
   */
  readonly config?: ConfigDef<F>
  /**
   * Optional snapshot definition. Built with `Snapshot.define(...)` for
   * the schema-driven auto path or `Snapshot.custom(...)` for the
   * user-managed path. When present, the dispatcher passes a
   * {@link SnapshotBinding} to `impl` as its second argument, and the
   * agent type's `snapshotting` metadata reflects the configured policy.
   */
  readonly snapshot?: S
  /**
   * Constructor effect. Runs once per agent instance, in the agent's
   * lifetime `Scope`. May depend on {@link Principal} (provided by the
   * dispatcher with the value the host passed to `initialize`) and on
   * the optional config service. When `snapshot` is set, `impl` receives
   * a second `SnapshotBinding<S>` argument.
   */
  readonly impl: (
    ...args: ImplArgs<C, S, CfgTagOf<F>>
  ) => Effect.Effect<
    Handlers<Methods, CfgTagOf<F>>,
    unknown,
    Scope.Scope | Principal | HostServices | CfgTagOf<F>
  >
}

/**
 * The value returned by {@link defineAgent}: the original definition plus
 * a derived `client` namespace for connecting to remote instances of this
 * agent type via the Golem RPC host. The `client` shape depends on the
 * agent's `mode`:
 *
 * - durable agents expose `get`, `getPhantom`, and `newPhantom`.
 * - ephemeral agents expose only `getPhantom` and `newPhantom`.
 *
 * @since 1.5.0
 * @category models
 */
export type DefinedAgent<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
  S extends SnapshotDef = never,
> = AgentDefinition<C, Methods, M, F, S> & {
  readonly client: AgentClient<C, Methods, M, F>
}

/**
 * Define an agent type and register it eagerly with the runtime.
 *
 * **Details**
 *
 * Eager registration ensures simply importing an agent module makes
 * the type discoverable by the host — no separate `registerAgent`
 * call at the component entrypoint is required. The returned value
 * mirrors the input definition and additionally exposes a typed
 * {@link clientFor} `client` namespace for connecting to remote
 * instances of this agent type.
 *
 * @see {@link registerAgent} for the lower-level registration-only
 *      entry point.
 *
 * @since 1.5.0
 * @category constructors
 */
export const defineAgent = <
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode = "durable",
  F extends ConfigFields = never,
  S extends SnapshotDef = never,
>(
  def: AgentDefinition<C, Methods, M, F, S>,
): DefinedAgent<C, Methods, M, F, S> => {
  // Register eagerly so simply importing an agent module makes it
  // discoverable by the runtime — no separate `registerAgent` call is
  // required at the component entrypoint.
  Effect.runSync(registerAgent(def))
  // The client view ignores the snapshot definition; erase `S` here so
  // `clientFor` can stay snapshot-agnostic.
  const clientDef = def as unknown as AgentDefinition<C, Methods, M, F>
  return { ...def, client: clientFor(clientDef) }
}

interface CompiledAgent {
  readonly name: string
  readonly definition: AgentDefinition<
    MethodParams,
    Record<string, AnyMethodSpec>,
    AgentCommon.AgentMode,
    never,
    SnapshotDef
  >
  readonly constructorBindings: ReadonlyArray<ParamBinding>
  /** Backwards-compatible legacy view: only component-model wire bindings. */
  readonly constructorCodecs: ReadonlyArray<ParamCodec>
  readonly methodCodecs: ReadonlyMap<string, MethodCodec<MethodParams, Schema.Top, Schema.Top>>
  readonly agentType: AgentCommon.AgentType
  /** Compiled config bundle when `def.config` is set; `null` otherwise. */
  readonly compiledConfig: CompiledConfig | null
  /** Compiled snapshot bundle when `def.snapshot` is set; `null` otherwise. */
  readonly compiledSnapshot: CompiledSnapshot | null
}

/** Module-level registry of compiled agents, keyed by `typeName`. */
const registry = new Map<string, CompiledAgent>()

/**
 * Register an agent definition with the runtime. Pure schema-walking work
 * — no `impl` is executed here, no per-instance state is created. Safe to
 * call at deploy time for type discovery.
 *
 * @see {@link defineAgent} for the eager-registration shorthand that
 *      additionally returns a typed RPC client.
 *
 * @since 1.5.0
 * @category constructors
 */
export const registerAgent = <
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode = AgentCommon.AgentMode,
  F extends ConfigFields = never,
  S extends SnapshotDef = never,
>(
  def: AgentDefinition<C, Methods, M, F, S>,
): Effect.Effect<void, UnsupportedSchemaError | HttpRouteError | InvalidSnapshotError> =>
  Effect.gen(function* () {
    const constructorBindings = (yield* compileParamBindings(
      `${def.name} constructor`,
      def.constructorParams,
    )) as Array<ParamBinding>

    const constructorWire = constructorBindings.filter(
      (b): b is Extract<ParamBinding, { kind: "wire" }> => b.kind === "wire",
    )
    // Backwards-compatible component-model only view.
    const constructorCodecs: Array<ParamCodec> = constructorWire
      .filter((b): b is typeof b & { witCodec: WitCodec<Schema.Top> } => b.witCodec !== null)
      .map((b) => ({ name: b.name, codec: b.witCodec }))

    const methodCodecs = new Map<string, MethodCodec<MethodParams, Schema.Top, Schema.Top>>()
    const methodHttpInputs: Array<MethodHttpInput> = []
    for (const [methodName, spec] of Object.entries(def.methods)) {
      const mc = (yield* compileMethodSpec(methodName, spec)) as MethodCodec<
        MethodParams,
        Schema.Top,
        Schema.Top
      >
      methodCodecs.set(methodName, mc)
      methodHttpInputs.push({
        name: methodName,
        params: spec.params,
        endpoints: spec.http ?? [],
        nonStringBindableParams: collectNonStringBindableParams(spec.params),
        stringBindableParams: collectStringBindableParams(spec.params),
      })
    }

    const constructorSchema: AgentCommon.DataSchema = {
      tag: "tuple",
      val: constructorWire.map((b) => [b.name, b.element.elementSchema]),
    }

    // Validate + compile HTTP routes (mount + per-method endpoints).
    const compiledHttp = yield* validateAgentHttp({
      agentName: def.name,
      mount: def.http,
      constructorParamNames: Object.keys(def.constructorParams),
      nonStringBindableConstructorParams: collectNonStringBindableParams(def.constructorParams),
      methods: methodHttpInputs,
    })

    // Now build the AgentMethod records, attaching the compiled
    // httpEndpoint list per method.
    const agentMethods: Array<AgentCommon.AgentMethod> = []
    for (const [methodName, spec] of Object.entries(def.methods)) {
      const mc = methodCodecs.get(methodName)!
      const eps = compiledHttp.endpoints.get(methodName) ?? []
      agentMethods.push({
        name: methodName,
        description: spec.description ?? "",
        httpEndpoint: [...eps],
        promptHint: spec.promptHint,
        inputSchema: mc.inputSchema,
        outputSchema: mc.outputSchema,
      })
    }

    let compiledConfig: CompiledConfig | null = null
    let configDeclarations: Array<AgentCommon.AgentConfigDeclaration> = []
    if (def.config !== undefined) {
      const cc = yield* def.config.__compile()
      compiledConfig = cc
      configDeclarations = [...cc.declarations]
    }

    let compiledSnapshot: CompiledSnapshot | null = null
    let snapshotting: AgentCommon.Snapshotting = { tag: "disabled" }
    if (def.snapshot !== undefined) {
      const cs = yield* compileSnapshot(def.name, def.snapshot)
      compiledSnapshot = cs
      snapshotting = { tag: "enabled", val: cs.witConfig }
    }

    const agentType: AgentCommon.AgentType = {
      typeName: def.name,
      description: def.description ?? "",
      sourceLanguage: "typescript",
      constructor: {
        description: "",
        promptHint: def.promptHint,
        inputSchema: constructorSchema,
      },
      methods: agentMethods,
      dependencies: [],
      mode: def.mode ?? "durable",
      httpMount: compiledHttp.mount,
      snapshotting,
      config: configDeclarations,
    }

    registry.set(def.name, {
      name: def.name,
      definition: def as unknown as AgentDefinition<
        MethodParams,
        Record<string, AnyMethodSpec>,
        AgentCommon.AgentMode,
        never,
        SnapshotDef
      >,
      constructorBindings,
      constructorCodecs,
      methodCodecs,
      agentType,
      compiledConfig,
      compiledSnapshot,
    })
  })

const collectNonStringBindableParams = (
  params: Readonly<Record<string, unknown>>,
): ReadonlySet<string> => {
  const out = new Set<string>()
  for (const [name, p] of Object.entries(params)) {
    if (isMultimodal(p) || isElementSpec(p)) {
      out.add(name)
    }
  }
  return out
}

const collectStringBindableParams = (
  params: Readonly<Record<string, unknown>>,
): ReadonlySet<string> => {
  const out = new Set<string>()
  for (const [name, p] of Object.entries(params)) {
    if (isMultimodal(p) || isElementSpec(p)) continue
    // Only Schema.Top values can be string-bindable.
    if (p && typeof p === "object" && "ast" in (p as object)) {
      if (isStringBindableSchema(p as Schema.Top)) {
        out.add(name)
      }
    }
  }
  return out
}

/** The currently-initialised agent in this container, if any. */
interface ActiveAgent {
  readonly name: string
  readonly scope: Scope.Closeable
  readonly handlers: Readonly<Record<string, Handler<AnyMethodSpec>>>
  /** Principal supplied to `initialize` / embedded in the loaded snapshot. */
  readonly principal: AgentCommon.Principal
  /**
   * Structured `AgentId` for this running instance. Captured once via
   * `getSelfMetadata` during initialize/load and reused for the
   * lifetime of the agent. Exposed to user code as the
   * {@link SelfAgentId} Context service.
   */
  readonly selfAgentId: CoreTypes.AgentId
  /**
   * Captured per-instance snapshot binding when the agent declared a
   * `snapshot` field; `null` otherwise. Read by `dispatchSaveSnapshot`
   * and written-through by `dispatchLoadSnapshot`.
   */
  readonly snapshot: BoundSnapshot | null
}

let activeAgent: ActiveAgent | null = null

/**
 * Close the active agent's scope (if any) so the next test can call
 * `initialize` again. The registry of `defineAgent`-registered types
 * is left intact (those are populated at module import time).
 *
 * @internal
 * @since 1.5.0
 */
export const __resetAgents = async (): Promise<void> => {
  if (activeAgent !== null) {
    await Effect.runPromise(Scope.close(activeAgent.scope, Exit.void))
    activeAgent = null
  }
}

/** Decode an incoming constructor-input `DataValue` into a record of
 *  decoded parameter values, in the same way both `initialize` and
 *  `load` need to. */
const decodeConstructorInput = async (
  agentTypeName: string,
  compiled: CompiledAgent,
  input: CoreTypes.DataValue,
): Promise<Record<string, unknown>> => {
  if (input.tag !== "tuple") {
    throw new Error(`${agentTypeName} constructor: expected tuple DataValue, got ${input.tag}`)
  }
  const wireBindings = compiled.constructorBindings.filter(
    (b): b is Extract<ParamBinding, { kind: "wire" }> => b.kind === "wire",
  )
  if (input.val.length !== wireBindings.length) {
    throw new Error(
      `${agentTypeName} constructor: expected ${wireBindings.length} argument(s), got ${input.val.length}`,
    )
  }
  const constructorInput: Record<string, unknown> = {}
  for (let i = 0; i < wireBindings.length; i++) {
    const b = wireBindings[i]!
    const ev = input.val[i]!
    constructorInput[b.name] = await Effect.runPromise(
      Effect.mapError(b.element.decode(ev), (err) =>
        err instanceof ElementValueKindError
          ? new Error(
              `${agentTypeName} constructor: argument ${i} (${b.name}) is ${err.actual}, expected ${err.expected}`,
            )
          : err,
      ) as Effect.Effect<unknown, unknown, never>,
    )
  }
  return constructorInput
}

/**
 * Open a fresh scope, run `impl` with the supplied principal/config
 * service, and return the resulting handlers + (optional) bound
 * snapshot. Used by both `dispatchInitialize` and `dispatchLoadSnapshot`
 * so the two share a single code path for everything except how state
 * is restored after `impl` returns.
 */
const initAgentInstance = async (
  agentTypeName: string,
  compiled: CompiledAgent,
  constructorInput: Record<string, unknown>,
  principal: AgentCommon.Principal,
): Promise<{
  scope: Scope.Closeable
  handlers: Record<string, Handler<AnyMethodSpec>>
  bindingHandle: BindingHandle | null
  selfAgentId: CoreTypes.AgentId
}> => {
  const bindingHandle: BindingHandle | null =
    compiled.compiledSnapshot !== null
      ? createBinding(agentTypeName, compiled.compiledSnapshot)
      : null

  // Capture the structured AgentId once at agent-init time. Subsequent
  // user-side reads via the `SelfAgentId` Context service are free.
  // Read via the `AgentHostClient` host service so tests can substitute
  // a fake without monkey-patching the real specifier.
  let selfAgentId: CoreTypes.AgentId
  try {
    selfAgentId = await runUserPromise(
      Effect.gen(function* () {
        const c = yield* AgentHostClient
        return c.getSelfMetadata().agentId
      }),
    )
  } catch (e) {
    throw new Error(
      `failed to fetch self metadata while initializing agent '${agentTypeName}': ${
        e instanceof Error ? e.message : String(e)
      }`,
    )
  }

  const scope = await Effect.runPromise(Scope.make())
  let handlers: Record<string, Handler<AnyMethodSpec>>
  try {
    const implArgs: Array<unknown> = [constructorInput]
    if (bindingHandle !== null) implArgs.push(bindingHandle.binding)
    let program = (
      (compiled.definition.impl as (...a: ReadonlyArray<unknown>) => unknown)(
        ...implArgs,
      ) as Effect.Effect<
        Record<string, Handler<AnyMethodSpec>>,
        unknown,
        Scope.Scope | Principal | SelfAgentId
      >
    ).pipe(
      Effect.provideService(Principal, principal),
      Effect.provideService(SelfAgentId, selfAgentId),
    )
    if (compiled.compiledConfig !== null && compiled.definition.config !== undefined) {
      const shape = await runUserPromise(compiled.compiledConfig.buildShape())
      program = (program as Effect.Effect<unknown, unknown, never>).pipe(
        // The config class is a Context.Service tag (Self/Identifier
        // resolved via the user's `defineConfig`-class declaration). We
        // erase the static generics here because the dispatcher works
        // generically over every registered agent.
        Effect.provideService(compiled.definition.config as never, shape as never),
      ) as typeof program
    }
    // Use `Scope.provide` (NOT `Scope.use`) — the latter auto-closes
    // the scope when `program` finishes, which would tear down any
    // resources `impl` opened (e.g. SqliteClient handles) before the
    // agent's first method invocation.
    handlers = (await runUserPromise(Scope.provide(program, scope))) as Record<
      string,
      Handler<AnyMethodSpec>
    >
  } catch (e) {
    // Initialization failed; close the scope to release anything that
    // managed to be acquired before the failure.
    await Effect.runPromise(Scope.close(scope, Exit.void))
    throw e
  }

  return { scope, handlers, bindingHandle, selfAgentId }
}

/**
 * Implementation of `agent-guest.guest.initialize`.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
export const dispatchInitialize = async (
  agentTypeName: string,
  input: CoreTypes.DataValue,
  principal: AgentCommon.Principal,
): Promise<void> => {
  const compiled = registry.get(agentTypeName)
  if (!compiled) {
    throw new Error(
      `unknown agent: ${agentTypeName}; registered: ${[...registry.keys()].join(", ") || "<none>"}`,
    )
  }
  if (activeAgent !== null) {
    throw new Error(`agent already initialized in this container: ${activeAgent.name}`)
  }

  const constructorInput = await decodeConstructorInput(agentTypeName, compiled, input)
  const { scope, handlers, bindingHandle, selfAgentId } = await initAgentInstance(
    agentTypeName,
    compiled,
    constructorInput,
    principal,
  )

  let snapshot: BoundSnapshot | null = null
  if (compiled.compiledSnapshot !== null) {
    const bound = bindingHandle!.read()
    if (bound === null) {
      await Effect.runPromise(Scope.close(scope, Exit.void))
      throw new SnapshotNotBoundError(agentTypeName)
    }
    snapshot = bound
  }

  activeAgent = { name: agentTypeName, scope, handlers, principal, selfAgentId, snapshot }
}

/**
 * Implementation of `agent-guest.guest.invoke`.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
export const dispatchInvoke = async (
  methodName: string,
  input: CoreTypes.DataValue,
  principal: AgentCommon.Principal,
): Promise<CoreTypes.DataValue> => {
  if (activeAgent === null) {
    throw new Error(`agent is not initialized; cannot invoke ${methodName}`)
  }
  const compiled = registry.get(activeAgent.name)!
  const mc = compiled.methodCodecs.get(methodName)
  if (!mc) {
    throw new Error(
      `unknown method '${methodName}' on agent '${activeAgent.name}'; available: ${[...compiled.methodCodecs.keys()].join(", ") || "<none>"}`,
    )
  }
  const handler = activeAgent.handlers[methodName]
  if (!handler) {
    throw new Error(
      `agent '${activeAgent.name}' did not provide an implementation for '${methodName}'`,
    )
  }
  // Provide the per-call principal as an Effect service so handler
  // bodies can read the *current* caller via `yield* Principal` (which
  // may differ from the initialize-time principal stored on
  // `activeAgent`). The optional config service is rebuilt fresh per
  // invocation: regular fields are memoized for the duration of THIS
  // call only; secret fields are never cached.
  let program: Effect.Effect<CoreTypes.DataValue, unknown, never> = invokeDataValue(
    mc,
    handler,
    input,
  ).pipe(
    Effect.provideService(Principal, principal),
    Effect.provideService(SelfAgentId, activeAgent.selfAgentId),
  ) as Effect.Effect<CoreTypes.DataValue, unknown, never>
  if (compiled.compiledConfig !== null && compiled.definition.config !== undefined) {
    const shape = await runUserPromise(compiled.compiledConfig.buildShape())
    program = program.pipe(
      Effect.provideService(compiled.definition.config as never, shape as never),
    ) as typeof program
  }
  return await runUserPromise(program)
}

/**
 * Implementation of `agent-guest.guest.discoverAgentTypes`.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
export const dispatchDiscoverAgentTypes = async (): Promise<Array<AgentCommon.AgentType>> =>
  Array.from(registry.values()).map((c) => c.agentType)

/**
 * Implementation of `agent-guest.guest.getDefinition`.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
export const dispatchGetDefinition = async (): Promise<AgentCommon.AgentType> => {
  if (activeAgent === null) {
    throw new Error("agent is not initialized; cannot get definition")
  }
  return registry.get(activeAgent.name)!.agentType
}

// ---------------------------------------------------------------------------
// Snapshotting dispatchers
// ---------------------------------------------------------------------------

/**
 * Resolve the `SqliteHostExtClient` impl out of the production
 * {@link userRuntimeLayer} synchronously. The save-snapshot dispatcher's
 * auto+sqlite path is fully synchronous on purpose (see the long
 * comment at the top of {@link dispatchSaveSnapshot}), so this helper
 * has to be too. `userRuntime.runSync` is safe here because the
 * underlying layers (`SqliteHostExtLive` etc.) are pure
 * `Layer.succeed`s — no async work happens during resolution.
 */
const resolveSqliteHostExtSync = (): {
  serializeDatabaseSync: (db: DatabaseSync) => Uint8Array
  restoreDatabaseSync: (db: DatabaseSync, bytes: Uint8Array) => void
  isAutocommitDatabaseSync: (db: DatabaseSync) => boolean
} =>
  userRuntime.runSync(
    Effect.gen(function* () {
      const ext = yield* SqliteHostExtClient
      return {
        serializeDatabaseSync: ext.serializeDatabaseSync,
        restoreDatabaseSync: ext.restoreDatabaseSync,
        isAutocommitDatabaseSync: ext.isAutocommitDatabaseSync,
      }
    }),
  )

/**
 * Encode the auto-snapshot path synchronously. Pulled out as a
 * standalone helper so {@link dispatchSaveSnapshot} can return its
 * result *without* a wrapping `async` (which would force the
 * wasm-rquickjs runtime to await a Promise — see the dispatcher
 * comment for why that matters).
 */
const encodeAutoSnapshot = (
  agent: ActiveAgent,
  snap: Extract<BoundSnapshot, { kind: "auto" }>,
): ApiHost.Snapshot => {
  const state = Effect.runSync(Ref.get(snap.ref) as Effect.Effect<unknown, never>)
  const encoded = Effect.runSync(
    Schema.encodeUnknownEffect(snap.schema)(state) as Effect.Effect<unknown, Schema.SchemaError>,
  )
  if (snap.declaredDatabases.length === 0) {
    return encodeJsonEnvelope(agent.principal, encoded)
  }
  const sqliteExt = resolveSqliteHostExtSync()
  const dbParts: Array<{ name: string; bytes: Uint8Array }> = []
  for (const dbName of snap.declaredDatabases) {
    const handle = snap.databases.get(dbName)
    if (handle === undefined) {
      throw new SnapshotDatabaseMissingPartError(agent.name, dbName, "save")
    }
    if (!sqliteExt.isAutocommitDatabaseSync(handle)) {
      throw new SnapshotDatabaseNotInAutocommitError(agent.name, dbName)
    }
    const rows = handle.prepare("PRAGMA database_list").all() as Array<{ name?: string }>
    const extra = rows
      .map((r) => String(r.name ?? ""))
      .filter((n) => n !== "main" && n !== "temp" && n !== "")
    if (extra.length > 0) {
      throw new SnapshotDatabaseHasAttachmentsError(agent.name, dbName, extra)
    }
    dbParts.push({ name: dbName, bytes: sqliteExt.serializeDatabaseSync(handle) })
  }
  return encodeMultipartJsonEnvelope(agent.principal, encoded, dbParts)
}

/**
 * Implementation of `golem:api/save-snapshot.save`. Reads the active
 * agent's bound snapshot state, encodes it according to the active
 * variant (auto → JSON envelope or multipart/mixed when SQLite databases
 * are declared; custom → binary v2 envelope), and returns the resulting
 * `Snapshot` to the host.
 *
 * **Why the auto path is intentionally NOT `async`.** Empirically, the
 * previous `async` implementation (which used `await runUserPromise(...)`
 * to resolve the SqliteHostExt service) caused the host to trap with
 * `wasm trap: cannot enter component instance` on every Nth invocation
 * once `Snapshot.policy.everyN(N)` triggered a save. The trap landed
 * in the oplog as an `ERROR` with `retry from: <previous-invoke-index>`
 * and no `SNAPSHOT` entry was ever written; the host then waited a few
 * seconds and retried the next invoke, so the user-visible state was
 * preserved but no snapshot was captured.
 *
 * Note: this is NOT a host-side concurrency race. Golem's invocation
 * loop strictly serializes invoke / save-snapshot calls — the next
 * call only starts after the previous one fully returns (including
 * all JS Promise resolution). So the trap originates *inside* the
 * `save-snapshot.save` call itself, not from a parallel `invoke`
 * arriving concurrently.
 *
 * The fix is to make the auto path execute as a single synchronous
 * JS frame: the JS function returns a plain `Snapshot` value (not a
 * Promise), so the wasm-rquickjs runtime takes the `non-Promise`
 * branch in `call_js_export_internal` and never has to drive a JS
 * Promise to completion across host imports. With this change the
 * trap stops reproducing and the multipart `SNAPSHOT` entry is
 * recorded normally — the SqliteCounter integration case asserts
 * exactly this.
 *
 * The exact host-side reason `async` save + at-least-one-host-import
 * combined to trap "cannot enter component instance" is not yet
 * pinned down; both `host.currentContext()` (called by
 * `withInvocationParent` inside the old `runUserPromise`) and the
 * Promise return shape of the JS export are involved in the bad
 * path, and removing both was sufficient to make the trap go away.
 *
 * The custom path still has to await the user's `Effect<Uint8Array,
 * ...>` save handler, so it remains `async`. No integration case
 * currently exercises a long-running custom save handler; if a
 * trap shows up there too, we'll need to either restrict the user
 * handler shape or push the host investigation further.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
export const dispatchSaveSnapshot = (): ApiHost.Snapshot | Promise<ApiHost.Snapshot> => {
  if (activeAgent === null) {
    throw new Error("agent is not initialized; cannot save snapshot")
  }
  const agent = activeAgent
  const compiled = registry.get(agent.name)!
  if (compiled.compiledSnapshot === null || agent.snapshot === null) {
    throw new Error(
      `agent '${agent.name}' did not declare a snapshot definition; the host should not be calling save`,
    )
  }
  const snap = agent.snapshot
  if (snap.kind === "auto") {
    return encodeAutoSnapshot(agent, snap)
  }
  return dispatchSaveCustomSnapshot(agent, compiled, snap)
}

/**
 * Custom (`Snapshot.custom(...)`) save path: must run the user's
 * `Effect<Uint8Array, ...>` handler under the same runtime layer the
 * dispatcher uses for `invoke`, so it stays inherently async. Pulled
 * into a standalone async helper so {@link dispatchSaveSnapshot}
 * itself can stay non-async for the auto path (see the long comment
 * on the dispatcher).
 */
const dispatchSaveCustomSnapshot = async (
  agent: ActiveAgent,
  compiled: CompiledAgent,
  snap: Extract<BoundSnapshot, { kind: "custom" }>,
): Promise<ApiHost.Snapshot> => {
  let saveProgram: Effect.Effect<Uint8Array, unknown, never> = snap.handlers.save.pipe(
    Effect.provideService(Principal, agent.principal),
  ) as Effect.Effect<Uint8Array, unknown, never>
  // Mirror `dispatchInvoke`: when the agent declares a config service,
  // make it available to the user's custom save handler too. Auto
  // snapshots don't run user code here, so they don't need this branch.
  if (compiled.compiledConfig !== null && compiled.definition.config !== undefined) {
    const shape = await runUserPromise(compiled.compiledConfig.buildShape())
    saveProgram = saveProgram.pipe(
      Effect.provideService(compiled.definition.config as never, shape as never),
    ) as Effect.Effect<Uint8Array, unknown, never>
  }
  const bytes = await runUserPromise(saveProgram)
  return encodeBinaryEnvelope(agent.principal, bytes)
}

/**
 * Implementation of `golem:api/load-snapshot.load`. The host calls
 * this *instead of* `agent-guest.guest.initialize` when restoring an
 * agent from a snapshot. We:
 *
 * 1. Recover the agent's own ID from `GOLEM_AGENT_ID` and parse it.
 * 2. Decode the envelope to recover principal + user-state bytes.
 * 3. Run the constructor (the same path `initialize` would have taken).
 * 4. Apply the restored state on top of the freshly-constructed
 *    instance (auto → write the Ref; custom → invoke the user's
 *    `load` Effect).
 * 5. Mark the agent active so subsequent `invoke`s see the restored
 *    state.
 *
 * @since 1.5.0
 * @category runtime hooks
 */
export const dispatchLoadSnapshot = async (snapshot: ApiHost.Snapshot): Promise<void> => {
  if (activeAgent !== null) {
    throw new Error(`agent already initialized in this container: ${activeAgent.name}`)
  }

  // 1. Recover the agent ID + parse it. Reading the WASI process env
  //    via the `EnvironmentClient` host service and parsing it via
  //    `AgentHostClient.parseAgentId` lets tests substitute fakes
  //    without monkey-patching the real specifier.
  const { env, parseAgentId } = await runUserPromise(
    Effect.gen(function* () {
      const ec = yield* EnvironmentClient
      const ah = yield* AgentHostClient
      const env = yield* ec.getEnvironment
      return { env, parseAgentId: ah.parseAgentId }
    }),
  )
  const agentIdEntry = env.find(([k]) => k === "GOLEM_AGENT_ID")
  if (agentIdEntry === undefined) {
    throw new Error("load-snapshot: GOLEM_AGENT_ID is not set in the process environment")
  }
  const agentIdString = agentIdEntry[1]
  const [agentTypeName, ctorDataValue] = parseAgentId(agentIdString)

  const compiled = registry.get(agentTypeName)
  if (!compiled) {
    throw new Error(
      `load-snapshot: unknown agent type '${agentTypeName}'; registered: ${[...registry.keys()].join(", ") || "<none>"}`,
    )
  }
  if (compiled.compiledSnapshot === null) {
    throw new Error(`load-snapshot: agent '${agentTypeName}' did not declare a snapshot definition`)
  }

  // 2. Decode the envelope. We use `anonymous` as the fallback principal
  //    only for the legacy v1 binary format which carries no embedded
  //    principal — every modern envelope embeds one.
  const fallbackPrincipal: AgentCommon.Principal = { tag: "anonymous" }
  let decoded: ReturnType<typeof decodeEnvelope>
  try {
    decoded = decodeEnvelope(snapshot, fallbackPrincipal)
  } catch (e) {
    if (e instanceof SnapshotEnvelopeError || e instanceof UnsupportedSnapshotFormatError) {
      throw e
    }
    throw e
  }
  const principal = decoded.principal

  // 3. Run the constructor with the recovered params + principal.
  const constructorInput = await decodeConstructorInput(
    agentTypeName,
    compiled,
    ctorDataValue as CoreTypes.DataValue,
  )
  const { scope, handlers, bindingHandle, selfAgentId } = await initAgentInstance(
    agentTypeName,
    compiled,
    constructorInput,
    principal,
  )

  const bound = bindingHandle!.read()
  if (bound === null) {
    await Effect.runPromise(Scope.close(scope, Exit.void))
    throw new SnapshotNotBoundError(agentTypeName)
  }

  // 4. Apply restored state.
  try {
    if (bound.kind === "auto") {
      const declared = bound.declaredDatabases
      if (declared.length === 0) {
        if (decoded.kind !== "json") {
          throw new SnapshotEnvelopeError(
            `agent '${agentTypeName}' expects a JSON envelope but received ${snapshot.mimeType}`,
          )
        }
        const decodedState = await Effect.runPromise(
          Schema.decodeUnknownEffect(bound.schema)(decoded.state) as Effect.Effect<
            unknown,
            Schema.SchemaError
          >,
        )
        await Effect.runPromise(Ref.set(bound.ref, decodedState) as Effect.Effect<void, never>)
      } else {
        if (decoded.kind !== "multipart") {
          throw new SnapshotEnvelopeError(
            `agent '${agentTypeName}' expects a multipart/mixed envelope (declared databases: ${declared.join(", ")}) but received ${snapshot.mimeType}`,
          )
        }
        // Strict part validation: every declared name must be present
        // exactly once; no unknown names allowed.
        const declaredSet = new Set(declared)
        const seen = new Set<string>()
        for (const part of decoded.databases) {
          if (!declaredSet.has(part.name)) {
            throw new SnapshotDatabaseUnknownPartError(agentTypeName, part.name)
          }
          if (seen.has(part.name)) {
            throw new SnapshotEnvelopeError(
              `multipart envelope: duplicate db part 'db:${part.name}'`,
            )
          }
          seen.add(part.name)
        }
        for (const dbName of declared) {
          if (!seen.has(dbName)) {
            throw new SnapshotDatabaseMissingPartError(agentTypeName, dbName, "load")
          }
        }
        // Validate that the user attached every declared database.
        for (const dbName of declared) {
          if (!bound.databases.has(dbName)) {
            throw new SnapshotDatabaseMissingPartError(agentTypeName, dbName, "save")
          }
        }
        // Restore each DB in place via the wasm-rquickjs extension.
        const sqliteExt = resolveSqliteHostExtSync()
        for (const part of decoded.databases) {
          const handle = bound.databases.get(part.name)!
          sqliteExt.restoreDatabaseSync(handle, part.bytes)
        }
        const decodedState = await Effect.runPromise(
          Schema.decodeUnknownEffect(bound.schema)(decoded.state) as Effect.Effect<
            unknown,
            Schema.SchemaError
          >,
        )
        await Effect.runPromise(Ref.set(bound.ref, decodedState) as Effect.Effect<void, never>)
      }
    } else {
      if (decoded.kind !== "binary") {
        throw new SnapshotEnvelopeError(
          `agent '${agentTypeName}' expects a binary envelope but received ${snapshot.mimeType}`,
        )
      }
      let loadProgram: Effect.Effect<void, unknown, never> = bound.handlers
        .load(decoded.userPayload)
        .pipe(Effect.provideService(Principal, principal)) as Effect.Effect<void, unknown, never>
      // Mirror the save path + `dispatchInvoke`: when the agent declares
      // a config service, make it available to the user's custom load
      // handler too.
      if (compiled.compiledConfig !== null && compiled.definition.config !== undefined) {
        const shape = await runUserPromise(compiled.compiledConfig.buildShape())
        loadProgram = loadProgram.pipe(
          Effect.provideService(compiled.definition.config as never, shape as never),
        ) as Effect.Effect<void, unknown, never>
      }
      await runUserPromise(loadProgram)
    }
  } catch (e) {
    await Effect.runPromise(Scope.close(scope, Exit.void))
    throw e
  }

  // 5. Publish.
  activeAgent = {
    name: agentTypeName,
    scope,
    handlers,
    principal,
    selfAgentId,
    snapshot: bound,
  }
}
