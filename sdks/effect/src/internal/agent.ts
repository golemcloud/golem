/**
 * @since 1.5.0
 */
import { Cause, Effect, Exit, Layer, ManagedRuntime, Ref, Schema, Scope } from "effect"
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
} from "../Http.js"
import type { BindableKeys, MountDefCovering, WebhookVarsValid } from "./httpTypes.js"
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

/**
 * Resolves to `true` when at least one entry in `Methods` carries the
 * `HasHttp = true` phantom (i.e. was built via `method({ http: [...] })`
 * with a non-empty `http` tuple OR via `withHttp(...)` with at least
 * one endpoint), else `false`.
 *
 * Mirrors (defence-in-depth) the runtime "any endpoints declared" check
 * in `validateAgentHttp` (Http.ts L1452: `m.endpoints.length > 0`). The
 * runtime check stays the canonical error source — this helper is
 * consumed by {@link AgentMetadata} below to make the agent's `http`
 * field a *required* mount when at least one method has endpoints, and
 * an *optional* mount otherwise.
 *
 * The `true extends Union ? true : false` shape (rather than
 * `Union extends true ? ...`) is required because the union of
 * per-method `HasHttp` values is a `boolean`-shaped distribution that
 * otherwise distributes back into `boolean` and loses the "any one of
 * them is true" signal.
 *
 * @since 1.5.0
 * @category models
 */
type AnyMethodHasHttp<Methods extends Record<string, AnyMethodSpec>> = true extends {
  [K in keyof Methods]: Methods[K] extends MethodSpec<any, any, any, infer H> ? H : false
}[keyof Methods]
  ? true
  : false

/**
 * Per-call constraint added on top of {@link AgentMetadata} that
 * makes the agent's `http` field required (and fully constrained by
 * {@link MountDefCovering} + {@link WebhookVarsValid}) iff
 * {@link AnyMethodHasHttp} resolves to `true` for the agent's
 * `methods` record. Otherwise resolves to `unknown` — a no-op
 * intersection that keeps the original `http?: …` optional.
 *
 * Intersected with the `def` parameter at the `defineAgent` /
 * `registerAgent` call sites. Intersection rather than
 * an in-line conditional on `AgentMetadata.http` because the
 * interface declares `http?: …` (optional) and there is no per-arity
 * way to flip a property between optional and required inside an
 * interface body. The intersection makes `http` required exactly when
 * `AnyMethodHasHttp<Methods>` is `true` — TS treats the intersection
 * `{ http?: T } & { http: T }` as `{ http: T }` (non-optional).
 *
 * Mirrors (defence-in-depth) the runtime check in `validateAgentHttp`
 * (Http.ts L1452-1461).
 *
 * @since 1.5.0
 * @category models
 */
type AgentHttpRequirement<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  MV extends string,
  WV extends string,
> =
  AnyMethodHasHttp<Methods> extends true
    ? { readonly http: MountDefCovering<C, MV, WV> & WebhookVarsValid<C, WV> }
    : unknown

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
 * Optional `config` field on an {@link AgentMetadata}. Built with
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
 * Constructor effect signature for an agent. Runs once per agent
 * instance in the agent's lifetime `Scope`. May depend on
 * {@link Principal} (provided by the dispatcher with the value the
 * host passed to `initialize`) and on the optional config service.
 * When `S` is a {@link SnapshotDef}, the constructor receives a
 * second {@link SnapshotBinding} argument.
 *
 * @since 1.5.0
 * @category models
 */
export type AgentImpl<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  F extends ConfigFields = never,
  S extends SnapshotDef = never,
> = (
  ...args: ImplArgs<C, S, CfgTagOf<F>>
) => Effect.Effect<
  Handlers<Methods, CfgTagOf<F>>,
  unknown,
  Scope.Scope | Principal | HostServices | CfgTagOf<F>
>

/**
 * Metadata describing an agent's *type*: the constructor and method
 * signatures, plus any optional host capabilities (HTTP mount, config
 * service, snapshot policy, execution mode). This is the input to
 * {@link defineAgent}; it carries everything the WIT-visible
 * `AgentType` needs except the per-component implementation, which is
 * supplied separately via {@link AgentSpec.implement}.
 *
 * @since 1.5.0
 * @category models
 */
