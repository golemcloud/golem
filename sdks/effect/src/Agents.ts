/**
 * Effect-idiomatic façade over the agent-management subset of
 * `golem:api/host@1.5.0`:
 *
 * - introspection: {@link getSelfMetadata}, {@link getAgentMetadata}
 * - resolution: {@link resolveComponentId}, {@link resolveAgentId},
 *   {@link resolveAgentIdStrict}
 * - lifecycle: {@link updateAgent}, {@link forkAgent},
 *   {@link revertAgent}, {@link fork}
 * - enumeration: {@link getAgents} (paged stream)
 *
 * Every host call is wrapped in `Effect.try` and surfaces unexpected
 * throws as {@link AgentsHostError}; user-supplied numeric inputs are
 * validated with {@link AgentsValidationError}.
 *
 * For the running agent's own `AgentId` (free of host-call cost), use
 * the `SelfAgentId` Context service instead.
 *
 * @since 0.1.0
 */
import { Effect, Option, Stream } from "effect"
import type * as ApiHost from "golem:api/host@1.5.0"
import { AgentHostClient } from "./host/AgentHostClient.js"
import { PromiseClient } from "./host/PromiseClient.js"

// ---------------------------------------------------------------------------
// Re-exported raw types
// ---------------------------------------------------------------------------

/**
 * Re-exported raw WIT types from `golem:api/host@1.5.0`. Surface the
 * structural shapes so user code can pattern-match on `tag` / consume
 * fields without importing the host module directly.
 *
 * @since 0.1.0
 * @category re-exports
 */
export type {
  AgentAllFilter,
  AgentAnyFilter,
  AgentConfigVarsFilter,
  AgentCreatedAtFilter,
  AgentEnvFilter,
  AgentId,
  AgentMetadata,
  AgentNameFilter,
  AgentPropertyFilter,
  AgentStatus,
  AgentStatusFilter,
  AgentVersionFilter,
  ComponentId,
  ComponentRevision,
  EnvironmentId,
  FilterComparator,
  ForkDetails,
  ForkResult,
  OplogIndex,
  PromiseId,
  RevertAgentTarget,
  StringFilterComparator,
  UpdateMode,
  Uuid,
} from "golem:api/host@1.5.0"

type RawAgentId = ApiHost.AgentId
type RawComponentId = ApiHost.ComponentId
type RawAgentMetadata = ApiHost.AgentMetadata
type RawAgentAnyFilter = ApiHost.AgentAnyFilter
type RawRevertAgentTarget = ApiHost.RevertAgentTarget
type RawForkResult = ApiHost.ForkResult
type RawUpdateMode = ApiHost.UpdateMode
type RawOplogIndex = ApiHost.OplogIndex

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/**
 * Raised when a `golem:api/host@1.5.0` agent-management call throws.
 *
 * @since 0.1.0
 * @category errors
 */
export class AgentsHostError {
  readonly _tag = "AgentsHostError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `AgentsHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

/**
 * Raised when a user-supplied input is out of range or malformed.
 *
 * @since 0.1.0
 * @category errors
 */
export class AgentsValidationError {
  readonly _tag = "AgentsValidationError"
  readonly message: string
  constructor(readonly reason: string) {
    this.message = `AgentsValidationError: ${reason}`
  }
}

const UINT64_MAX = (1n << 64n) - 1n

const toUint64 = (
  value: bigint | number,
  ctx: string,
): Effect.Effect<bigint, AgentsValidationError> => {
  let v: bigint
  if (typeof value === "bigint") v = value
  else if (Number.isSafeInteger(value)) v = BigInt(value)
  else
    return Effect.fail(
      new AgentsValidationError(`${ctx} must be a safe integer or bigint (got ${String(value)})`),
    )
  if (v < 0n || v > UINT64_MAX) {
    return Effect.fail(
      new AgentsValidationError(`${ctx} must fit an unsigned 64-bit integer (got ${v.toString()})`),
    )
  }
  return Effect.succeed(v)
}

// ---------------------------------------------------------------------------
// Revert-target builders
// ---------------------------------------------------------------------------

/**
 * Pure-data constructors for the `revert-agent-target` variant.
 *
 * @since 0.1.0
 * @category constructors
 */
export const RevertTarget = {
  toOplogIndex: (idx: RawOplogIndex): RawRevertAgentTarget => ({
    tag: "revert-to-oplog-index",
    val: idx,
  }),
  lastInvocations: (
    n: bigint | number,
  ): Effect.Effect<RawRevertAgentTarget, AgentsValidationError> =>
    toUint64(n, "RevertTarget.lastInvocations").pipe(
      Effect.map((val) => ({ tag: "revert-last-invocations", val }) as const),
    ),
} as const

/**
 * Local WIT-drift exhaustiveness witness for {@link RevertTarget}: every
 * tag in `golem:api/host@1.5.0.revert-agent-target` must have a
 * corresponding constructor here. If `golem-types/*.d.ts` is regenerated
 * with a new variant, this `satisfies` clause fails to compile and points
 * directly at the wrapper that needs updating.
 */
void ({
  "revert-to-oplog-index": RevertTarget.toOplogIndex,
  "revert-last-invocations": RevertTarget.lastInvocations,
} satisfies Record<RawRevertAgentTarget["tag"], unknown>)

// ---------------------------------------------------------------------------
// Filter DSL
// ---------------------------------------------------------------------------

type FilterNode =
  | { readonly _tag: "leaf"; readonly node: ApiHost.AgentPropertyFilter }
  | { readonly _tag: "all"; readonly children: ReadonlyArray<FilterNode> }
  | { readonly _tag: "any"; readonly children: ReadonlyArray<FilterNode> }

/**
 * Immutable filter AST for {@link getAgents}. Compose with `.and(...)`
 * (intersection) and `.or(...)` (union); compile to the WIT
 * {@link RawAgentAnyFilter} via {@link toRawFilter}.
 *
 * @since 0.1.0
 * @category models
 */
export class Filter {
  /** @internal */
  private constructor(private readonly node: FilterNode) {}

