import type * as AgentHost from "golem:agent/host@2.0.0"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { uuidToString } from "golem:core/types@2.0.0"
import type { AgentId } from "../src/AgentId.js"
import type { InvocationMetadata } from "../src/Client.js"
import type { Uuid } from "../src/Ids.js"

/** A stable agent ID for a test label: the label's hash goes into the low bits. */
export const hostAgentIdFor = (label: string): CoreTypes.AgentId => {
  let hash = 0n
  for (const char of label) hash = (hash * 131n + BigInt(char.codePointAt(0)!)) & 0xffff_ffff_ffffn
  return { uuid: { highBits: 0x0000_0001_0000_8000n, lowBits: 0x8000_0000_0000_0000n | hash } }
}

/** The SDK form of {@link hostAgentIdFor}. */
export const agentIdFor = (label: string): AgentId => ({
  uuid: uuidToString(hostAgentIdFor(label).uuid) as Uuid,
})

/** Host invocation metadata for the agent of `label`, with the invocation UUID `uuid`. */
export const hostInvocationMetadata = (
  label: string,
  uuid: { highBits: bigint; lowBits: bigint } = { highBits: 1n, lowBits: 2n },
): AgentHost.InvocationMetadata => ({
  agentId: hostAgentIdFor(label),
  invocationId: {
    owner: { tag: "agent", val: hostAgentIdFor(label) },
    id: uuid,
  },
})

/** The SDK form of `hostInvocationMetadata(label)` with the default invocation UUID. */
export const sdkInvocationMetadata = (label: string): InvocationMetadata => ({
  agentId: agentIdFor(label),
  invocationId: {
    agentId: agentIdFor(label),
    uuid: "00000000-0000-0001-0000-000000000002" as Uuid,
  },
})