export interface AgentMetadata<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode = AgentCommon.AgentMode,
  F extends ConfigFields = never,
  S extends SnapshotDef = never,
  MV extends string = BindableKeys<C>,
  WV extends string = never,
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
   *
   * Type-level constraints:
   * - Every `{var}` in the mount path must be a constructor parameter
   *   name AND must be statically eligible for path binding (i.e. not
   *   a {@link Multimodal} or {@link ElementSpec} carrier — see
   *   {@link BindableKeys}); enforced via {@link MountDefCovering}.
   * - Every `{var}` in the optional `webhookSuffix` must likewise be a
   *   bindable constructor-parameter name; enforced via
   *   {@link WebhookVarsValid}.
   *
   * Full string-bindability (rejecting `Schema.Struct` etc.) and
   * full constructor-coverage are enforced at registration time by
   * the runtime validators in `Http.ts`.
   */
  readonly http?: MountDefCovering<C, MV, WV> & WebhookVarsValid<C, WV>
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
   * user-managed path. When present, the agent's `impl` receives a
   * second {@link SnapshotBinding} argument, and the agent type's
   * `snapshotting` metadata reflects the configured policy.
   */
  readonly snapshot?: S
}

/**
 * The value returned by {@link defineAgent}: an immutable, canonical
 * snapshot of the agent's {@link AgentMetadata} plus
 *
 * - a derived `client` namespace for connecting to remote instances of
 *   this agent type via the Golem RPC host, and
 * - an `implement(...)` method that registers the agent with the runtime
 *   and returns an {@link ImplementedAgent}.
 *
 * The `client` shape depends on the agent's `mode`:
 *
 * - durable agents expose `get`, `getPhantom`, and `newPhantom`.
 * - ephemeral agents expose only `getPhantom` and `newPhantom`.
 *
 * A spec on its own performs NO runtime registration — it is safe for
 * pure RPC callers to import a module that only constructs spec values.
 * Registration happens when `.implement(...)` is invoked.
 *
 * @since 1.5.0
 * @category models
 */
export type AgentSpec<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode = AgentCommon.AgentMode,
  F extends ConfigFields = never,
  S extends SnapshotDef = never,
> = AgentMetadata<C, Methods, M, F, S> & {
  readonly client: AgentClient<C, Methods, M, F>
  /**
   * Attach an implementation to the spec and eagerly register the agent
   * with the runtime. Returns an {@link ImplementedAgent} that exposes
   * the same {@link AgentClient} instance as the spec.
   *
   * Calling `implement` twice on the same spec — or on two specs that
   * share the same `name` — surfaces a {@link DuplicateAgentNameError}
   * via the same deferred-error path as today's eager `defineAgent`
   * (stashed in {@link pendingRegistrationErrors}, re-emitted from
   * {@link dispatchDiscoverAgentTypes}).
   */
  readonly implement: (impl: AgentImpl<C, Methods, F, S>) => ImplementedAgent<C, Methods, M, F, S>
}

/**
 * The value returned by {@link AgentSpec.implement}: the canonical
 * {@link AgentMetadata} plus the same `client` reference the spec
 * exposes, and a `spec` back-reference so consumers can reach the
 * original spec without storing it in a separate top-level binding.
 *
 * Notably, an `ImplementedAgent` does NOT expose `implement(...)`
 * itself — this prevents accidental double-registration at the type
 * level.
 *
 * @since 1.5.0
 * @category models
 */
export type ImplementedAgent<
  C extends MethodParams,
  Methods extends Record<string, AnyMethodSpec>,
  M extends AgentCommon.AgentMode,
  F extends ConfigFields = never,
  S extends SnapshotDef = never,
> = AgentMetadata<C, Methods, M, F, S> & {
  readonly client: AgentClient<C, Methods, M, F>
  readonly spec: AgentSpec<C, Methods, M, F, S>
}

