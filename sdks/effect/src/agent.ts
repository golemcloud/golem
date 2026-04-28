import { Effect, Exit, Layer, Ref, Schema, Scope } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import * as ApiHost from "golem:api/host@1.5.0"
import * as AgentHost from "golem:agent/host@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
import type { DatabaseSync } from "node:sqlite"
import * as NodeSqlite from "node:sqlite"
import * as WasiEnv from "wasi:cli/environment@0.2.3"
import { ElementValueKindError } from "./element.js"
import {
  HttpRouteError,
  isStringBindableSchema,
  validateAgentHttp,
  type MethodHttpInput,
  type MountDef,
} from "./http.js"
import { isMultimodal } from "./multimodal.js"
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
import { Principal } from "./principal.js"
import { SelfAgentId } from "./self-agent-id.js"
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
} from "./snapshot.js"
import {
  decodeEnvelope,
  encodeBinaryEnvelope,
  encodeJsonEnvelope,
  encodeMultipartJsonEnvelope,
  SnapshotEnvelopeError,
  UnsupportedSnapshotFormatError,
} from "./snapshot-envelope.js"
import { isElementSpec } from "./unstructured.js"
import { type UnsupportedSchemaError, type WitCodec } from "./wit-codec.js"
import { clientFor, type AgentClient } from "./client.js"
import type { CompiledConfig, ConfigClass, ConfigFields, ConfigShape } from "./config.js"
import * as GolemLogging from "./logging.js"
import * as GolemTracing from "./tracing.js"

/**
 * Combined Logger + Tracer layer applied automatically to every piece
 * of user code the dispatcher runs (`impl`, method handlers, custom
 * snapshot save/load). Routes `Effect.log*` to `wasi:logging` and
 * `Effect.withSpan` to `golem:api/context`.
 */
const observabilityLayer = Layer.mergeAll(GolemLogging.layer, GolemTracing.layer)

/**
 * Provide the host-backed Logger + Tracer to a user effect, then chain
 * its span tree under the live host invocation context. Best-effort;
 * never affects business logic on host failure.
 */
const provideObservability = <A, E, R>(eff: Effect.Effect<A, E, R>): Effect.Effect<A, E, R> =>
  GolemTracing.withInvocationParent(Effect.provide(eff, observabilityLayer))

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
 */
export type ConfigDef<F extends ConfigFields> = ConfigClass<F>

/**
 * Compute the `CfgTag` (R-slot identity) carried by a config field
 * record. `never` for agents without a `config` field — that collapses
 * the union below back to `Scope.Scope | Principal`.
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
 */
export type ImplArgs<C extends MethodParams, S> = [S] extends [never]
  ? readonly [input: MethodInput<C>]
  : readonly [input: MethodInput<C>, snapshot: SnapshotBinding<S>]

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
    ...args: ImplArgs<C, S>
  ) => Effect.Effect<Handlers<Methods, CfgTagOf<F>>, unknown, Scope.Scope | Principal | CfgTagOf<F>>
}

/**
 * The value returned by {@link defineAgent}: the original definition plus
 * a derived `client` namespace for connecting to remote instances of this
 * agent type via the Golem RPC host. The `client` shape depends on the
 * agent's `mode`:
 *
 * - durable agents expose `get`, `getPhantom`, and `newPhantom`.
 * - ephemeral agents expose only `getPhantom` and `newPhantom`.
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

/** Close the active agent's scope (if any) so the next test can call
 *  `initialize` again. The registry of `defineAgent`-registered types is
 *  left intact (those are populated at module import time). */
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
  let selfAgentId: CoreTypes.AgentId
  try {
    selfAgentId = getSelfMetadataImpl().agentId
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
      const shape = await Effect.runPromise(
        compiled.compiledConfig.buildShape() as Effect.Effect<unknown, never>,
      )
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
    handlers = (await Effect.runPromise(
      provideObservability(Scope.provide(program, scope)),
    )) as Record<string, Handler<AnyMethodSpec>>
  } catch (e) {
    // Initialization failed; close the scope to release anything that
    // managed to be acquired before the failure.
    await Effect.runPromise(Scope.close(scope, Exit.void))
    throw e
  }

  return { scope, handlers, bindingHandle, selfAgentId }
}

