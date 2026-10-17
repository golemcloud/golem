import type * as Common from "golem:tool/common@0.1.0"
import type * as Core from "golem:core/types@2.0.0"
import { Effect, Exit } from "effect"
import { abandonGuestQuotaTokenWireHandle } from "../schema-model/quotaTokenHandle.js"
import { QUOTA_INTERNAL } from "../schema-model/quotaInternal.js"
import { abandonGuestPermissionCardWireHandle } from "../schema-model/permissionCardHandle.js"
import { PERMISSION_CARD_INTERNAL } from "../schema-model/permissionCardInternal.js"
import { restoreCapabilityAdoption } from "./collectionOwnership.js"

export function discardRejectedPayload<A, E, R>(
  payload: Common.TypedSchemaValue | undefined,
  decode: Effect.Effect<A, E, R>,
): Effect.Effect<A, E, R> {
  return Effect.onExit(decode, (exit) =>
    Exit.isFailure(exit) && payload
      ? Effect.promise(() => discardSchemaValueTree(payload.value))
      : Effect.void,
  )
}

export async function discardSchemaValueTree(tree: Core.SchemaValueTree): Promise<void> {
  const seen = new Set<object>()
  const cleanups: Promise<void>[] = []
  const nodes = tree?.valueNodes
  if (!Array.isArray(nodes)) return
  restoreCapabilityAdoption(tree)
  for (const node of nodes) {
    if (
      node?.tag !== "secret-value" &&
      node?.tag !== "quota-token-handle" &&
      node?.tag !== "permission-card-handle" &&
      node?.tag !== "stream-value"
    )
      continue
    const raw = node.val
    ;(node as { val: unknown }).val = undefined
    if (!raw || seen.has(raw)) continue
    seen.add(raw)
    cleanups.push(
      (async () => {
        try {
          if (node.tag === "quota-token-handle")
            abandonGuestQuotaTokenWireHandle(QUOTA_INTERNAL, raw as Core.QuotaToken, node)
          else if (node.tag === "permission-card-handle")
            abandonGuestPermissionCardWireHandle(
              PERMISSION_CARD_INTERNAL,
              raw as Core.PermissionCard,
              node,
            )
          if (node.tag === "stream-value") {
            const { SchemaValueStream } = await import("golem:core/types@2.0.0")
            const source = await SchemaValueStream.unwrap(raw as Core.SchemaValueStream)
            await source[Symbol.asyncIterator]().return?.()
          }
        } catch {
          // Continue releasing siblings if a handle is malformed or a stream cannot close.
        } finally {
          try {
            ;(raw as { [Symbol.dispose]?: () => void })[Symbol.dispose]?.()
          } catch {
            // Preserve the rejection and release the remaining resources.
          }
        }
      })(),
    )
  }
  await Promise.all(cleanups)
}