/**
 * Define an agent type and build a typed RPC client for it WITHOUT
 * registering the agent with the runtime.
 *
 * **Why the split**
 *
 * The returned {@link AgentSpec} is safe to import from modules whose
 * only job is to make remote calls to this agent type via
 * `spec.client.*`. Importing a spec module does not pull in the
 * implementation code or trigger any registration side effect on the
 * host. Registration happens when {@link AgentSpec.implement} is
 * called — typically from the component's entry point.
 *
 * **Canonicalization**
 *
 * `defineAgent` shallow-clones and freezes the supplied metadata
 * (top-level object, `constructorParams`, `methods`) so that the
 * spec's `client` (built immediately) and any later
 * `spec.implement(...)` registration always agree on the same view of
 * the metadata. Mutating the original user-supplied literal after
 * `defineAgent` returns has no effect on the spec.
 *
 * **Compile-time guarantees on the `http` field**
 *
 * When at least one method in `methods` declares an `http` array of
 * endpoints, the agent's `http: Http.mount(...)` field becomes
 * *required* — `tsc` rejects the call site with "Property 'http' is
 * missing" when it is omitted. When no method declares HTTP
 * endpoints, `http` remains optional, matching the runtime "any
 * endpoints declared" check in `validateAgentHttp`.
 *
 * When `http` IS supplied, two additional type-level constraints
 * apply to it:
 *
 * - Every constructor parameter in `constructorParams` must appear as
 *   a `{var}` segment in the mount path; missing vars surface as an
 *   `Invalid<"mount path missing var '…'">` carrier on the assigned
 *   {@link MountDefCovering} type.
 * - Every `{var}` in the optional `webhookSuffix` must match a
 *   constructor-parameter name AND must be statically eligible for
 *   binding (i.e. NOT a {@link Multimodal} or {@link ElementSpec}
 *   carrier — see {@link BindableKeys}); violations surface as a
 *   {@link WebhookVarsValid} carrier with a readable reason string.
 *
 * Trivial path-shape rules and per-endpoint duplicate-binding /
 * case-fold / bodyless-unbound checks fire earlier — at the
 * `Http.mount(...)` / `Http.get(...)` / `method({ http: [...] })`
 * call sites.
 *
 * **Runtime fallbacks (defence-in-depth)**
 *
 * Full string-bindability of mount-path / webhook-suffix vars,
 * the brace-balance check, the var-name regex, and any
 * configuration whose path string was supplied as a non-literal
 * `string` value still run inside `validateAgentHttp` /
 * `validateMount` / `validateEndpoint` and surface as `HttpRouteError`
 * from `spec.implement(...)`.
 *
 * **Validation-error reporting**
 *
 * Validation failures from a later `spec.implement(...)` call (i.e.
 * `UnsupportedSchemaError`, `HttpRouteError`, `InvalidSnapshotError`,
 * `DuplicateAgentNameError`, or any other typed failure / defect)
 * are NOT thrown synchronously. Instead the failure is captured in
 * {@link pendingRegistrationErrors} and re-emitted as a typed
 * `golem:agent/common@1.5.0.agent-error` (`invalid-type` variant)
 * thrown from the WIT-exported
 * {@link dispatchDiscoverAgentTypes} host call. This lets the
 * Golem CLI's metadata-extraction step surface misconfigurations
 * as proper structured diagnostics rather than as a WASM
 * instantiation crash. The returned {@link ImplementedAgent} value
 * is still constructed so that other modules importing the agent
 * keep working — any subsequent `initialize` / `invoke` against the
 * un-registered type still fails at the dispatcher with the usual
 * "unknown agent" error.
 *
 * @see {@link registerAgent} for the lower-level registration-only
 *      entry point (Effect-typed failures).
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
  MV extends string = BindableKeys<C>,
  WV extends string = never,
>(
  metadata: AgentMetadata<C, Methods, M, F, S, MV, WV> & AgentHttpRequirement<C, Methods, MV, WV>,
): AgentSpec<C, Methods, M, F, S> => {
  // Canonicalize: shallow-clone the top-level object and the two
  // nested containers, then freeze them so the spec is immutable from
  // the caller's perspective. This closes the mutation window between
  // spec construction (which `clientFor` reads from) and
  // `.implement(...)` (which `registerAgent` re-reads from later).
  const canonicalConstructorParams = Object.freeze({ ...metadata.constructorParams }) as C
  const canonicalMethods = Object.freeze({ ...metadata.methods }) as Methods
  const canonical = Object.freeze({
    ...metadata,
    constructorParams: canonicalConstructorParams,
    methods: canonicalMethods,
  }) as AgentMetadata<C, Methods, M, F, S, MV, WV>

  // Build the typed RPC client once from the canonical metadata. The
  // same reference is shared between the spec and any
  // `ImplementedAgent` produced by `spec.implement(...)` below.
  const sharedClient = clientFor(canonical as unknown as AgentMetadata<C, Methods, M, F>)

  // `implement` is created as a closure rather than a prototype method
  // so the generics inferred by `defineAgent` (C, Methods, M, F, S)
  // flow into the constructor parameter shape without requiring the
  // user to re-state them.
  //
  // `.implement(...)` is single-shot per spec — the `consumed` flag is
  // set on the first call regardless of whether registration succeeded
  // or pushed a deferred error. A second call always becomes a deferred
  // {@link DuplicateAgentNameError} so a flaky retry loop cannot leak
  // additional registrations or accumulate stacked errors.
  let consumed = false
  const implement = (impl: AgentImpl<C, Methods, F, S>): ImplementedAgent<C, Methods, M, F, S> => {
    if (consumed) {
      pendingRegistrationErrors.push({
        agentName: canonical.name,
        cause: Cause.fail(new DuplicateAgentNameError(canonical.name)),
      })
    } else {
      consumed = true
      // The caller-side `AgentHttpRequirement` intersection is already
      // satisfied at the `defineAgent` call site; re-introduce it here
      // for `registerAgent`'s strictly-typed input.
      const metadataForRegistration = canonical as AgentMetadata<C, Methods, M, F, S, MV, WV> &
        AgentHttpRequirement<C, Methods, MV, WV>
      const exit = Effect.runSyncExit(registerAgent(metadataForRegistration, impl))
      if (Exit.isFailure(exit)) {
        pendingRegistrationErrors.push({ agentName: canonical.name, cause: exit.cause })
      }
    }
    // Erase the `MV` / `WV` mount-vars phantoms — the public
    // {@link ImplementedAgent} surfaces only the user-visible fields.
    return Object.freeze({
      ...canonical,
      client: sharedClient,
      spec,
    }) as unknown as ImplementedAgent<C, Methods, M, F, S>
  }

  // Erase the `MV` / `WV` phantoms as above; the public
  // {@link AgentSpec} surface is generic only over the user-visible
  // parameters.
  const spec = Object.freeze({
    ...canonical,
    client: sharedClient,
    implement,
  }) as unknown as AgentSpec<C, Methods, M, F, S>
  return spec
}

interface CompiledAgent {
  readonly name: string
  /**
   * The agent's metadata, type-erased over its generics. Carries every
   * field except the implementation (`impl` lives in its own slot).
   */
  readonly metadata: AgentMetadata<
    MethodParams,
    Record<string, AnyMethodSpec>,
    AgentCommon.AgentMode,
    never,
    SnapshotDef
  >
  /**
   * The user-supplied constructor effect, supplied to {@link registerAgent}
   * alongside the metadata. Called by {@link dispatchInitialize} and
   * {@link dispatchLoadSnapshot} with the decoded constructor input.
   */
  readonly impl: AgentImpl<MethodParams, Record<string, AnyMethodSpec>, never, SnapshotDef>
  readonly constructorBindings: ReadonlyArray<ParamBinding>
  /** Filtered view of {@link constructorBindings}: only component-model wire bindings. */
  readonly constructorCodecs: ReadonlyArray<ParamCodec>
  readonly methodCodecs: ReadonlyMap<string, MethodCodec<MethodParams, Schema.Top, Schema.Top>>
  readonly agentType: AgentCommon.AgentType
  /** Compiled config bundle when `metadata.config` is set; `null` otherwise. */
  readonly compiledConfig: CompiledConfig | null
  /** Compiled snapshot bundle when `metadata.snapshot` is set; `null` otherwise. */
  readonly compiledSnapshot: CompiledSnapshot | null
}

