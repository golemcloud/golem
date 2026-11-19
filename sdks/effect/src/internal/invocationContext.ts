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
 * An invocation context that the SDK ends when its call returns.
 *
 * @internal
 * @since 2.0.0
 */
export interface EndableInvocationContext extends InvocationContextShape {
  /**
   * Marks the call as over. A first read of the caller after this throws, unless the call
   * returned a stream: its producer runs after the return, so the caller is read from the host
   * now, while the call still holds it, and kept for the producer.
   */
  readonly end: (returnedStream?: boolean) => void
}

/**
 * Whether the result of a call holds a stream, whose producer runs after the call returns: an
 * encoded method result, or a tool result with an encoded value.
 *
 * @internal
 * @since 2.0.0
 */
export const returnsStream = (result: unknown): boolean => {
  const tree =
    typeof result === "object" && result !== null && "result" in result
      ? (result as { readonly result?: { readonly value?: unknown } }).result?.value
      : result
  if (typeof tree !== "object" || tree === null || !("valueNodes" in tree)) return false
  const nodes = (tree as { readonly valueNodes: unknown }).valueNodes
  return (
    Array.isArray(nodes) &&
    nodes.some(
      (node) =>
        typeof node === "object" &&
        node !== null &&
        (node as { readonly tag?: unknown }).tag === "stream-value",
    )
  )
}

/**
 * A context whose caller is read from the host on first use and kept for the rest of the
 * invocation. Once the call has ended the host holds another invocation's caller, so a context
 * that has not read it by then refuses to, and one that has keeps the value it read.
 *
 * @internal
 * @since 2.0.0
 */
export const lazyInvocationContext = (read: () => AgentCommon.Caller): EndableInvocationContext => {
  let caller: Caller | undefined
  let ended = false
  return {
    get caller(): Caller {
      if (caller === undefined) {
        if (ended) {
          throw new Error(
            "this InvocationContext belongs to a call that has ended; store ctx.caller.original.ref instead",
          )
        }
        caller = callerFromHost(read())
      }
      return caller
    },
    end: (returnedStream = false) => {
      if (returnedStream && caller === undefined && !ended) {
        caller = callerFromHost(read())
      }
      ended = true
    },
  }
}
