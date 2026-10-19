import type * as CoreTypes from "golem:core/types@2.0.0"
import type { Identity } from "../AgentIdentity.js"

const rawPhantoms = new WeakMap<Identity, CoreTypes.Uuid | undefined>()

export const retainRawPhantomId = (
  identity: Identity,
  phantom: CoreTypes.Uuid | undefined,
): void => {
  rawPhantoms.set(identity, phantom)
}

export const rawPhantomId = (identity: Identity): CoreTypes.Uuid | undefined => {
  if (!rawPhantoms.has(identity)) {
    throw new TypeError(
      "Expected an AgentIdentity created by AgentIdentity.parse or AgentIdentity.make",
    )
  }
  return rawPhantoms.get(identity)
}