/** Module-level registry of compiled agents, keyed by `typeName`. */
const registry = new Map<string, CompiledAgent>()

/**
 * Validation failures captured by {@link defineAgent} (i.e. typed
 * failures or defects from {@link registerAgent} — invalid schemas,
 * malformed HTTP routes, malformed snapshot config, duplicate agent
 * names, etc.). These are deliberately NOT thrown at module-import
 * time; instead they are surfaced from the WIT-exported
 * `discover-agent-types` host call as a typed `AgentError` so that
 * tooling (e.g. the Golem CLI's metadata-extraction pass) can present
 * them as structured diagnostics rather than as a WASM instantiation
 * crash.
 */
const pendingRegistrationErrors: Array<{
  readonly agentName: string
  readonly cause: Cause.Cause<unknown>
}> = []

/**
 * Raised by {@link registerAgent} when an agent type name is registered
 * more than once in the same component. The SDK rejects the second
 * registration fail-fast rather than silently overwriting the earlier
 * definition, because two `defineAgent` calls sharing a `name` would
 * otherwise leave `discoverAgentTypes` / `initialize` operating on
 * whichever module happened to be imported last.
 *
 * `defineAgent` does NOT throw this error synchronously: it stashes
 * the failure in {@link pendingRegistrationErrors} and re-emits it as
 * a typed `agent-error` (`invalid-type` variant) from
 * {@link dispatchDiscoverAgentTypes} so the host / CLI can surface
 * the conflict as a structured diagnostic.
 *
 * @since 1.5.0
 * @category errors
 */
