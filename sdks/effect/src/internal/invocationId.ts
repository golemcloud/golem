import type { Uuid } from "../Ids.js"
import type { InvocationId } from "../InvocationId.js"

const prototype = Object.freeze(
  Object.create(Object.prototype, {
    toString: {
      value(this: InvocationId): string {
        return `${this.agentId}.${this.uuid}`
      },
    },
  }),
)

/** Build an immutable invocation ID whose `toString` returns `<agent-id>.<uuid>`. */
export const makeInvocationId = (agentId: string, uuid: Uuid): InvocationId =>
  Object.freeze(Object.assign(Object.create(prototype) as object, { agentId, uuid }))
