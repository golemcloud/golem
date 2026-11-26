import { Effect, Exit, Scope } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { uuidToString } from "golem:core/types@2.0.0"
import type { InvocationMetadata, RemoteCallError } from "../Client.js"
import type { RpcConnection, RpcInvocation } from "../host/RpcClient.js"
import type { Uuid } from "../Ids.js"
import type { Invocation, InvocationId } from "../InvocationId.js"
import { agentIdFromWit } from "./agentId.js"
import { makeInvocationId } from "./invocationId.js"
import { type GuestScopeCard, isScopeCard, peekScopeCard } from "./scopeCard.js"
import { restoreEncodedCapabilities } from "./schema-model/wit.js"

/**
 * Convert host invocation metadata to the SDK form. The invocation ID combines the agent ID
 * with the UUID of the invocation; the owner case of the host ID is not used.
 */
export const invocationMetadataFromHost = (
  metadata: AgentHost.InvocationMetadata,
): InvocationMetadata => {
  const agentId = agentIdFromWit(metadata.agentId)
  return Object.freeze({
    agentId,
    invocationId: makeInvocationId(agentId, uuidToString(metadata.invocationId.id) as Uuid),
  })
}

const RPC_TAGS: ReadonlySet<string> = new Set(
  Object.keys({
    "protocol-error": true,
    denied: true,
    "not-found": true,
    "remote-internal-error": true,
    "remote-agent-error": true,
    conflict: true,
    cancelled: true,
    "cannot-cancel-running": true,
  } satisfies Record<AgentHost.RpcError["tag"], true>),
)

const extractRpcError = (error: unknown): AgentHost.RpcError | undefined => {
  const candidates = [error]
  if (error instanceof Error) candidates.push((error as { payload?: unknown }).payload, error.cause)
  return candidates.find((value): value is AgentHost.RpcError => {
    if (value === null || typeof value !== "object") return false
    const tag = (value as { tag?: unknown }).tag
    return typeof tag === "string" && RPC_TAGS.has(tag)
  })
}

export const wrapHostThrow = (error: unknown): RemoteCallError => {
  const cause = extractRpcError(error) ?? {
    tag: "protocol-error",
    val: error instanceof Error ? error.message : String(error),
  }
  switch (cause.tag) {
    case "cancelled":
      return { _tag: "RpcCancelled", reason: cause.val }
    case "cannot-cancel-running":
      return { _tag: "RpcCannotCancelRunning", reason: cause.val }
    default:
      return { _tag: "RpcCallError", cause }
  }
}

const bestEffort = (operation: () => void): void => {
  try {
    operation()
  } catch {
    // Host resource cleanup must not replace the operation's result.
  }
}

const scopeCardUnavailable = (reason: string): RemoteCallError => ({
  _tag: "ScopeCardUnavailableError",
  reason,
})

/** Borrow a scope card for one awaited call. A released or foreign card fails the call. */
const borrowScopeCard = (
  card: GuestScopeCard | undefined,
): Effect.Effect<CoreTypes.PermissionCard | undefined, RemoteCallError> =>
  Effect.suspend(() => {
    if (card === undefined) return Effect.succeed(undefined)
    if (!isScopeCard(card))
      return Effect.fail(scopeCardUnavailable("the value is not a scope card"))
    const raw = peekScopeCard(card)
    return raw === undefined
      ? Effect.fail(scopeCardUnavailable("the scope card was released"))
      : Effect.succeed(raw)
  })

/**
 * Run one awaited invocation. The scope card is checked before `input` runs, and again when the
 * host is called. When the call fails or is interrupted before the host receives the encoded
 * input, the capabilities that the encoder moved into it go back to their handles (see
 * `restoreEncodedCapabilities`). Set `restoreUnsent` to `false` for a tree that the caller built:
 * it then stays as the caller supplied it.
 *
 * Once `invokeAndAwait` is called, nothing is restored, even when it fails: the host may already
 * own the input. Interrupting the call aborts the signal of the host call, which drops it: the
 * host withdraws the invocation on a best-effort basis while it has not started.
 */
