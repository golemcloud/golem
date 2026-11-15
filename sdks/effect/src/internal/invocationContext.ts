import type * as AgentCommon from "golem:agent/common@2.0.0"
import {
  AccountPrincipal,
  AgentPrincipal,
  AnonymousPrincipal,
  Caller,
  type InvocationContextShape,
  type Principal,
  TokenProvenance,
  ToolPrincipal,
  UserPrincipal,
} from "../InvocationContext.js"

/**
 * @internal
 * @since 2.0.0
 */
export const principalFromHost = (principal: AgentCommon.Principal): Principal => {
  switch (principal.tag) {
    case "user":
      return new UserPrincipal(principal.val)
    case "account":
      return new AccountPrincipal(principal.val)
    case "agent":
      return new AgentPrincipal(principal.val)
    case "tool":
      return new ToolPrincipal(principal.val)
    case "anonymous":
      return new AnonymousPrincipal()
  }
}

/**
 * Wrap the host's answer to `golem:agent/invocation-context.caller`.
 *
 * @internal
 * @since 2.0.0
 */
export const callerFromHost = (host: AgentCommon.Caller): Caller =>
  new Caller(
    principalFromHost(host.original),
    principalFromHost(host.immediate),
    host.tokens.map((token) => new TokenProvenance(token)),
  )

/**
 * A context whose caller is read from the host on first use and kept for the rest of the
 * invocation.
 *
 * @internal
 * @since 2.0.0
 */
export const lazyInvocationContext = (read: () => AgentCommon.Caller): InvocationContextShape => {
  let caller: Caller | undefined
  return {
    get caller(): Caller {
      caller ??= callerFromHost(read())
      return caller
    },
  }
}