/** Implementation of `agent-guest.guest.initialize`. */
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

/** Implementation of `agent-guest.guest.invoke`. */
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
    const shape = await Effect.runPromise(
      compiled.compiledConfig.buildShape() as Effect.Effect<unknown, never>,
    )
    program = program.pipe(
      Effect.provideService(compiled.definition.config as never, shape as never),
    ) as typeof program
  }
  return await Effect.runPromise(provideObservability(program))
}

/** Implementation of `agent-guest.guest.discoverAgentTypes`. */
export const dispatchDiscoverAgentTypes = async (): Promise<Array<AgentCommon.AgentType>> =>
  Array.from(registry.values()).map((c) => c.agentType)

/** Implementation of `agent-guest.guest.getDefinition`. */
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
 * Module-level indirection so tests can swap the host's
 * `wasi:cli/environment.getEnvironment` binding without monkey-patching
 * the imported namespace. Used by `load-snapshot.load` to read
 * `GOLEM_AGENT_ID` from the process environment.
 */
let getEnvironmentImpl: () => Array<[string, string]> = () => WasiEnv.getEnvironment()

/** Test-only hook: replace the host `getEnvironment` shim. */
export const __setGetEnvironmentForTest = (fn: () => Array<[string, string]>): void => {
  getEnvironmentImpl = fn
}

/** Reset the env shim back to the real `wasi:cli/environment` binding. */
export const __resetGetEnvironmentForTest = (): void => {
  getEnvironmentImpl = () => WasiEnv.getEnvironment()
}

/**
 * Module-level indirection so tests can swap the host's
 * `golem:agent/host.parseAgentId` binding.
 */
let parseAgentIdImpl: (
  agentId: string,
) => [string, AgentCommon.DataValue, CoreTypes.Uuid | undefined] = (id) =>
  AgentHost.parseAgentId(id)

/** Test-only hook: replace the host `parseAgentId` shim. */
export const __setParseAgentIdForTest = (
  fn: (agentId: string) => [string, AgentCommon.DataValue, CoreTypes.Uuid | undefined],
): void => {
  parseAgentIdImpl = fn
}

/** Reset the parse-agent-id shim back to the real binding. */
export const __resetParseAgentIdForTest = (): void => {
  parseAgentIdImpl = (id) => AgentHost.parseAgentId(id)
}

/**
 * Module-level indirection for the host's `getSelfMetadata` binding,
 * used at agent-init time to capture the structured `SelfAgentId`
 * service value. Tests can swap this out via
 * `__setGetSelfMetadataForTest` to avoid pulling in the real host
 * import.
 */
let getSelfMetadataImpl: () => ApiHost.AgentMetadata = () => ApiHost.getSelfMetadata()

/** Test-only hook: replace the host `getSelfMetadata` shim. */
export const __setGetSelfMetadataForTest = (fn: () => ApiHost.AgentMetadata): void => {
  getSelfMetadataImpl = fn
}

/** Reset the `getSelfMetadata` shim back to the real binding. */
export const __resetGetSelfMetadataForTest = (): void => {
  getSelfMetadataImpl = () => ApiHost.getSelfMetadata()
}

/**
 * Module-level indirections for the three wasm-rquickjs extensions to
 * `node:sqlite` (`serializeDatabaseSync` / `restoreDatabaseSync` /
 * `isAutocommitDatabaseSync`). Tests can swap these out via
 * `__setSerializeDatabaseSyncForTest` etc., mirroring the pattern used
 * for `getEnvironment` / `parseAgentId`.
 */
let serializeDatabaseSyncImpl: (db: DatabaseSync) => Uint8Array = (db) =>
  NodeSqlite.serializeDatabaseSync(db)
let restoreDatabaseSyncImpl: (db: DatabaseSync, bytes: Uint8Array) => void = (db, bytes) =>
  NodeSqlite.restoreDatabaseSync(db, bytes)
let isAutocommitDatabaseSyncImpl: (db: DatabaseSync) => boolean = (db) =>
  NodeSqlite.isAutocommitDatabaseSync(db)