export class DuplicateAgentNameError {
  readonly _tag = "DuplicateAgentNameError"
  readonly message: string
  constructor(readonly agentName: string) {
    this.message = `DuplicateAgentNameError: an agent named '${agentName}' is already registered in this component`
  }
}

/**
 * Register an agent metadata + implementation pair with the runtime.
 * Pure schema-walking work for the metadata side — `impl` is captured
 * but NOT executed here; no per-instance state is created. Safe to
 * call at deploy time for type discovery.
 *
 * Surfaces validation failures as typed Effect failures, unlike the
 * chained `defineAgent(...).implement(...)` form, which defers them
 * to {@link dispatchDiscoverAgentTypes}.
 *
 * @see {@link defineAgent} for the chained-API shorthand that additionally
 *      builds a typed RPC client.
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
  MV extends string = BindableKeys<C>,
  WV extends string = never,
>(
  metadata: AgentMetadata<C, Methods, M, F, S, MV, WV> & AgentHttpRequirement<C, Methods, MV, WV>,
  impl: AgentImpl<C, Methods, F, S>,
): Effect.Effect<
  void,
  UnsupportedSchemaError | HttpRouteError | InvalidSnapshotError | DuplicateAgentNameError
> =>
  Effect.gen(function* () {
    if (registry.has(metadata.name)) {
      return yield* Effect.fail(new DuplicateAgentNameError(metadata.name))
    }

    const constructorBindings = (yield* compileParamBindings(
      `${metadata.name} constructor`,
      metadata.constructorParams,
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
    for (const [methodName, spec] of Object.entries(metadata.methods)) {
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
      agentName: metadata.name,
      mount: metadata.http,
      constructorParamNames: Object.keys(metadata.constructorParams),
      nonStringBindableConstructorParams: collectNonStringBindableParams(
        metadata.constructorParams,
      ),
      stringBindableConstructorParams: collectStringBindableParams(metadata.constructorParams),
      methods: methodHttpInputs,
    })

    // Now build the AgentMethod records, attaching the compiled
    // httpEndpoint list per method.
    const agentMethods: Array<AgentCommon.AgentMethod> = []
    for (const [methodName, spec] of Object.entries(metadata.methods)) {
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
    if (metadata.config !== undefined) {
      const cc = yield* metadata.config.__compile()
      compiledConfig = cc
      configDeclarations = [...cc.declarations]
    }

    let compiledSnapshot: CompiledSnapshot | null = null
    let snapshotting: AgentCommon.Snapshotting = { tag: "disabled" }
    if (metadata.snapshot !== undefined) {
      const cs = yield* compileSnapshot(metadata.name, metadata.snapshot)
      compiledSnapshot = cs
      snapshotting = { tag: "enabled", val: cs.witConfig }
    }

    const agentType: AgentCommon.AgentType = {
      typeName: metadata.name,
      description: metadata.description ?? "",
      sourceLanguage: "typescript",
      constructor: {
        description: "",
        promptHint: metadata.promptHint,
        inputSchema: constructorSchema,
      },
      methods: agentMethods,
      dependencies: [],
      mode: metadata.mode ?? "durable",
      httpMount: compiledHttp.mount,
      snapshotting,
      config: configDeclarations,
    }

    registry.set(metadata.name, {
      name: metadata.name,
      metadata: metadata as unknown as AgentMetadata<
        MethodParams,
        Record<string, AnyMethodSpec>,
        AgentCommon.AgentMode,
        never,
        SnapshotDef
      >,
      impl: impl as unknown as AgentImpl<
        MethodParams,
        Record<string, AnyMethodSpec>,
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
  pendingRegistrationErrors.length = 0
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
      (compiled.impl as (...a: ReadonlyArray<unknown>) => unknown)(...implArgs) as Effect.Effect<
        Record<string, Handler<AnyMethodSpec>>,
        unknown,
        Scope.Scope | Principal | SelfAgentId
      >
    ).pipe(
      Effect.provideService(Principal, principal),
      Effect.provideService(SelfAgentId, selfAgentId),
    )
    if (compiled.compiledConfig !== null && compiled.metadata.config !== undefined) {
      const shape = await runUserPromise(compiled.compiledConfig.buildShape())
      program = (program as Effect.Effect<unknown, unknown, never>).pipe(
        // The config class is a Context.Service tag (Self/Identifier
        // resolved via the user's `defineConfig`-class declaration). We
        // erase the static generics here because the dispatcher works
        // generically over every registered agent.
        Effect.provideService(compiled.metadata.config as never, shape as never),
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
  if (compiled.compiledConfig !== null && compiled.metadata.config !== undefined) {
    const shape = await runUserPromise(compiled.compiledConfig.buildShape())
    program = program.pipe(
      Effect.provideService(compiled.metadata.config as never, shape as never),
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
export const dispatchDiscoverAgentTypes = async (): Promise<Array<AgentCommon.AgentType>> => {
  if (pendingRegistrationErrors.length > 0) {
    throw makeRegistrationAgentError(pendingRegistrationErrors)
  }
  return Array.from(registry.values()).map((c) => c.agentType)
}

/**
 * Render the stashed `defineAgent` registration failures as the
 * `invalid-type` variant of the WIT
 * `golem:agent/common@1.5.0.agent-error` ADT — that's the closest
 * variant to "this component's declared agent types could not be
 * built". The `val` string concatenates one section per failing agent
 * (the agent name, plus `Cause.pretty` of its underlying Effect
 * cause), giving the Golem CLI / host a human-readable diagnostic
 * payload while staying within the host-defined `string`-typed
 * variant.
 */
