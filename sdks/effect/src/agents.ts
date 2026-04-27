import { Effect, Option, Stream } from "effect"
import * as ApiHost from "golem:api/host@1.5.0"

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
 * the {@link SelfAgentId} Context service instead.
 */

// ---------------------------------------------------------------------------
// Re-exported raw types
// ---------------------------------------------------------------------------

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
type RawComponentRevision = ApiHost.ComponentRevision

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/** Raised when a `golem:api/host@1.5.0` agent-management call throws. */
export class AgentsHostError {
  readonly _tag = "AgentsHostError"
  readonly message: string
  constructor(readonly cause: unknown) {
    this.message = `AgentsHostError: ${cause instanceof Error ? cause.message : String(cause)}`
  }
}

/** Raised when a user-supplied input is out of range or malformed. */
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

/** Pure-data constructors for the `revert-agent-target` variant. */
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

/** Compile a {@link Filter} to its WIT shape. */
export const toRawFilter = (filter: Filter): RawAgentAnyFilter => ({
  filters: toDnf(filter.toNode()).map((conj) => ({ filters: conj })),
})

const isFilter = (v: unknown): v is Filter => v instanceof Filter

// ---------------------------------------------------------------------------
// Host-binding indirections
// ---------------------------------------------------------------------------

let getSelfMetadataImpl: () => RawAgentMetadata = () => ApiHost.getSelfMetadata()
let getAgentMetadataImpl: (id: RawAgentId) => RawAgentMetadata | undefined = (id) =>
  ApiHost.getAgentMetadata(id)
let updateAgentImpl: (
  id: RawAgentId,
  targetRevision: RawComponentRevision,
  mode: RawUpdateMode,
) => void = (id, t, m) => ApiHost.updateAgent(id, t, m)
let forkAgentImpl: (
  source: RawAgentId,
  target: RawAgentId,
  oplogIdxCutOff: RawOplogIndex,
) => void = (s, t, o) => ApiHost.forkAgent(s, t, o)
let revertAgentImpl: (id: RawAgentId, target: RawRevertAgentTarget) => void = (id, t) =>
  ApiHost.revertAgent(id, t)
let forkImpl: () => RawForkResult = () => ApiHost.fork()
let resolveComponentIdImpl: (ref: string) => RawComponentId | undefined = (r) =>
  ApiHost.resolveComponentId(r)
let resolveAgentIdImpl: (ref: string, name: string) => RawAgentId | undefined = (r, n) =>
  ApiHost.resolveAgentId(r, n)
let resolveAgentIdStrictImpl: (ref: string, name: string) => RawAgentId | undefined = (r, n) =>
  ApiHost.resolveAgentIdStrict(r, n)
let getAgentsCtorImpl: (
  componentId: RawComponentId,
  filter: RawAgentAnyFilter | undefined,
  precise: boolean,
) => ApiHost.GetAgents = (componentId, filter, precise) =>
  new ApiHost.GetAgents(componentId, filter, precise)

/** @internal */
export const __setGetSelfMetadataForTest = (fn: () => RawAgentMetadata): void => {
  getSelfMetadataImpl = fn
}
/** @internal */
export const __resetGetSelfMetadataForTest = (): void => {
  getSelfMetadataImpl = () => ApiHost.getSelfMetadata()
}
/** @internal */
export const __setGetAgentMetadataForTest = (
  fn: (id: RawAgentId) => RawAgentMetadata | undefined,
): void => {
  getAgentMetadataImpl = fn
}
/** @internal */
export const __resetGetAgentMetadataForTest = (): void => {
  getAgentMetadataImpl = (id) => ApiHost.getAgentMetadata(id)
}
/** @internal */
export const __setUpdateAgentForTest = (
  fn: (id: RawAgentId, t: RawComponentRevision, m: RawUpdateMode) => void,
): void => {
  updateAgentImpl = fn
}
/** @internal */
export const __resetUpdateAgentForTest = (): void => {
  updateAgentImpl = (id, t, m) => ApiHost.updateAgent(id, t, m)
}
/** @internal */
export const __setForkAgentForTest = (
  fn: (s: RawAgentId, t: RawAgentId, o: RawOplogIndex) => void,
): void => {
  forkAgentImpl = fn
}
/** @internal */
export const __resetForkAgentForTest = (): void => {
  forkAgentImpl = (s, t, o) => ApiHost.forkAgent(s, t, o)
}
/** @internal */
export const __setRevertAgentForTest = (
  fn: (id: RawAgentId, t: RawRevertAgentTarget) => void,
): void => {
  revertAgentImpl = fn
}
/** @internal */
export const __resetRevertAgentForTest = (): void => {
  revertAgentImpl = (id, t) => ApiHost.revertAgent(id, t)
}
/** @internal */
export const __setForkForTest = (fn: () => RawForkResult): void => {
  forkImpl = fn
}
/** @internal */
export const __resetForkForTest = (): void => {
  forkImpl = () => ApiHost.fork()
}
/** @internal */
export const __setResolveComponentIdForTest = (
  fn: (ref: string) => RawComponentId | undefined,
): void => {
  resolveComponentIdImpl = fn
}
/** @internal */
export const __resetResolveComponentIdForTest = (): void => {
  resolveComponentIdImpl = (r) => ApiHost.resolveComponentId(r)
}
/** @internal */
export const __setResolveAgentIdForTest = (
  fn: (ref: string, name: string) => RawAgentId | undefined,
): void => {
  resolveAgentIdImpl = fn
}
/** @internal */
export const __resetResolveAgentIdForTest = (): void => {
  resolveAgentIdImpl = (r, n) => ApiHost.resolveAgentId(r, n)
}
/** @internal */
export const __setResolveAgentIdStrictForTest = (
  fn: (ref: string, name: string) => RawAgentId | undefined,
): void => {
  resolveAgentIdStrictImpl = fn
}
/** @internal */
export const __resetResolveAgentIdStrictForTest = (): void => {
  resolveAgentIdStrictImpl = (r, n) => ApiHost.resolveAgentIdStrict(r, n)
}
/** @internal */
export const __setGetAgentsCtorForTest = (
  fn: (
    componentId: RawComponentId,
    filter: RawAgentAnyFilter | undefined,
    precise: boolean,
  ) => ApiHost.GetAgents,
): void => {
  getAgentsCtorImpl = fn
}
/** @internal */
export const __resetGetAgentsCtorForTest = (): void => {
  getAgentsCtorImpl = (c, f, p) => new ApiHost.GetAgents(c, f, p)
}