export const awaitInvocation = <R>(
  rpc: RpcConnection,
  method: string,
  input: Effect.Effect<CoreTypes.SchemaValueTree, RemoteCallError, R>,
  scopeCard?: GuestScopeCard,
  restoreUnsent = true,
) =>
  Effect.suspend(() => {
    let unsent: CoreTypes.SchemaValueTree | undefined
    return borrowScopeCard(scopeCard).pipe(
      Effect.andThen(
        Effect.uninterruptibleMask((restore) =>
          Effect.tap(restore(input), (tree) =>
            Effect.sync(() => {
              unsent = tree
            }),
          ),
        ),
      ),
      Effect.flatMap((tree) =>
        borrowScopeCard(scopeCard).pipe(
          Effect.flatMap((raw) =>
            Effect.tryPromise({
              try: (signal) => {
                unsent = undefined
                return rpc.invokeAndAwait(method, tree, raw, signal)
              },
              catch: wrapHostThrow,
            }),
          ),
        ),
      ),
      Effect.map((completed) => ({
        metadata: invocationMetadataFromHost(completed.metadata),
        result: completed.result,
      })),
      Effect.onExit((exit) =>
        restoreUnsent && Exit.isFailure(exit) && unsent !== undefined
          ? Effect.sync(() => restoreEncodedCapabilities(unsent!))
          : Effect.void,
      ),
    )
  })

/** The SDK form of the ID of a host invocation handle. */
const invocationIdFromHost = (
  id: CoreTypes.InvocationId,
): Effect.Effect<InvocationId, RemoteCallError> =>
  id.owner.tag === "agent"
    ? Effect.succeed(makeInvocationId(agentIdFromWit(id.owner.val), uuidToString(id.id) as Uuid))
    : Effect.fail({
        _tag: "RpcCallError",
        cause: { tag: "protocol-error", val: `invocation ${id.owner.tag} owner is not an agent` },
      })

/**
 * Wrap a host invocation handle. `decode` turns the awaited result into the value of the
 * invocation. Interrupting `awaitResult` drops the host await, which only detaches.
 */
export const makeInvocation = <A, E>(
  raw: RpcInvocation,
  decode: (tree: CoreTypes.SchemaValueTree | undefined) => Effect.Effect<A, RemoteCallError | E>,
): Effect.Effect<Invocation<A, E>, RemoteCallError> =>
  Effect.map(invocationIdFromHost(raw.id), (id) =>
    Object.freeze({
      id,
      status: Effect.tryPromise({ try: () => raw.status(), catch: wrapHostThrow }),
      awaitResult: Effect.tryPromise({
        try: (signal) => raw.await(signal),
        catch: wrapHostThrow,
      }).pipe(Effect.flatMap(decode)),
      cancel: Effect.tryPromise({ try: () => raw.cancel(), catch: wrapHostThrow }),
    }),
  )

/** The handle of the same invocation, with `f` applied to its awaited result. */
export const mapInvocation = <A, E, B, E2>(
  invocation: Invocation<A, E>,
  f: (value: A) => Effect.Effect<B, E2>,
): Invocation<B, E | E2> =>
  Object.freeze({ ...invocation, awaitResult: Effect.flatMap(invocation.awaitResult, f) })

/** Own a host invocation handle in the current scope; closing the scope only detaches. */
export const scopedInvocation = <A, E>(
  acquire: Effect.Effect<RpcInvocation, RemoteCallError>,
  decode: (tree: CoreTypes.SchemaValueTree | undefined) => Effect.Effect<A, RemoteCallError | E>,
): Effect.Effect<Invocation<A, E>, RemoteCallError, Scope.Scope> =>
  Effect.acquireRelease(acquire, (raw) => Effect.sync(() => bestEffort(() => raw.drop()))).pipe(
    Effect.flatMap((raw) => makeInvocation(raw, decode)),
  )

/** Trigger `method` and own the handle of the accepted invocation in the current scope. */
export const triggerInvocation = <A, E>(
  rpc: RpcConnection,
  method: string,
  input: CoreTypes.SchemaValueTree,
  decode: (tree: CoreTypes.SchemaValueTree | undefined) => Effect.Effect<A, RemoteCallError | E>,
) =>
  scopedInvocation(
    Effect.tryPromise({ try: () => rpc.trigger(method, input), catch: wrapHostThrow }),
    decode,
  )

/** Schedule `method` at `at` and own the handle of the accepted invocation in the current scope. */
export const scheduleInvocation = <A, E>(
  rpc: RpcConnection,
  at: AgentHost.Datetime,
  method: string,
  input: CoreTypes.SchemaValueTree,
  decode: (tree: CoreTypes.SchemaValueTree | undefined) => Effect.Effect<A, RemoteCallError | E>,
) =>
  scopedInvocation(
    Effect.tryPromise({ try: () => rpc.schedule(at, method, input), catch: wrapHostThrow }),
    decode,
  )