const makeRegistrationAgentError = (
  errors: ReadonlyArray<{ readonly agentName: string; readonly cause: Cause.Cause<unknown> }>,
): AgentCommon.AgentError => {
  const sections = errors.map(
    ({ agentName, cause }) => `agent '${agentName}': ${Cause.pretty(cause)}`,
  )
  const header =
    errors.length === 1
      ? "effect-golem: 1 agent registration error"
      : `effect-golem: ${errors.length} agent registration errors`
  const message = `${header}\n${sections.join("\n")}`
  return { tag: "invalid-type", val: message }
}

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
 * {@link userRuntimeLayer} synchronously. `userRuntime.runSync` is
 * safe here because the underlying layers (`SqliteHostExtLive` etc.)
 * are pure `Layer.succeed`s — no async work happens during resolution.
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
 * Encode the auto-snapshot path synchronously.
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
 * @since 1.5.0
 * @category runtime hooks
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
    return encodeAutoSnapshot(agent, snap)
  }
  return dispatchSaveCustomSnapshot(agent, compiled, snap)
}

/**
 * Custom (`Snapshot.custom(...)`) save path: runs the user's
 * `Effect<Uint8Array, ...>` handler under the same runtime layer the
 * dispatcher uses for `invoke`.
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
  if (compiled.compiledConfig !== null && compiled.metadata.config !== undefined) {
    const shape = await runUserPromise(compiled.compiledConfig.buildShape())
    saveProgram = saveProgram.pipe(
      Effect.provideService(compiled.metadata.config as never, shape as never),
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
            throw new SnapshotDatabaseMissingPartError(agentTypeName, dbName, "load-envelope")
          }
        }
        // Validate that the user attached every declared database.
        for (const dbName of declared) {
          if (!bound.databases.has(dbName)) {
            throw new SnapshotDatabaseMissingPartError(agentTypeName, dbName, "load-attach")
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
      if (compiled.compiledConfig !== null && compiled.metadata.config !== undefined) {
        const shape = await runUserPromise(compiled.compiledConfig.buildShape())
        loadProgram = loadProgram.pipe(
          Effect.provideService(compiled.metadata.config as never, shape as never),
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
