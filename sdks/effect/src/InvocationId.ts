/**
 * Typed IDs of agent invocations. The text form of an invocation ID is `<agent-id>.<uuid>`.
 *
 * @since 1.6.0
 */
import { Effect } from "effect"
import type { Uuid } from "./Ids.js"
import { makeInvocationId } from "./internal/invocationId.js"

/**
 * The ID of one invocation of an agent: the agent ID text and the UUID of the invocation. The
 * `toString` of an ID that the SDK produces returns the `<agent-id>.<uuid>` text form, the same
 * text as {@link format}.
 *
 * @since 1.6.0
 * @category models
 */
export interface InvocationId {
  readonly agentId: string
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
const suffixLength = 37

/**
 * Format an invocation ID as `<agent-id>.<uuid>`.
 *
 * @since 1.6.0
 * @category conversions
 */
export const format = (id: InvocationId): string => `${id.agentId}.${id.uuid}`

/**
 * Parse the `<agent-id>.<uuid>` text form. The last 37 characters are the `.` and the UUID, so a
 * `.` in the agent ID is permitted. The UUID must be in the hyphenated form; upper-case hex is
 * accepted and normalized to lower case. The parse fails for a missing `.`, an invalid UUID, or
 * an empty agent ID.
 *
 * @since 1.6.0
 * @category conversions
 */
export const parse = (text: string): Effect.Effect<InvocationId, InvocationIdParseError> =>
  Effect.suspend(() => {
    const fail = (reason: string) => Effect.fail(new InvocationIdParseError(text, reason))
    if (text.length < suffixLength || text[text.length - suffixLength] !== ".")
      return fail("expected `<agent-id>.<uuid>`")
    const uuid = text.slice(text.length - suffixLength + 1)
    if (!uuidPattern.test(uuid)) return fail(`invalid UUID '${uuid}'`)
    const agentId = text.slice(0, text.length - suffixLength)
    if (agentId.length === 0) return fail("empty agent ID")
    return Effect.succeed(makeInvocationId(agentId, uuid.toLowerCase() as Uuid))
  })
