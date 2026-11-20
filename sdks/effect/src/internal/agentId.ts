import type * as CoreTypes from "golem:core/types@2.0.0"
import { parseUuid, uuidToString } from "golem:core/types@2.0.0"
import type { AgentId } from "../AgentId.js"
import type { Uuid } from "../Ids.js"

const prototype = Object.freeze(
  Object.create(Object.prototype, {
    toString: {
      value(this: AgentId): string {
        return this.uuid
      },
    },
  }),
)

/** Build an immutable agent ID whose `toString` returns the UUID text. */
export const typedAgentId = (uuid: Uuid): AgentId =>
  Object.freeze(Object.assign(Object.create(prototype) as object, { uuid }))

/** The SDK form of a host agent ID. */
export const agentIdFromWit = (id: CoreTypes.AgentId): AgentId =>
  typedAgentId(uuidToString(id.uuid) as Uuid)

/** The host form of an agent ID. Throws for an ID whose UUID text is invalid. */
export const agentIdToWit = (id: AgentId): CoreTypes.AgentId => ({ uuid: parseUuid(id.uuid) })