export const __setSerializeDatabaseSyncForTest = (fn: (db: DatabaseSync) => Uint8Array): void => {
  serializeDatabaseSyncImpl = fn
}
export const __resetSerializeDatabaseSyncForTest = (): void => {
  serializeDatabaseSyncImpl = (db) => NodeSqlite.serializeDatabaseSync(db)
}
export const __setRestoreDatabaseSyncForTest = (
  fn: (db: DatabaseSync, bytes: Uint8Array) => void,
): void => {
  restoreDatabaseSyncImpl = fn
}
export const __resetRestoreDatabaseSyncForTest = (): void => {
  restoreDatabaseSyncImpl = (db, bytes) => NodeSqlite.restoreDatabaseSync(db, bytes)
}
export const __setIsAutocommitDatabaseSyncForTest = (fn: (db: DatabaseSync) => boolean): void => {
  isAutocommitDatabaseSyncImpl = fn
}
export const __resetIsAutocommitDatabaseSyncForTest = (): void => {
  isAutocommitDatabaseSyncImpl = (db) => NodeSqlite.isAutocommitDatabaseSync(db)
}

/**
 * Implementation of `golem:api/save-snapshot.save`. Reads the active
 * agent's bound snapshot state, encodes it according to the active
 * variant (auto → JSON envelope; custom → binary v2 envelope), and
 * returns the resulting `Snapshot` to the host.
 */
export const dispatchSaveSnapshot = async (): Promise<ApiHost.Snapshot> => {
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
    const state = await Effect.runPromise(Ref.get(snap.ref) as Effect.Effect<unknown, never>)
    const encoded = await Effect.runPromise(
      Schema.encodeUnknownEffect(snap.schema)(state) as Effect.Effect<unknown, Schema.SchemaError>,
    )
    if (snap.declaredDatabases.length === 0) {
      return encodeJsonEnvelope(agent.principal, encoded)
    }
    const dbParts: Array<{ name: string; bytes: Uint8Array }> = []
    for (const dbName of snap.declaredDatabases) {
      const handle = snap.databases.get(dbName)
      if (handle === undefined) {
        throw new SnapshotDatabaseMissingPartError(agent.name, dbName, "save")
      }
      if (!isAutocommitDatabaseSyncImpl(handle)) {
        throw new SnapshotDatabaseNotInAutocommitError(agent.name, dbName)
      }
      const rows = handle.prepare("PRAGMA database_list").all() as Array<{ name?: string }>
      const extra = rows
        .map((r) => String(r.name ?? ""))
        .filter((n) => n !== "main" && n !== "temp" && n !== "")
      if (extra.length > 0) {
        throw new SnapshotDatabaseHasAttachmentsError(agent.name, dbName, extra)
      }
      dbParts.push({ name: dbName, bytes: serializeDatabaseSyncImpl(handle) })
    }
    return encodeMultipartJsonEnvelope(agent.principal, encoded, dbParts)
  }
  const bytes = await Effect.runPromise(
    provideObservability(
      snap.handlers.save.pipe(Effect.provideService(Principal, agent.principal)) as Effect.Effect<
        Uint8Array,
        unknown,
        never
      >,
    ),
  )
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
 */
export const dispatchLoadSnapshot = async (snapshot: ApiHost.Snapshot): Promise<void> => {
  if (activeAgent !== null) {
    throw new Error(`agent already initialized in this container: ${activeAgent.name}`)
  }

  // 1. Recover the agent ID + parse it.
  const env = getEnvironmentImpl()
  const agentIdEntry = env.find(([k]) => k === "GOLEM_AGENT_ID")
  if (agentIdEntry === undefined) {
    throw new Error("load-snapshot: GOLEM_AGENT_ID is not set in the process environment")
  }
  const agentIdString = agentIdEntry[1]
  const [agentTypeName, ctorDataValue] = parseAgentIdImpl(agentIdString)

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
        for (const part of decoded.databases) {
          const handle = bound.databases.get(part.name)!
          restoreDatabaseSyncImpl(handle, part.bytes)
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
      await Effect.runPromise(
        provideObservability(
          bound.handlers
            .load(decoded.userPayload)
            .pipe(Effect.provideService(Principal, principal)) as Effect.Effect<
            void,
            unknown,
            never
          >,
        ),
      )
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
