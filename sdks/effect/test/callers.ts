/**
 * Host-shaped callers for tests: the values `golem:agent/invocation-context.caller` reports.
 */
import type * as AgentCommon from "golem:agent/common@2.0.0"
import { hostAgentIdFor } from "./invocation-metadata.js"

export const issuer = "https://example.test"

const uuid = (n: bigint): AgentCommon.Uuid => ({ highBits: 0n, lowBits: n })

export const anonymousPrincipal: AgentCommon.Principal = { tag: "anonymous" }

export const userPrincipal = (
  subject: string,
  principalIssuer: string = issuer,
): AgentCommon.Principal => ({
  tag: "user",
  val: {
    pair: { issuer: principalIssuer, subject },
    claims: "{}",
    authenticatedAt: { seconds: 1_700_000_000n, nanoseconds: 0 },
  },
})

export const accountPrincipal = (n: bigint = 1n): AgentCommon.Principal => ({
  tag: "account",
  val: {
    pair: { issuer: "golem", subject: `account:${n}` },
    accountId: { uuid: uuid(n) },
    authentication: "api-token",
  },
})

export const agentPrincipal = (name: string = "caller-agent"): AgentCommon.Principal => ({
  tag: "agent",
  val: {
    pair: { issuer: "golem", subject: `agent:${name}` },
    agentId: hostAgentIdFor(name),
    agentType: "CallerAgent",
    environmentId: { uuid: uuid(3n) },
  },
})

export const toolPrincipal = (toolName: string = "caller-tool"): AgentCommon.Principal => ({
  tag: "tool",
  val: {
    pair: { issuer: "golem", subject: `tool:${toolName}` },
    environmentId: { uuid: uuid(3n) },
    toolName,
  },
})
