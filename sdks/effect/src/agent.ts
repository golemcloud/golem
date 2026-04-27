import { Effect, Exit, Schema, Scope } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import type * as CoreTypes from "golem:core/types@1.5.0"
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
import { isElementSpec } from "./unstructured.js"
import { type UnsupportedSchemaError, type WitCodec } from "./wit-codec.js"
import { clientFor, type AgentClient } from "./client.js"
import type { CompiledConfig, ConfigClass, ConfigFields, ConfigShape } from "./config.js"

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
 */
export interface AgentDefinition<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode = AgentCommon.AgentMode,
  F extends ConfigFields = never,
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
   * Constructor effect. Runs once per agent instance, in the agent's
   * lifetime `Scope`. May depend on {@link Principal} (provided by the
   * dispatcher with the value the host passed to `initialize`) and on
   * the optional config service.
   */
  readonly impl: (
    input: MethodInput<C>,
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
> = AgentDefinition<C, Methods, M, F> & {
  readonly client: AgentClient<C, Methods, M, F>
}

export const defineAgent = <
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode = "durable",
  F extends ConfigFields = never,
>(
  def: AgentDefinition<C, Methods, M, F>,
): DefinedAgent<C, Methods, M, F> => {
  // Register eagerly so simply importing an agent module makes it
  // discoverable by the runtime — no separate `registerAgent` call is
  // required at the component entrypoint.
  Effect.runSync(registerAgent(def))
  return { ...def, client: clientFor(def) }
}

interface CompiledAgent {
  readonly name: string
  readonly definition: AgentDefinition<MethodParams, Record<string, AnyMethodSpec>>
  readonly constructorBindings: ReadonlyArray<ParamBinding>
  /** Backwards-compatible legacy view: only component-model wire bindings. */
  readonly constructorCodecs: ReadonlyArray<ParamCodec>
  readonly methodCodecs: ReadonlyMap<string, MethodCodec<MethodParams, Schema.Top, Schema.Top>>
  readonly agentType: AgentCommon.AgentType
  /** Compiled config bundle when `def.config` is set; `null` otherwise. */
  readonly compiledConfig: CompiledConfig | null
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
>(
  def: AgentDefinition<C, Methods, M, F>,
): Effect.Effect<void, UnsupportedSchemaError | HttpRouteError> =>
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
      snapshotting: { tag: "disabled" },
      config: configDeclarations,
    }

    registry.set(def.name, {
      name: def.name,
      definition: def as AgentDefinition<MethodParams, Record<string, AnyMethodSpec>>,
      constructorBindings,
      constructorCodecs,
      methodCodecs,
      agentType,
      compiledConfig,
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

  if (input.tag !== "tuple") {
    throw new Error(`${agentTypeName} constructor: expected tuple DataValue, got ${input.tag}`)
  }
  // Wire bindings line up positionally with the input tuple. (The
  // runtime-injected `Principal` is delivered as an Effect service by
  // the dispatcher, not as a wire parameter, so it is never present
  // here.)
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

  // Open a fresh scope tied to the agent's lifetime, then run `impl` with
  // that scope provided. The scope stays open until shutdown, so any
  // `Effect.acquireRelease` inside `impl` holds resources for as long as
  // the agent lives.
  //
  // The `Principal` service is provided here so that `impl` can read the
  // initialize-time principal via `yield* Principal` without surfacing
  // the requirement in the public `AgentDefinition.impl` type. The
  // optional config service is built fresh per invocation (so regular
  // fields are memoized for the duration of `impl` only, secret fields
  // are never cached) and provided alongside Principal.
  const scope = await Effect.runPromise(Scope.make())
  let handlers: Record<string, Handler<AnyMethodSpec>>
  try {
    let program = (
      compiled.definition.impl(constructorInput) as Effect.Effect<
        Record<string, Handler<AnyMethodSpec>>,
        unknown,
        Scope.Scope | Principal
      >
    ).pipe(Effect.provideService(Principal, principal))
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
    handlers = (await Effect.runPromise(Scope.use(program, scope))) as Record<
      string,
      Handler<AnyMethodSpec>
    >
  } catch (e) {
    // Initialization failed; close the scope to release anything that
    // managed to be acquired before the failure.
    await Effect.runPromise(Scope.close(scope, Exit.void))
    throw e
  }

  activeAgent = { name: agentTypeName, scope, handlers }
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
  ).pipe(Effect.provideService(Principal, principal)) as Effect.Effect<
    CoreTypes.DataValue,
    unknown,
    never
  >
  if (compiled.compiledConfig !== null && compiled.definition.config !== undefined) {
    const shape = await Effect.runPromise(
      compiled.compiledConfig.buildShape() as Effect.Effect<unknown, never>,
    )
    program = program.pipe(
      Effect.provideService(compiled.definition.config as never, shape as never),
    ) as typeof program
  }
  return await Effect.runPromise(program)
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