  /** Match by agent name. */
  static name(comparator: ApiHost.StringFilterComparator, value: string): Filter {
    return Filter.leaf({ tag: "name", val: { comparator, value } })
  }

  /** Match by agent status. */
  static status(comparator: ApiHost.FilterComparator, value: ApiHost.AgentStatus): Filter {
    return Filter.leaf({ tag: "status", val: { comparator, value } })
  }

  /** Match by component version. */
  static version(comparator: ApiHost.FilterComparator, value: bigint): Filter {
    return Filter.leaf({ tag: "version", val: { comparator, value } })
  }

  /** Match by creation time (epoch nanos). */
  static createdAt(comparator: ApiHost.FilterComparator, value: bigint): Filter {
    return Filter.leaf({ tag: "created-at", val: { comparator, value } })
  }

  /** Match by env-var key/value. */
  static env(name: string, comparator: ApiHost.StringFilterComparator, value: string): Filter {
    return Filter.leaf({ tag: "env", val: { name, comparator, value } })
  }

  /** Match by config-var key/value. */
  static config(name: string, comparator: ApiHost.StringFilterComparator, value: string): Filter {
    return Filter.leaf({ tag: "config", val: { name, comparator, value } })
  }

  private static leaf(node: ApiHost.AgentPropertyFilter): Filter {
    return new Filter({ _tag: "leaf", node })
  }

  /** Intersection — both must match. */
  and(other: Filter): Filter {
    return new Filter({
      _tag: "all",
      children:
        this.node._tag === "all" ? [...this.node.children, other.node] : [this.node, other.node],
    })
  }

  /** Union — either may match. */
  or(other: Filter): Filter {
    return new Filter({
      _tag: "any",
      children:
        this.node._tag === "any" ? [...this.node.children, other.node] : [this.node, other.node],
    })
  }