// ---------------------------------------------------------------------------
// Effect-typed host calls
// ---------------------------------------------------------------------------

/**
 * Read the running agent's full metadata. Heavyweight (host call +
 * oplog entry per invocation); for just the agent-id, prefer the
 * `SelfAgentId` Context service.
 */
export const getSelfMetadata: Effect.Effect<RawAgentMetadata, AgentsHostError> = Effect.try({
  try: () => getSelfMetadataImpl(),
  catch: (e) => new AgentsHostError(e),
})

/** Read another agent's metadata, or `undefined` if it does not exist. */
export const getAgentMetadata = (
  id: RawAgentId,
): Effect.Effect<RawAgentMetadata | undefined, AgentsHostError> =>
  Effect.try({
    try: () => getAgentMetadataImpl(id),
    catch: (e) => new AgentsHostError(e),
  })

/** Resolve a component reference to its `ComponentId`. */
export const resolveComponentId = (
  componentReference: string,
): Effect.Effect<RawComponentId | undefined, AgentsHostError> =>
  Effect.try({
    try: () => resolveComponentIdImpl(componentReference),
    catch: (e) => new AgentsHostError(e),
  })

/** Resolve a `(componentReference, agentName)` pair to an `AgentId`. */
export const resolveAgentId = (
  componentReference: string,
  agentName: string,
): Effect.Effect<RawAgentId | undefined, AgentsHostError> =>
  Effect.try({
    try: () => resolveAgentIdImpl(componentReference, agentName),
    catch: (e) => new AgentsHostError(e),
  })

/** Strict variant of {@link resolveAgentId}. */
export const resolveAgentIdStrict = (
  componentReference: string,
  agentName: string,
): Effect.Effect<RawAgentId | undefined, AgentsHostError> =>
  Effect.try({
    try: () => resolveAgentIdStrictImpl(componentReference, agentName),
    catch: (e) => new AgentsHostError(e),
  })

/**
 * Initiate an update of the given agent to `targetRevision`. The
 * revision is validated to fit a `u64`. Returns immediately — the
 * actual update is asynchronous on the host side.
 */
export const updateAgent = (input: {
  readonly agentId: RawAgentId
  readonly targetRevision: bigint | number
  readonly mode: RawUpdateMode
}): Effect.Effect<void, AgentsHostError | AgentsValidationError> =>
  Effect.gen(function* () {
    const target = yield* toUint64(input.targetRevision, "updateAgent.targetRevision")
    yield* Effect.try({
      try: () => updateAgentImpl(input.agentId, target, input.mode),
      catch: (e) => new AgentsHostError(e),
    })
  })

/** Fork another agent at a given oplog index. */
export const forkAgent = (input: {
  readonly source: RawAgentId
  readonly target: RawAgentId
  readonly oplogIdxCutOff: RawOplogIndex
}): Effect.Effect<void, AgentsHostError> =>
  Effect.try({
    try: () => forkAgentImpl(input.source, input.target, input.oplogIdxCutOff),
    catch: (e) => new AgentsHostError(e),
  })

/** Revert an agent to a previous state. */
export const revertAgent = (
  agentId: RawAgentId,
  target: RawRevertAgentTarget,
): Effect.Effect<void, AgentsHostError> =>
  Effect.try({
    try: () => revertAgentImpl(agentId, target),
    catch: (e) => new AgentsHostError(e),
  })

/**
 * Fork the current agent at the current execution point. Both the
 * original and the new ("forked") agent see this call return; inspect
 * the {@link RawForkResult} `tag` to discover which side you are on.
 */
export const fork: Effect.Effect<RawForkResult, AgentsHostError> = Effect.try({
  try: () => forkImpl(),
  catch: (e) => new AgentsHostError(e),
})

