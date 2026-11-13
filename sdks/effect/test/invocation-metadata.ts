import type * as AgentHost from "golem:agent/host@2.0.0"
import type { InvocationMetadata } from "../src/Client.js"
import type { Uuid } from "../src/Ids.js"

/** Host invocation metadata for `agentId`, with the invocation UUID `uuid`. */
export const hostInvocationMetadata = (
  agentId: string,
  uuid: { highBits: bigint; lowBits: bigint } = { highBits: 1n, lowBits: 2n },
): AgentHost.InvocationMetadata => ({
  agentId,
  invocationId: {
    owner: {
      tag: "agent",
      val: { componentId: { uuid: { highBits: 0n, lowBits: 0n } }, agentId },
    },
    id: uuid,
  },
})

/** The SDK form of `hostInvocationMetadata(agentId)` with the default invocation UUID. */
export const sdkInvocationMetadata = (agentId: string): InvocationMetadata => ({
  agentId,
  invocationId: { agentId, uuid: "00000000-0000-0001-0000-000000000002" as Uuid },
})
