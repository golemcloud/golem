/**
 * Typed IDs of agent invocations. The text form of an invocation ID is `<agent-id>.<uuid>`,
 * where `<agent-id>` is the UUID text of the agent ID.
 *
 * @since 1.6.0
 */
import { Effect } from "effect"
import type { AgentId } from "./AgentId.js"
import type { Uuid } from "./Ids.js"
import { makeInvocationId } from "./internal/invocationId.js"

/**
 * The ID of one invocation of an agent: the agent ID and the UUID of the invocation. The
 * `toString` of an ID that the SDK produces returns the `<agent-id>.<uuid>` text form, the same
 * text as {@link format}.
 *
 * @since 1.6.0
 * @category models
 */
export interface InvocationId {
  readonly agentId: AgentId
  readonly uuid: Uuid
}

/**
 * A text is not a valid invocation ID.
 *
 * @since 1.6.0
 * @category errors
 */
export class InvocationIdParseError {
  readonly _tag = "InvocationIdParseError"
  constructor(
    readonly input: string,
    readonly reason: string,
  ) {}
}

const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/**
 * Format an invocation ID as `<agent-id>.<uuid>`.
 *
 * @since 1.6.0
 * @category conversions
 */
export const format = (id: InvocationId): string => `${id.agentId}.${id.uuid}`

/**
 * Parse the `<agent-id>.<uuid>` text form. Both parts must be UUIDs in the hyphenated form;
 * upper-case hex is accepted and normalized to lower case.
 *
 * @since 1.6.0
 * @category conversions
 */
export const parse = (text: string): Effect.Effect<InvocationId, InvocationIdParseError> =>
  Effect.suspend(() => {
    const fail = (reason: string) => Effect.fail(new InvocationIdParseError(text, reason))
    const parts = text.split(".")
    if (parts.length !== 2) return fail("expected `<agent-id>.<uuid>`")
    const [agentId, uuid] = parts as [string, string]
    if (!uuidPattern.test(agentId)) return fail(`invalid agent ID '${agentId}'`)
    if (!uuidPattern.test(uuid)) return fail(`invalid UUID '${uuid}'`)
    return Effect.succeed(
      makeInvocationId(agentId.toLowerCase() as AgentId, uuid.toLowerCase() as Uuid),
    )
  })