// ---------------------------------------------------------------------------
// `GetAgents` pager
// ---------------------------------------------------------------------------

/**
 * Stream all agents of the given component matching the optional
 * filter. Drives the host's `GetAgents` pager via `Stream.paginate`.
 */
export const getAgents = (input: {
  readonly componentId: RawComponentId
  readonly filter?: Filter | RawAgentAnyFilter | undefined
  readonly precise?: boolean
}): Stream.Stream<RawAgentMetadata, AgentsHostError> => {
  const rawFilter = isFilter(input.filter) ? toRawFilter(input.filter) : input.filter
  return Stream.unwrap(
    Effect.try({
      try: () => getAgentsCtorImpl(input.componentId, rawFilter, input.precise ?? false),
      catch: (e) => new AgentsHostError(e),
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
 */
export class PromiseAlreadyCompletedError {
  readonly _tag = "PromiseAlreadyCompletedError"
  readonly message: string
  constructor(readonly promiseId: ApiHost.PromiseId) {
    this.message = `PromiseAlreadyCompletedError: promise (oplog idx ${promiseId.oplogIdx.toString()}) was already completed`
  }
}

let createPromiseImpl: () => ApiHost.PromiseId = () => ApiHost.createPromise()
let getPromiseCtorImpl: (id: ApiHost.PromiseId) => ApiHost.GetPromiseResult = (id) =>
  ApiHost.getPromise(id)
let completePromiseImpl: (id: ApiHost.PromiseId, payload: Uint8Array) => boolean = (id, p) =>
  ApiHost.completePromise(id, p)

/** @internal */
export const __setCreatePromiseForTest = (fn: () => ApiHost.PromiseId): void => {
  createPromiseImpl = fn
}
/** @internal */
export const __resetCreatePromiseForTest = (): void => {
  createPromiseImpl = () => ApiHost.createPromise()
}
/** @internal */
export const __setGetPromiseCtorForTest = (
  fn: (id: ApiHost.PromiseId) => ApiHost.GetPromiseResult,
): void => {
  getPromiseCtorImpl = fn
}
/** @internal */
export const __resetGetPromiseCtorForTest = (): void => {
  getPromiseCtorImpl = (id) => ApiHost.getPromise(id)
}
/** @internal */
export const __setCompletePromiseForTest = (
  fn: (id: ApiHost.PromiseId, payload: Uint8Array) => boolean,
): void => {
  completePromiseImpl = fn
}
/** @internal */
export const __resetCompletePromiseForTest = (): void => {
  completePromiseImpl = (id, p) => ApiHost.completePromise(id, p)
}

/**
 * Effect-flavoured promise rendezvous. Thin wrappers around the
 * `golem:api/host@1.5.0` promise API: a host-side shared rendezvous
 * channel where one fiber creates + awaits while another fiber (often
 * external) completes.
 */
export const Promises = {
  /** Create a new host promise. */
  create: Effect.try({
    try: () => createPromiseImpl(),
    catch: (e: unknown) => new AgentsHostError(e),
  }) as Effect.Effect<ApiHost.PromiseId, AgentsHostError>,

  /** Poll a promise: returns `undefined` until completed. */
  poll: (id: ApiHost.PromiseId): Effect.Effect<Uint8Array | undefined, AgentsHostError> =>
    Effect.try({
      try: () => getPromiseCtorImpl(id).get(),
      catch: (e) => new AgentsHostError(e),
    }),

  /**
   * Await a promise's completion, returning the payload.
   * Bridges the host's `Pollable.subscribe()` -> `.promise()` chain
   * into Effect via `Effect.callback`. Best-effort interruption: when
   * the surrounding fiber is interrupted, the JS-side resolution is
   * dropped (the host pollable cannot be cancelled).
   */
  await: (id: ApiHost.PromiseId): Effect.Effect<Uint8Array, AgentsHostError> =>
    Effect.gen(function* () {
      const handle = yield* Effect.try({
        try: () => getPromiseCtorImpl(id),
        catch: (e) => new AgentsHostError(e),
      })
      // Fast path: already completed.
      const ready = yield* Effect.try({
        try: () => handle.get(),
        catch: (e) => new AgentsHostError(e),
      })
      if (ready !== undefined) return ready
      return yield* Effect.callback<Uint8Array, AgentsHostError>((resume) => {
        let cancelled = false
        try {
          const pollable = handle.subscribe()
          pollable
            .promise()
            .then(() => {
              if (cancelled) return
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
              if (!cancelled) resume(Effect.fail(new AgentsHostError(e)))
            })
        } catch (e) {
          resume(Effect.fail(new AgentsHostError(e)))
        }
        return Effect.sync(() => {
          cancelled = true
        })
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
  ): Effect.Effect<true, AgentsHostError | PromiseAlreadyCompletedError> =>
    Effect.gen(function* () {
      const ok = yield* Effect.try({
        try: () => completePromiseImpl(id, payload),
        catch: (e) => new AgentsHostError(e),
      })
      if (!ok) return yield* Effect.fail(new PromiseAlreadyCompletedError(id))
      return true as const
    }),
} as const
