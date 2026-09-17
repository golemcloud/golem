import { Effect } from "effect"
import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import type { RemoteCallError } from "../Client.js"
import type { RpcCancellationToken, RpcConnection } from "../host/RpcClient.js"

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

export const awaitInvocation = (
  rpc: RpcConnection,
  method: string,
  input: CoreTypes.SchemaValueTree,
) =>
  Effect.acquireUseRelease(
    Effect.try({ try: () => rpc.asyncInvokeAndAwait(method, input), catch: wrapHostThrow }),
    (invocation) =>
      Effect.tryPromise({ try: () => invocation.get(), catch: wrapHostThrow }).pipe(
        Effect.map((result) => ({ metadata: invocation.metadata, result })),
      ),
    (invocation) =>
      Effect.sync(() => {
        bestEffort(() => invocation.cancel())
        bestEffort(() => invocation.drop())
      }),
  )

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
  ).pipe(Effect.map(({ metadata, token }) => ({ metadata, cancel: cancelOnce(token) })))
