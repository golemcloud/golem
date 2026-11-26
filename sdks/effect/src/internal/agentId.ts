import type * as CoreTypes from "golem:core/types@2.0.0"
import { parseUuid, uuidToString } from "golem:core/types@2.0.0"
import type { AgentId } from "../AgentId.js"

/** The SDK form of a host agent ID. */
export const agentIdFromWit = (id: CoreTypes.AgentId): AgentId =>
  uuidToString(id.uuid).toLowerCase() as AgentId

/** The host form of an agent ID. Throws for an ID whose UUID text is invalid. */
export const agentIdToWit = (id: AgentId): CoreTypes.AgentId => ({ uuid: parseUuid(id) })
