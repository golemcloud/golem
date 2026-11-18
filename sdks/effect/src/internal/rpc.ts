import { Effect, Exit } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { uuidToString } from "golem:core/types@2.0.0"
import type { InvocationMetadata, RemoteCallError } from "../Client.js"
import type { RpcCancellationToken, RpcConnection } from "../host/RpcClient.js"
import type { Uuid } from "../Ids.js"
import { makeInvocationId } from "./invocationId.js"
import { type GuestScopeCard, isScopeCard, peekScopeCard } from "./scopeCard.js"
import { restoreEncodedCapabilities } from "./schema-model/wit.js"

/**
 * Convert host invocation metadata to the SDK form. The invocation ID combines the agent ID text
 * with the UUID of the invocation; the owner case of the host ID is not used.
 */
export const invocationMetadataFromHost = (
  metadata: AgentHost.InvocationMetadata,
): InvocationMetadata =>
  Object.freeze({
    agentId: metadata.agentId,
    invocationId: makeInvocationId(
      metadata.agentId,
      uuidToString(metadata.invocationId.id) as Uuid,
    ),
  })

const RPC_TAGS = new Set<AgentHost.RpcError["tag"]>([
  "protocol-error",
  "denied",
  "not-found",
  "remote-internal-error",
  "remote-agent-error",
])

const extractRpcError = (error: unknown): AgentHost.RpcError | undefined => {
  const candidates = [error]
  if (error instanceof Error) candidates.push((error as { payload?: unknown }).payload, error.cause)
  return candidates.find((value): value is AgentHost.RpcError => {
    if (value === null || typeof value !== "object") return false
    const tag = (value as { tag?: unknown }).tag
    return typeof tag === "string" && RPC_TAGS.has(tag as AgentHost.RpcError["tag"])
  })
}

export const wrapHostThrow = (error: unknown): RemoteCallError => ({
  _tag: "RpcCallError",
  cause: extractRpcError(error) ?? {
    tag: "protocol-error",
    val: error instanceof Error ? error.message : String(error),
  },
})

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
 * Once `asyncInvokeAndAwait` is called, nothing is restored, even when it throws: the host may
 * already own the input. A handle without metadata is a rejected call: its future fails with the
 * rejection error.
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
        Effect.acquireUseRelease(
          borrowScopeCard(scopeCard).pipe(
            Effect.flatMap((raw) =>
              Effect.try({
                try: () => {
                  unsent = undefined
                  return rpc.asyncInvokeAndAwait(method, tree, raw)
                },
                catch: wrapHostThrow,
              }),
            ),
            Effect.map((invocation) => ({
              invocation,
              metadata: invocation.metadata && invocationMetadataFromHost(invocation.metadata),
            })),
          ),
          ({ invocation, metadata }) =>
            Effect.tryPromise({ try: () => invocation.get(), catch: wrapHostThrow }).pipe(
              Effect.flatMap((result) =>
                metadata === undefined
                  ? Effect.fail<RemoteCallError>({
                      _tag: "RpcCallError",
                      cause: {
                        tag: "protocol-error",
                        val: `${method}: host completed a rejected invocation`,
                      },
                    })
                  : Effect.succeed({ metadata, result }),
              ),
            ),
          ({ invocation }) =>
            Effect.sync(() => {
              bestEffort(() => invocation.cancel())
              bestEffort(() => invocation.drop())
            }),
        ),
      ),
      Effect.onExit((exit) =>
        restoreUnsent && Exit.isFailure(exit) && unsent !== undefined
          ? Effect.sync(() => restoreEncodedCapabilities(unsent!))
          : Effect.void,
      ),
    )
  })

export const cancelOnce = (token: RpcCancellationToken): Effect.Effect<void> => {
  let consumed = false
  return Effect.sync(() => {
    if (consumed) return
    consumed = true
    bestEffort(() => token.cancel())
  })
}

export const scheduleCancelableInvocation = (
  rpc: RpcConnection,
  at: AgentHost.Datetime,
  method: string,
  input: CoreTypes.SchemaValueTree,
) =>
  Effect.acquireRelease(
    Effect.try({
      try: () => rpc.scheduleCancelableInvocation(at, method, input),
      catch: wrapHostThrow,
    }),
    ({ token }) => Effect.sync(() => bestEffort(() => token.drop())),
  ).pipe(
    Effect.map(({ metadata, token }) => ({
      metadata: invocationMetadataFromHost(metadata),
      cancel: cancelOnce(token),
    })),
  )