  /** @internal — read the AST root. */
  toNode(): FilterNode {
    return this.node
  }
}

/**
 * Local WIT-drift exhaustiveness witness for {@link Filter}: every tag
 * in `golem:api/host@1.5.0.agent-property-filter` must have a corresponding
 * static constructor on {@link Filter}. If `golem-types/*.d.ts` is
 * regenerated with a new variant, this `satisfies` clause fails to compile
 * and points directly at the wrapper that needs updating.
 */
void ({
  name: Filter.name,
  status: Filter.status,
  version: Filter.version,
  "created-at": Filter.createdAt,
  env: Filter.env,
  config: Filter.config,
} satisfies Record<ApiHost.AgentPropertyFilter["tag"], unknown>)

/**
 * Convert a {@link FilterNode} to its **disjunctive normal form** —
 * a list of conjunctions of leaves, where the outer list represents
 * OR and each inner list represents AND. The WIT expects exactly this
 * shape (`AgentAnyFilter { filters: AgentAllFilter[] }`).
 *
 * Transformation:
 *
 * - `leaf` → `[[leaf]]`
 * - `any(c)` → `concat(toDnf(c))`
 * - `all(c)` → cartesian product of each child's DNF, concatenating
 *   the conjunctions (so `(A or B) and (C or D)` becomes
 *   `(A and C) or (A and D) or (B and C) or (B and D)`).
 */
const toDnf = (node: FilterNode): Array<Array<ApiHost.AgentPropertyFilter>> => {
  switch (node._tag) {
    case "leaf":
      return [[node.node]]
    case "any": {
      const out: Array<Array<ApiHost.AgentPropertyFilter>> = []
      for (const child of node.children) {
        for (const conj of toDnf(child)) out.push(conj)
      }
      return out
    }
    case "all": {
      let acc: Array<Array<ApiHost.AgentPropertyFilter>> = [[]]
      for (const child of node.children) {
        const childDnf = toDnf(child)
        const next: Array<Array<ApiHost.AgentPropertyFilter>> = []
        for (const left of acc) {
          for (const right of childDnf) {
            next.push([...left, ...right])
          }
        }
        acc = next
      }
      return acc
    }
  }
}

/**
 * Compile a {@link Filter} to its WIT shape.
 *
 * @since 0.1.0
 * @category combinators
 */
export const toRawFilter = (filter: Filter): RawAgentAnyFilter => ({
  filters: toDnf(filter.toNode()).map((conj) => ({ filters: conj })),
})

const isFilter = (v: unknown): v is Filter => v instanceof Filter

// ---------------------------------------------------------------------------
// Effect-typed host calls
// ---------------------------------------------------------------------------

/**
 * Read the running agent's full metadata. Heavyweight (host call +
 * oplog entry per invocation); for just the agent-id, prefer the
 * `SelfAgentId` Context service.
 *
 * @since 0.1.0
 * @category operations
 */
export const getSelfMetadata: Effect.Effect<RawAgentMetadata, AgentsHostError, AgentHostClient> =
  Effect.gen(function* () {
    const ah = yield* AgentHostClient
    return yield* Effect.try({
      try: () => ah.getSelfMetadata(),
      catch: (e) => new AgentsHostError(e),
    })
  })

/**
 * Read another agent's metadata, or `undefined` if it does not exist.
 *
 * @since 0.1.0
 * @category operations
 */
export const getAgentMetadata = (
  id: RawAgentId,
): Effect.Effect<RawAgentMetadata | undefined, AgentsHostError, AgentHostClient> =>
  Effect.gen(function* () {
    const ah = yield* AgentHostClient
    return yield* Effect.try({
      try: () => ah.getAgentMetadata(id),
      catch: (e) => new AgentsHostError(e),
    })
  })

/**
 * Resolve a component reference to its `ComponentId`.
 *
 * @since 0.1.0
 * @category operations
 */
export const resolveComponentId = (
  componentReference: string,
): Effect.Effect<RawComponentId | undefined, AgentsHostError, AgentHostClient> =>
  Effect.gen(function* () {
    const ah = yield* AgentHostClient
    return yield* Effect.try({
      try: () => ah.resolveComponentId(componentReference),
      catch: (e) => new AgentsHostError(e),
    })
  })

/**
 * Resolve a `(componentReference, agentName)` pair to an `AgentId`.
 *
 * @since 0.1.0
 * @category operations
 */
export const resolveAgentId = (
  componentReference: string,
  agentName: string,
): Effect.Effect<RawAgentId | undefined, AgentsHostError, AgentHostClient> =>
  Effect.gen(function* () {
    const ah = yield* AgentHostClient
    return yield* Effect.try({
      try: () => ah.resolveAgentId(componentReference, agentName),
      catch: (e) => new AgentsHostError(e),
    })
  })

/**
 * Strict variant of {@link resolveAgentId}.
 *
 * @since 0.1.0
 * @category operations
 */
export const resolveAgentIdStrict = (
  componentReference: string,
  agentName: string,
): Effect.Effect<RawAgentId | undefined, AgentsHostError, AgentHostClient> =>
  Effect.gen(function* () {
    const ah = yield* AgentHostClient
    return yield* Effect.try({
      try: () => ah.resolveAgentIdStrict(componentReference, agentName),
      catch: (e) => new AgentsHostError(e),
    })
  })

/**
 * Initiate an update of the given agent to `targetRevision`. The
 * revision is validated to fit a `u64`. Returns immediately — the
 * actual update is asynchronous on the host side.
 *
 * @since 0.1.0
 * @category operations
 */
export const updateAgent = (input: {
  readonly agentId: RawAgentId
  readonly targetRevision: bigint | number
  readonly mode: RawUpdateMode
}): Effect.Effect<void, AgentsHostError | AgentsValidationError, AgentHostClient> =>
  Effect.gen(function* () {
    const target = yield* toUint64(input.targetRevision, "updateAgent.targetRevision")
    const ah = yield* AgentHostClient
    yield* Effect.try({
      try: () => ah.updateAgent(input.agentId, target, input.mode),
      catch: (e) => new AgentsHostError(e),
    })
  })

/**
 * Fork another agent at a given oplog index.
 *
 * @since 0.1.0
 * @category operations
 */
export const forkAgent = (input: {
  readonly source: RawAgentId
  readonly target: RawAgentId
  readonly oplogIdxCutOff: RawOplogIndex
}): Effect.Effect<void, AgentsHostError, AgentHostClient> =>
  Effect.gen(function* () {
    const ah = yield* AgentHostClient
    yield* Effect.try({
      try: () => ah.forkAgent(input.source, input.target, input.oplogIdxCutOff),
      catch: (e) => new AgentsHostError(e),
    })
  })

/**
 * Revert an agent to a previous state.
 *
 * @since 0.1.0
 * @category operations
 */
export const revertAgent = (
  agentId: RawAgentId,
  target: RawRevertAgentTarget,
): Effect.Effect<void, AgentsHostError, AgentHostClient> =>
  Effect.gen(function* () {
    const ah = yield* AgentHostClient
    yield* Effect.try({
      try: () => ah.revertAgent(agentId, target),
      catch: (e) => new AgentsHostError(e),
    })
  })

/**
 * Fork the current agent at the current execution point. Both the
 * original and the new ("forked") agent see this call return; inspect
 * the {@link RawForkResult} `tag` to discover which side you are on.
 *
 * @since 0.1.0
 * @category operations
 */
export const fork: Effect.Effect<RawForkResult, AgentsHostError, AgentHostClient> = Effect.gen(
  function* () {
    const ah = yield* AgentHostClient
    return yield* Effect.try({
      try: () => ah.fork(),
      catch: (e) => new AgentsHostError(e),
    })
  },
)

// ---------------------------------------------------------------------------
// `GetAgents` pager
// ---------------------------------------------------------------------------

/**
 * Stream all agents of the given component matching the optional
 * filter. Drives the host's `GetAgents` pager via `Stream.paginate`.
 *
 * @since 0.1.0
 * @category operations
 */
export const getAgents = (input: {
  readonly componentId: RawComponentId
  readonly filter?: Filter | RawAgentAnyFilter | undefined
  readonly precise?: boolean
}): Stream.Stream<RawAgentMetadata, AgentsHostError, AgentHostClient> => {
  const rawFilter = isFilter(input.filter) ? toRawFilter(input.filter) : input.filter
  return Stream.unwrap(
    Effect.gen(function* () {
      const ah = yield* AgentHostClient
      return yield* Effect.try({
        try: () => ah.getAgentsCtor(input.componentId, rawFilter, input.precise ?? false),
        catch: (e) => new AgentsHostError(e),
      })
    }).pipe(
      Effect.map((handle) =>
        Stream.paginate<ApiHost.GetAgents, RawAgentMetadata, AgentsHostError>(handle, (state) =>
          Effect.try({
            try: () => state.getNext(),
            catch: (e) => new AgentsHostError(e),
          }).pipe(
            Effect.map((chunk) =>
              chunk === undefined
                ? ([[], Option.none()] as const)
                : chunk.length === 0
                  ? ([[], Option.some(state)] as const)
                  : ([Array.from(chunk), Option.some(state)] as const),
            ),
          ),
        ),
      ),
    ),
  )
}

// ---------------------------------------------------------------------------
// Promise rendezvous (sub-namespace re-exported as `Promises`)
// ---------------------------------------------------------------------------

/**
 * Raised when {@link Promises.complete} is called on a promise that
 * was already completed (the host returns `false`).
 *
 * @since 0.1.0
 * @category errors
 */
export class PromiseAlreadyCompletedError {
  readonly _tag = "PromiseAlreadyCompletedError"
  readonly message: string
  constructor(readonly promiseId: ApiHost.PromiseId) {
    this.message = `PromiseAlreadyCompletedError: promise (oplog idx ${promiseId.oplogIdx.toString()}) was already completed`
  }
}

/**
 * Effect-flavoured promise rendezvous. Thin wrappers around the
 * `golem:api/host@1.5.0` promise API: a host-side shared rendezvous
 * channel where one fiber creates + awaits while another fiber (often
 * external) completes.
 *
 * @since 0.1.0
 * @category operations
 */
export const Promises = {
  /** Create a new host promise. */
  create: Effect.gen(function* () {
    const pc = yield* PromiseClient
    return yield* Effect.try({
      try: () => pc.createPromise(),
      catch: (e: unknown) => new AgentsHostError(e),
    })
  }) as Effect.Effect<ApiHost.PromiseId, AgentsHostError, PromiseClient>,

  /** Poll a promise: returns `undefined` until completed. */
  poll: (
    id: ApiHost.PromiseId,
  ): Effect.Effect<Uint8Array | undefined, AgentsHostError, PromiseClient> =>
    Effect.gen(function* () {
      const pc = yield* PromiseClient
      return yield* Effect.try({
        try: () => pc.getPromise(id).get(),
        catch: (e) => new AgentsHostError(e),
      })
    }),

  /**
   * Await a promise's completion, returning the payload.
   *
   * Bridges the host's `Pollable.subscribe()` →
   * `.abortablePromise(signal)` chain into Effect via `Effect.callback`.
   * The returned Effect is **fully interruptible**: interrupting the
   * surrounding fiber rejects the abortable promise synchronously
   * via `signal.aborted` and the Effect resolves to interruption.
   *
   * Note: unlike `RemoteMethod` invocations, there is **no host-level
   * cancel** for `golem:api/host` promises — `wasi:io/poll`'s
   * `Pollable` resource exposes no `.cancel()` and `getPromise(...)`
   * doesn't either. The interruption is therefore purely JS-side
   * (the in-flight `.then(...)` chain is dropped), and the underlying
   * host promise remains pending until some peer calls `complete`.
   */
  await: (id: ApiHost.PromiseId): Effect.Effect<Uint8Array, AgentsHostError, PromiseClient> =>
    Effect.gen(function* () {
      const pc = yield* PromiseClient
      const handle = yield* Effect.try({
        try: () => pc.getPromise(id),
        catch: (e) => new AgentsHostError(e),
      })
      // Fast path: already completed.
      const ready = yield* Effect.try({
        try: () => handle.get(),
        catch: (e) => new AgentsHostError(e),
      })
      if (ready !== undefined) return ready
      return yield* Effect.callback<Uint8Array, AgentsHostError>((resume, signal) => {
        // Guard the setup phase against synchronous throws from
        // `handle.subscribe()` / `pollable.abortablePromise(...)`.
        // Without this, a misbehaving host could escape the register
        // function as an Effect defect rather than a typed
        // `AgentsHostError`.
        try {
          const pollable = handle.subscribe()
          pollable
            .abortablePromise(signal)
            .then(() => {
              if (signal.aborted) return
              try {
                const value = handle.get()
                if (value === undefined) {
                  resume(
                    Effect.fail(
                      new AgentsHostError(
                        "Promises.await: pollable signalled ready but get() returned undefined",
                      ),
                    ),
                  )
                  return
                }
                resume(Effect.succeed(value))
              } catch (e) {
                resume(Effect.fail(new AgentsHostError(e)))
              }
            })
            .catch((e: unknown) => {
              // `abortablePromise` rejects with an `AbortError`-shaped
              // DOMException when `signal` aborts; that path is the
              // fiber-interrupt path and must NOT be reported as a
              // typed `AgentsHostError`.
              if (signal.aborted) return
              resume(Effect.fail(new AgentsHostError(e)))
            })
        } catch (e) {
          if (!signal.aborted) resume(Effect.fail(new AgentsHostError(e)))
        }
      })
    }),

  /**
   * Complete a promise. Resolves to `true` on success and fails with
   * {@link PromiseAlreadyCompletedError} when the host signals the
   * promise was already completed.
   */
  complete: (
    id: ApiHost.PromiseId,
    payload: Uint8Array,
  ): Effect.Effect<true, AgentsHostError | PromiseAlreadyCompletedError, PromiseClient> =>
    Effect.gen(function* () {
      const pc = yield* PromiseClient
      const ok = yield* Effect.try({
        try: () => pc.completePromise(id, payload),
        catch: (e) => new AgentsHostError(e),
      })
      if (!ok) return yield* Effect.fail(new PromiseAlreadyCompletedError(id))
      return true as const
    }),
} as const
