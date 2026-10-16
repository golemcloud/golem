import type * as Common from "golem:tool/common@0.1.0"
import { Context } from "effect"

// Keep adoption rollback tied to the wire tree, not to its mutable nodes. A
// successful public delivery leaves the decoded handles owned by the caller.
const adoptions = new WeakMap<object, () => void>()

export const retainCapabilityAdoption = (tree: object, rollback: () => void): void => {
  adoptions.set(tree, rollback)
}

export const restoreCapabilityAdoption = (tree: object): void => {
  const rollback = adoptions.get(tree)
  adoptions.delete(tree)
  rollback?.()
}

// Result decoding can change the public value, but collection still owns the wire
// capabilities until all output channels have settled and collection returns.
export class CollectionOwnership extends Context.Service<
  CollectionOwnership,
  {
    readonly retain: (payload: Common.TypedSchemaValue | undefined) => void
    readonly retainDiscard: (discard: () => Promise<void>) => void
  }
>()("effect-golem/internal/CollectionOwnership") {}
